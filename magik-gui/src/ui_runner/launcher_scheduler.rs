// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

pub(super) const CATALOG_MESSAGES_PER_FRAME: usize = 2;
pub(super) const MEDIA_MESSAGES_PER_FRAME: usize = 2;

#[derive(Default)]
pub(super) struct CatalogJobEventBuf {
    events: Vec<CatalogWorkerMessage>,
}

impl CatalogJobEventBuf {
    pub(super) fn new() -> Self {
        Self {
            events: Vec::with_capacity(8),
        }
    }

    pub(super) fn clear(&mut self) {
        self.events.clear();
    }

    fn push(&mut self, event: CatalogWorkerMessage) {
        self.events.push(event);
    }

    pub(super) fn drain(&mut self) -> impl Iterator<Item = CatalogWorkerMessage> + '_ {
        self.events.drain(..)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.events.len()
    }

    #[cfg(test)]
    fn capacity(&self) -> usize {
        self.events.capacity()
    }
}

#[derive(Default)]
pub(super) struct MediaJobEventBuf {
    events: Vec<MediaWorkerMessage>,
}

impl MediaJobEventBuf {
    pub(super) fn new() -> Self {
        Self {
            events: Vec::with_capacity(8),
        }
    }

    pub(super) fn clear(&mut self) {
        self.events.clear();
    }

    fn push(&mut self, event: MediaWorkerMessage) {
        self.events.push(event);
    }

    pub(super) fn drain(&mut self) -> impl Iterator<Item = MediaWorkerMessage> + '_ {
        self.events.drain(..)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.events.len()
    }

    #[cfg(test)]
    fn capacity(&self) -> usize {
        self.events.capacity()
    }
}

enum CatalogJobState {
    Idle,
    Running(mpsc::Receiver<CatalogWorkerMessage>),
}

enum SearchIndexJobState {
    Idle,
    Running(mpsc::Receiver<CatalogWorkerMessage>),
}

enum MediaJobState {
    Idle,
    Running(MediaWorkerHandle),
    Unavailable,
}

pub(super) struct LauncherScheduler {
    catalog: CatalogJobState,
    search_index: SearchIndexJobState,
    search_index_generation: Arc<AtomicUsize>,
    search_index_allowed: Arc<AtomicBool>,
    media: MediaJobState,
    launch_handoff: LaunchHandoffSession,
}

impl LauncherScheduler {
    pub(super) fn new(launch_handoff_bench_enabled: bool) -> Self {
        Self {
            catalog: CatalogJobState::Idle,
            search_index: SearchIndexJobState::Idle,
            search_index_generation: Arc::new(AtomicUsize::new(0)),
            search_index_allowed: Arc::new(AtomicBool::new(false)),
            media: MediaJobState::Idle,
            launch_handoff: LaunchHandoffSession::from_env(launch_handoff_bench_enabled),
        }
    }

    pub(super) fn catalog_worker_running(&self) -> bool {
        matches!(self.catalog, CatalogJobState::Running(_))
    }

    pub(super) fn catalog_messages_running(&self) -> bool {
        self.catalog_worker_running()
            || matches!(self.search_index, SearchIndexJobState::Running(_))
    }

    pub(super) fn search_index_running(&self) -> bool {
        matches!(self.search_index, SearchIndexJobState::Running(_))
    }

    pub(super) fn set_search_index_allowed(&self, allowed: bool) {
        self.search_index_allowed.store(allowed, Ordering::Release);
    }

    /// This is entered by the catalog session only after persistence (when
    /// required) and the launcher's interaction-aware idle gate. Starting a
    /// new generation cancels any stale index job at its next cooperative
    /// checkpoint.
    pub(super) fn start_search_index(
        &mut self,
        job: mister_magik_catalog::arcade_catalog::ArcadeTextIndexBuildJob,
        games: usize,
        source: CatalogSource,
    ) {
        let generation = self.search_index_generation.fetch_add(1, Ordering::AcqRel) + 1;
        let active_generation = Arc::clone(&self.search_index_generation);
        let search_index_allowed = Arc::clone(&self.search_index_allowed);
        let text_index_token = job.text_index_token();
        let (tx, rx) = mpsc::channel();
        self.search_index = SearchIndexJobState::Running(rx);
        if std::thread::Builder::new()
            .name("arcade-search-index".to_string())
            .spawn(move || {
                let lease = mister_magik_catalog::work_coordinator::background("search-index");
                if tx
                    .send(CatalogWorkerMessage::SearchIndexBuildStarted {
                        text_index_token,
                        games,
                        source,
                    })
                    .is_err()
                {
                    return;
                }
                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::SearchIndex,
                );
                let Some(timing) = job.build_with_timing_while(|| {
                    lease.cooperate();
                    while !search_index_allowed.load(Ordering::Acquire) {
                        if active_generation.load(Ordering::Acquire) != generation {
                            return false;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    active_generation.load(Ordering::Acquire) == generation
                }) else {
                    return;
                };
                let _ = tx.send(CatalogWorkerMessage::SearchIndexesReady {
                    text_index_token,
                    games,
                    source,
                    timing,
                });
            })
            .is_err()
        {
            self.search_index = SearchIndexJobState::Idle;
        }
    }

    pub(super) fn start_catalog_worker(
        &mut self,
        root: String,
        request: CatalogWorkerRequest,
        initial_cache: CatalogWorkerInitialCache,
        execution_mode: CatalogExecutionMode,
    ) {
        self.catalog = CatalogJobState::Running(start_library_catalog_worker(
            root,
            request,
            initial_cache,
            execution_mode,
        ));
    }

    pub(super) fn poll_catalog(&mut self, out: &mut CatalogJobEventBuf) -> bool {
        out.clear();
        let mut disconnected = false;
        if let CatalogJobState::Running(rx) = &self.catalog {
            for _ in 0..CATALOG_MESSAGES_PER_FRAME {
                match rx.try_recv() {
                    Ok(message) => out.push(message),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            self.catalog = CatalogJobState::Idle;
        }
        let mut search_disconnected = false;
        if let SearchIndexJobState::Running(rx) = &self.search_index {
            while out.events.len() < CATALOG_MESSAGES_PER_FRAME {
                match rx.try_recv() {
                    Ok(message) => out.push(message),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        search_disconnected = true;
                        break;
                    }
                }
            }
        }
        if search_disconnected {
            self.search_index = SearchIndexJobState::Idle;
        }
        disconnected
    }

    pub(super) fn media_worker_running(&self) -> bool {
        matches!(self.media, MediaJobState::Running(_))
    }

    pub(super) fn media_worker_unavailable(&self) -> bool {
        matches!(self.media, MediaJobState::Unavailable)
    }

    pub(super) fn ensure_media_worker_started(&mut self, start: Instant, mode: &str) {
        if !matches!(self.media, MediaJobState::Idle) {
            return;
        }
        match start_screenshot_media_worker() {
            Some(handle) => {
                self.media = MediaJobState::Running(handle);
                print_startup_event(
                    start,
                    "screenshot_media_worker_start",
                    format!("mode={mode}"),
                );
            }
            None => {
                self.media = MediaJobState::Unavailable;
                print_startup_event(
                    start,
                    "screenshot_media_worker_skip",
                    format!("mode={mode}"),
                );
            }
        }
    }

    pub(super) fn ensure_media_system(&self, system_id: &str) {
        if let MediaJobState::Running(handle) = &self.media {
            handle.ensure_system(system_id);
        }
    }

    pub(super) fn finish_media_worker(&self) {
        if let MediaJobState::Running(handle) = &self.media {
            handle.finish();
        }
    }

    pub(super) fn drop_media_worker(&mut self) {
        self.media = MediaJobState::Idle;
    }

    pub(super) fn mark_media_worker_unavailable(&mut self) {
        self.media = MediaJobState::Unavailable;
    }

    pub(super) fn set_media_interaction_active(&self, active: bool, reason: &str) {
        if let MediaJobState::Running(handle) = &self.media {
            handle.set_interaction_active(active, reason);
        }
    }

    pub(super) fn poll_media(&mut self, out: &mut MediaJobEventBuf) {
        out.clear();
        if let MediaJobState::Running(handle) = &self.media {
            for _ in 0..MEDIA_MESSAGES_PER_FRAME {
                let Some(message) = handle.try_recv() else {
                    break;
                };
                out.push(message);
            }
        }
    }

    pub(super) fn record_loading_frame(&mut self, loop_start: Instant) {
        self.launch_handoff.record_loading_frame(loop_start);
    }

    pub(super) fn launch_loading_title(&self) -> &str {
        self.launch_handoff.loading_title()
    }

    pub(super) fn visible_loading_title<'a>(&'a self, fallback: &'a str) -> &'a str {
        self.launch_handoff.visible_loading_title(fallback)
    }

    pub(super) fn launch_is_active(&self) -> bool {
        self.launch_handoff.is_active()
    }

    pub(super) fn has_pending_launch(&self) -> bool {
        self.launch_handoff.has_pending_launch()
    }

    pub(super) fn launch_benchmark_enabled(&self) -> bool {
        self.launch_handoff.benchmark_enabled()
    }

    pub(super) fn should_request_benchmark_launch(&self) -> bool {
        self.launch_handoff.should_request_benchmark_launch()
    }

    pub(super) fn begin_launch(
        &mut self,
        nav: &LauncherNav,
        catalog: &ArcadeCatalog,
        durable_catalog_fingerprint: Option<&str>,
        launch_ref: &str,
        now: Instant,
    ) -> bool {
        self.launch_handoff
            .begin_launch(nav, catalog, durable_catalog_fingerprint, launch_ref, now)
    }

    pub(super) fn complete_loading_frame(&mut self, loading_presented: Instant) {
        self.launch_handoff
            .complete_loading_frame(loading_presented);
    }

    pub(super) fn poll_launch_completion(
        &mut self,
        result_received: Instant,
    ) -> Option<LaunchHandoffCompletion> {
        self.launch_handoff.poll_completion(result_received)
    }

    pub(super) fn stop_spawned_mister_for_recovery(&mut self) -> bool {
        self.launch_handoff.stop_spawned_mister_for_recovery()
    }

    pub(super) fn finish_launch_failure_recovery(&mut self, recovery_presented: Instant) {
        self.launch_handoff
            .finish_failure_recovery(recovery_presented);
    }

    pub(super) fn launch_runtime_action(&self, now: Instant) -> Option<LaunchHandoffRuntimeAction> {
        self.launch_handoff.runtime_action(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_buffers_are_reused_without_shrinking() {
        let mut catalog = CatalogJobEventBuf::new();
        let mut media = MediaJobEventBuf::new();
        let catalog_capacity = catalog.capacity();
        let media_capacity = media.capacity();

        catalog.clear();
        media.clear();

        assert_eq!(catalog.capacity(), catalog_capacity);
        assert_eq!(media.capacity(), media_capacity);
    }

    #[test]
    fn scheduler_starts_without_background_workers() {
        let scheduler = LauncherScheduler::new(false);

        assert!(!scheduler.catalog_worker_running());
        assert!(!scheduler.media_worker_running());
        assert!(!scheduler.media_worker_unavailable());
        assert!(!scheduler.launch_benchmark_enabled());
    }

    #[test]
    fn catalog_poll_is_budgeted_per_frame() {
        let (tx, rx) = mpsc::channel();
        for idx in 0..3 {
            tx.send(CatalogWorkerMessage::Timing {
                name: format!("event-{idx}"),
                detail: String::new(),
            })
            .unwrap();
        }
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.catalog = CatalogJobState::Running(rx);
        let mut events = CatalogJobEventBuf::new();

        scheduler.poll_catalog(&mut events);
        assert_eq!(events.len(), CATALOG_MESSAGES_PER_FRAME);

        scheduler.poll_catalog(&mut events);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn catalog_poll_releases_disconnected_worker_after_buffer_is_drained() {
        let (tx, rx) = mpsc::channel();
        tx.send(CatalogWorkerMessage::Done).unwrap();
        drop(tx);
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.catalog = CatalogJobState::Running(rx);
        let mut events = CatalogJobEventBuf::new();

        scheduler.poll_catalog(&mut events);
        assert_eq!(events.len(), 1);
        assert!(!scheduler.catalog_worker_running());
    }
}
