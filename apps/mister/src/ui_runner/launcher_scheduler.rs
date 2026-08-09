// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

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

enum SearchQueryJobState {
    Idle,
    Running(mpsc::Receiver<CatalogWorkerMessage>),
}

enum SystemShardJobState {
    Idle,
    Running {
        system_id: String,
        generation: Option<String>,
        receiver: mpsc::Receiver<CatalogWorkerMessage>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum SystemShardPriority {
    Prefetch,
    Selected,
    Urgent,
}

struct SystemShardRequest {
    system_id: String,
    priority: SystemShardPriority,
    reason: &'static str,
    requested_at: Instant,
    base_catalog: ArcadeCatalog,
    base_catalog_version: usize,
}

enum MediaJobState {
    Idle,
    Running(MediaWorkerHandle),
    Unavailable,
}

pub(super) struct LauncherScheduler {
    catalog: CatalogJobState,
    catalog_progress: crate::catalog_progress_report::CatalogProgressMonitor,
    search_query: SearchQueryJobState,
    pending_search_query: Option<launcher::ArcadeSearchRequest>,
    search_catalog: Arc<
        Mutex<
            Option<(
                usize,
                mister_magik_catalog::persisted_search::PersistedSearchCatalog,
            )>,
        >,
    >,
    system_shard: SystemShardJobState,
    system_shard_attempted: BTreeSet<String>,
    system_shard_queue: VecDeque<SystemShardRequest>,
    system_shard_generation: Option<String>,
    media: MediaJobState,
    launch_handoff: LaunchHandoffSession,
}

impl LauncherScheduler {
    pub(super) fn new(launch_handoff_bench_enabled: bool) -> Self {
        let now = Instant::now();
        Self {
            catalog: CatalogJobState::Idle,
            catalog_progress: crate::catalog_progress_report::CatalogProgressMonitor::new(now),
            search_query: SearchQueryJobState::Idle,
            pending_search_query: None,
            search_catalog: Arc::new(Mutex::new(None)),
            system_shard: SystemShardJobState::Idle,
            system_shard_attempted: BTreeSet::new(),
            system_shard_queue: VecDeque::new(),
            system_shard_generation: None,
            media: MediaJobState::Idle,
            launch_handoff: LaunchHandoffSession::from_env(launch_handoff_bench_enabled),
        }
    }

    pub(super) fn catalog_worker_running(&self) -> bool {
        matches!(self.catalog, CatalogJobState::Running(_))
    }

    pub(super) fn catalog_messages_running(&self) -> bool {
        self.catalog_worker_running()
            || matches!(self.search_query, SearchQueryJobState::Running(_))
            || matches!(self.system_shard, SystemShardJobState::Running { .. })
    }

    pub(super) fn system_shard_loading(&self, system_id: &str) -> bool {
        matches!(
            &self.system_shard,
            SystemShardJobState::Running {
                system_id: active,
                ..
            } if active == system_id
        )
    }

    pub(super) fn system_shard_attempted(&self, system_id: &str) -> bool {
        self.system_shard_attempted.contains(system_id)
    }

    pub(super) fn request_system_shard(
        &mut self,
        system_id: String,
        priority: SystemShardPriority,
        reason: &'static str,
        base_catalog: ArcadeCatalog,
        base_catalog_version: usize,
        now: Instant,
    ) -> bool {
        if self.system_shard_generation.is_none() {
            return false;
        }
        if self.system_shard_attempted.contains(&system_id) {
            if let Some(queued) = self
                .system_shard_queue
                .iter_mut()
                .find(|request| request.system_id == system_id)
            {
                if priority > queued.priority {
                    queued.priority = priority;
                    queued.reason = reason;
                    queued.requested_at = now;
                    queued.base_catalog = base_catalog;
                    queued.base_catalog_version = base_catalog_version;
                }
            }
            return false;
        }
        self.system_shard_attempted.insert(system_id.clone());
        self.system_shard_queue.push_back(SystemShardRequest {
            system_id,
            priority,
            reason,
            requested_at: now,
            base_catalog,
            base_catalog_version,
        });
        self.start_next_system_shard_load();
        true
    }

    pub(super) fn set_system_shard_generation(&mut self, generation: Option<&str>) -> bool {
        if self.system_shard_generation.as_deref() == generation {
            return false;
        }
        self.system_shard_generation = generation.map(str::to_string);
        self.system_shard_attempted.clear();
        self.system_shard_queue.clear();
        true
    }

    pub(super) fn retry_system_shard(
        &mut self,
        system_id: String,
        reason: &'static str,
        base_catalog: ArcadeCatalog,
        base_catalog_version: usize,
        now: Instant,
    ) -> bool {
        if self.system_shard_loading(&system_id) {
            return false;
        }
        self.system_shard_attempted.remove(&system_id);
        self.system_shard_queue
            .retain(|request| request.system_id != system_id);
        self.request_system_shard(
            system_id,
            SystemShardPriority::Urgent,
            reason,
            base_catalog,
            base_catalog_version,
            now,
        )
    }

    fn start_next_system_shard_load(&mut self) {
        if matches!(self.system_shard, SystemShardJobState::Running { .. }) {
            return;
        }
        let Some((index, _)) = self
            .system_shard_queue
            .iter()
            .enumerate()
            .max_by_key(|(_, request)| request.priority)
        else {
            return;
        };
        let request = self
            .system_shard_queue
            .remove(index)
            .expect("queued shard request");
        let system_id = request.system_id;
        let base_catalog = request.base_catalog;
        let base_catalog_version = request.base_catalog_version;
        crate::ui_logln!(
            "catalog_system_prefetch_start system={} priority={:?} reason={} queue_wait_us={}",
            system_id,
            request.priority,
            request.reason,
            request.requested_at.elapsed().as_micros()
        );
        let worker_system_id = system_id.clone();
        let (tx, rx) = mpsc::channel();
        self.system_shard = SystemShardJobState::Running {
            system_id,
            generation: self.system_shard_generation.clone(),
            receiver: rx,
        };
        if std::thread::Builder::new()
            .name("catalog-shard-load".to_string())
            .spawn(move || {
                use mister_magik_catalog::sharded_catalog::CatalogReader;
                let load_started = Instant::now();
                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::CatalogWorker,
                );
                let storage =
                    mister_magik_catalog::catalog_config::default_sharded_catalog_path();
                let result = mister_magik_catalog::lazy_sharded_reader::LazyShardedCatalogReader::open(
                    &storage,
                    mister_magik_catalog::production_sharded_projection::production_registry_limits(),
                )
                .and_then(|reader| {
                    let parsed = mister_magik_catalog::catalog_classify::SystemId::parse(
                        &worker_system_id,
                    )
                    .map_err(|error| {
                        mister_magik_catalog::sharded_catalog::CatalogError::new(
                            "open-system",
                            error.to_string(),
                        )
                    })?;
                    reader.open_system(&parsed)
                });
                let navigation_decode_us = load_started.elapsed().as_micros();
                let message = match result {
                    Ok(system) => {
                        let prepare_started = Instant::now();
                        let games = system.games();
                        let game_count = games.len();
                        let (replacement, launch_plans) =
                            arcade_rows_from_shard(&worker_system_id, games);
                        let catalog = base_catalog.replacing_system_games(
                            &worker_system_id,
                            replacement,
                            launch_plans,
                        );
                        CatalogWorkerMessage::SystemShardReady {
                            system_id: worker_system_id.clone(),
                            catalog,
                            base_catalog_version,
                            game_count,
                            prepare_us: prepare_started
                                .elapsed()
                                .as_micros()
                                .min(u64::MAX as u128) as u64,
                        }
                    }
                    Err(error) => CatalogWorkerMessage::SystemShardFailed {
                        system_id: worker_system_id.clone(),
                        error: error.to_string(),
                    },
                };
                crate::ui_logln!(
                    "catalog_system_prefetch_finish system={} status={} load_us={}",
                    worker_system_id,
                    if matches!(&message, CatalogWorkerMessage::SystemShardReady { .. }) {
                        "ready"
                    } else {
                        "failed"
                    },
                    load_started.elapsed().as_micros()
                );
                crate::ui_logln!(
                    "catalog_navigation_decode system={} decode_us={}",
                    worker_system_id,
                    navigation_decode_us
                );
                let _ = tx.send(message);
            })
            .is_err()
        {
            self.system_shard = SystemShardJobState::Idle;
            self.start_next_system_shard_load();
        }
    }

    pub(super) fn request_arcade_search(&mut self, request: launcher::ArcadeSearchRequest) {
        self.pending_search_query = Some(request);
        self.start_next_arcade_search();
    }

    fn start_next_arcade_search(&mut self) {
        if matches!(self.search_query, SearchQueryJobState::Running(_)) {
            return;
        }
        let Some(request) = self.pending_search_query.take() else {
            return;
        };
        let worker_request = request.clone();
        let search_catalog = Arc::clone(&self.search_catalog);
        let (tx, rx) = mpsc::channel();
        self.search_query = SearchQueryJobState::Running(rx);
        if std::thread::Builder::new()
            .name("catalog-search-query".to_string())
            .spawn(move || {
                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::CatalogWorker,
                );
                let storage = mister_magik_catalog::catalog_config::default_sharded_catalog_path();
                let result = cached_search_catalog(
                    &search_catalog,
                    worker_request.catalog_version,
                    &storage,
                )
                .and_then(|catalog| {
                    catalog.search(&worker_request.system_ids, &worker_request.query)
                });
                let message = match result {
                    Ok(result) => CatalogWorkerMessage::SearchQueryReady {
                        request: worker_request,
                        result,
                    },
                    Err(error) => CatalogWorkerMessage::SearchQueryFailed {
                        request: worker_request,
                        error: error.to_string(),
                    },
                };
                let _ = tx.send(message);
            })
            .is_err()
        {
            self.search_query = SearchQueryJobState::Idle;
            self.start_next_arcade_search();
        }
    }

    pub(super) fn start_catalog_worker(
        &mut self,
        root: String,
        request: CatalogWorkerRequest,
        initial_cache: CatalogWorkerInitialCache,
        execution_mode: CatalogExecutionMode,
    ) {
        self.finish_catalog_progress("replaced", "a new catalog worker replaced this worker");
        let evidence = self.catalog_progress.start(
            root.clone(),
            request.label(),
            execution_mode.label(),
            Instant::now(),
        );
        self.enqueue_catalog_progress(evidence);
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
        for _ in 0..CATALOG_MESSAGES_PER_FRAME {
            let received = match &self.catalog {
                CatalogJobState::Running(rx) => rx.try_recv(),
                CatalogJobState::Idle => break,
            };
            match received {
                Ok(message) => {
                    self.record_catalog_progress_message(&message, Instant::now());
                    out.push(message);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if disconnected {
            self.catalog = CatalogJobState::Idle;
            self.finish_catalog_progress(
                "disconnected",
                "catalog worker channel disconnected without a terminal message",
            );
        }
        let mut search_query_terminal = false;
        if let SearchQueryJobState::Running(rx) = &self.search_query {
            while out.events.len() < CATALOG_MESSAGES_PER_FRAME {
                match rx.try_recv() {
                    Ok(message) => {
                        search_query_terminal = matches!(
                            message,
                            CatalogWorkerMessage::SearchQueryReady { .. }
                                | CatalogWorkerMessage::SearchQueryFailed { .. }
                        );
                        out.push(message);
                        if search_query_terminal {
                            break;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        search_query_terminal = true;
                        break;
                    }
                }
            }
        }
        if search_query_terminal {
            self.search_query = SearchQueryJobState::Idle;
            self.start_next_arcade_search();
        }
        let mut shard_terminal = false;
        if let SystemShardJobState::Running {
            generation,
            receiver,
            ..
        } = &self.system_shard
        {
            let generation_is_current = generation == &self.system_shard_generation;
            while out.events.len() < CATALOG_MESSAGES_PER_FRAME {
                match receiver.try_recv() {
                    Ok(message) => {
                        shard_terminal = matches!(
                            message,
                            CatalogWorkerMessage::SystemShardReady { .. }
                                | CatalogWorkerMessage::SystemShardFailed { .. }
                        );
                        if generation_is_current {
                            out.push(message);
                        }
                        if shard_terminal {
                            break;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        shard_terminal = true;
                        break;
                    }
                }
            }
        }
        if shard_terminal {
            self.system_shard = SystemShardJobState::Idle;
            self.start_next_system_shard_load();
        }
        disconnected
    }

    pub(super) fn tick_catalog_progress(&mut self, background_work_allowed: bool, now: Instant) {
        if let Some(evidence) =
            self.catalog_progress
                .tick(self.catalog_worker_running(), background_work_allowed, now)
        {
            self.enqueue_catalog_progress(evidence);
        }
    }

    fn record_catalog_progress_message(&mut self, message: &CatalogWorkerMessage, now: Instant) {
        match message {
            CatalogWorkerMessage::Progress {
                title,
                detail,
                percent,
                metadata,
            } => {
                if let Some(target) = metadata
                    .as_ref()
                    .and_then(|metadata| metadata.scan_target.as_ref())
                {
                    self.catalog_progress.note_scan_target(target.clone());
                }
                self.note_catalog_progress("progress", title, detail, *percent, now);
            }
            CatalogWorkerMessage::Timing { name, detail } => {
                self.note_catalog_progress("timing", name, detail, -1, now);
            }
            CatalogWorkerMessage::FreshCleanupStarted => {
                self.note_catalog_progress("cleanup", "fresh-cleanup", "started", -1, now);
            }
            CatalogWorkerMessage::FreshCleanupCompleted { removed } => {
                self.note_catalog_progress(
                    "cleanup",
                    "fresh-cleanup",
                    &format!("completed removed={removed}"),
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::SystemDiscovered { system_id } => {
                self.note_catalog_progress(
                    "system-discovered",
                    "publishing-systems",
                    system_id,
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::ReconciliationPlanReady {
                system_ids,
                all_published_systems,
            } => {
                self.note_catalog_progress(
                    "plan-ready",
                    "reconciling-systems",
                    &format!(
                        "systems={} all_published_systems={}",
                        system_ids.len(),
                        u8::from(*all_published_systems)
                    ),
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::SystemScanning { system_id } => {
                self.note_catalog_progress(
                    "system-scanning",
                    "reconciling-systems",
                    system_id,
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::SystemPrepared {
                system_id,
                generation,
            } => {
                self.note_catalog_progress(
                    "system-prepared",
                    "publishing-systems",
                    &format!("system={system_id} generation={generation}"),
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::SystemUpdateFailed { system_id, error } => {
                self.note_catalog_progress(
                    "system-update-failed",
                    "reconciling-systems",
                    &format!("system={system_id} error={error}"),
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::ManifestPublished {
                generation,
                rebuilt,
                removed,
            } => {
                self.note_catalog_progress(
                    "manifest-published",
                    "publishing-systems",
                    &format!(
                        "generation={generation} rebuilt={} removed={}",
                        rebuilt.len(),
                        removed.len()
                    ),
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::SystemShardReady {
                system_id,
                game_count,
                prepare_us,
                ..
            } => {
                self.note_catalog_progress(
                    "system-ready",
                    "publishing-systems",
                    &format!("system={system_id} games={game_count} prepare_us={prepare_us}"),
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::SystemShardFailed { system_id, error } => {
                self.note_catalog_progress(
                    "system-failed",
                    "publishing-systems",
                    &format!("system={system_id} error={error}"),
                    -1,
                    now,
                );
            }
            CatalogWorkerMessage::Ready {
                catalog,
                durable_save_pending,
                ..
            } => {
                self.note_catalog_progress(
                    "catalog-ready",
                    "catalog-ready",
                    &format!(
                        "games={} durable_save_pending={}",
                        catalog.len(),
                        u8::from(*durable_save_pending)
                    ),
                    100,
                    now,
                );
            }
            CatalogWorkerMessage::HydrationDoneNeedsValidation { root } => {
                self.note_catalog_progress("hydration-ready", "validation-deferred", root, -1, now);
            }
            CatalogWorkerMessage::Persisted { summary, .. } => self.finish_catalog_progress_at(
                "completed",
                &format!(
                    "persisted games={} files={} entries={}",
                    summary.discoveries, summary.normal_files, summary.entries
                ),
                now,
            ),
            CatalogWorkerMessage::Unchanged { summary } => self.finish_catalog_progress_at(
                "unchanged",
                &format!(
                    "games={} files={} entries={}",
                    summary.discoveries, summary.normal_files, summary.entries
                ),
                now,
            ),
            CatalogWorkerMessage::Done => {
                self.finish_catalog_progress_at("completed", "worker completed", now);
            }
            CatalogWorkerMessage::LoadFailed { error } => {
                self.finish_catalog_progress_at("failed", error, now);
            }
            CatalogWorkerMessage::PersistenceFailed { error } => {
                self.finish_catalog_progress_at("failed", error, now);
            }
            CatalogWorkerMessage::Changed { detail, .. } => {
                self.finish_catalog_progress_at("changed", detail, now);
            }
            CatalogWorkerMessage::SearchQueryReady { .. }
            | CatalogWorkerMessage::SearchQueryFailed { .. } => {}
        }
    }

    fn note_catalog_progress(
        &mut self,
        activity_kind: &str,
        phase: &str,
        detail: &str,
        percent: i32,
        now: Instant,
    ) {
        if let Some(evidence) =
            self.catalog_progress
                .note_activity(activity_kind, phase, detail, percent, now)
        {
            self.enqueue_catalog_progress(evidence);
        }
    }

    fn finish_catalog_progress(&mut self, state: &str, detail: &str) {
        self.finish_catalog_progress_at(state, detail, Instant::now());
    }

    fn finish_catalog_progress_at(&mut self, state: &str, detail: &str, now: Instant) {
        let episode_id = self.catalog_progress.episode_id().map(str::to_string);
        if let Some(evidence) = self.catalog_progress.finish(state, detail, now)
            && let Some(episode_id) = episode_id
        {
            emit_catalog_progress(episode_id, evidence);
        }
    }

    fn enqueue_catalog_progress(
        &self,
        evidence: crate::catalog_progress_report::CatalogProgressEvidence,
    ) {
        if let Some(episode_id) = self.catalog_progress.episode_id() {
            emit_catalog_progress(episode_id.to_string(), evidence);
        }
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

    pub(super) fn recover_stale_launch_transport(&mut self, lifecycle_launch_active: bool) -> bool {
        self.launch_handoff
            .recover_stale_transport(lifecycle_launch_active)
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

fn cached_search_catalog(
    cache: &Mutex<
        Option<(
            usize,
            mister_magik_catalog::persisted_search::PersistedSearchCatalog,
        )>,
    >,
    catalog_version: usize,
    storage: &std::path::Path,
) -> Result<
    mister_magik_catalog::persisted_search::PersistedSearchCatalog,
    mister_magik_catalog::persisted_search::PersistedSearchError,
> {
    versioned_cache_value(cache, catalog_version, || {
        mister_magik_catalog::persisted_search::PersistedSearchCatalog::open(
            storage,
            mister_magik_catalog::production_sharded_projection::production_registry_limits(),
        )
    })
}

fn versioned_cache_value<T: Clone, E>(
    cache: &Mutex<Option<(usize, T)>>,
    version: usize,
    load: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((cached_version, value)) = cache.as_ref()
        && *cached_version == version
    {
        return Ok(value.clone());
    }
    let value = load()?;
    *cache = Some((version, value.clone()));
    Ok(value)
}

#[cfg(not(test))]
fn emit_catalog_progress(
    episode_id: String,
    evidence: crate::catalog_progress_report::CatalogProgressEvidence,
) {
    crate::catalog_progress_report::enqueue(episode_id, evidence);
}

#[cfg(test)]
fn emit_catalog_progress(
    _episode_id: String,
    _evidence: crate::catalog_progress_report::CatalogProgressEvidence,
) {
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_cache_reuses_and_invalidates_values() {
        let cache = Mutex::new(None);
        let first = versioned_cache_value::<_, ()>(&cache, 7, || Ok("first".to_string())).unwrap();
        let reused = versioned_cache_value::<_, ()>(&cache, 7, || {
            panic!("matching versions must not reload")
        })
        .unwrap();
        let replaced =
            versioned_cache_value::<_, ()>(&cache, 8, || Ok("second".to_string())).unwrap();

        assert_eq!(first, "first");
        assert_eq!(reused, "first");
        assert_eq!(replaced, "second");
    }

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
    fn system_shard_requests_deduplicate_and_upgrade_priority() {
        let (_tx, rx) = mpsc::channel();
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.system_shard_generation = Some("generation-a".to_string());
        scheduler.system_shard = SystemShardJobState::Running {
            system_id: "active".to_string(),
            generation: Some("generation-a".to_string()),
            receiver: rx,
        };
        let now = Instant::now();

        assert!(scheduler.request_system_shard(
            "c64".to_string(),
            SystemShardPriority::Prefetch,
            "menu",
            empty_arcade_catalog("/tmp"),
            1,
            now
        ));
        assert!(!scheduler.request_system_shard(
            "c64".to_string(),
            SystemShardPriority::Urgent,
            "open",
            empty_arcade_catalog("/tmp"),
            2,
            now
        ));

        assert_eq!(scheduler.system_shard_queue.len(), 1);
        assert_eq!(scheduler.system_shard_queue[0].system_id, "c64");
        assert_eq!(
            scheduler.system_shard_queue[0].priority,
            SystemShardPriority::Urgent
        );
        assert_eq!(scheduler.system_shard_queue[0].reason, "open");
        assert_eq!(scheduler.system_shard_queue[0].base_catalog_version, 2);
    }

    #[test]
    fn urgent_system_shard_request_ranks_ahead_of_prefetch() {
        let (_tx, rx) = mpsc::channel();
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.system_shard_generation = Some("generation-a".to_string());
        scheduler.system_shard = SystemShardJobState::Running {
            system_id: "active".to_string(),
            generation: Some("generation-a".to_string()),
            receiver: rx,
        };
        let now = Instant::now();
        assert!(scheduler.request_system_shard(
            "acornatom".to_string(),
            SystemShardPriority::Prefetch,
            "menu",
            empty_arcade_catalog("/tmp"),
            1,
            now
        ));
        assert!(scheduler.request_system_shard(
            "c64".to_string(),
            SystemShardPriority::Urgent,
            "open",
            empty_arcade_catalog("/tmp"),
            1,
            now
        ));

        let next = scheduler
            .system_shard_queue
            .iter()
            .max_by_key(|request| request.priority)
            .expect("queued request");
        assert_eq!(next.system_id, "c64");
    }

    #[test]
    fn system_shard_generation_change_clears_attempts_and_speculative_queue() {
        let (_tx, rx) = mpsc::channel();
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.system_shard_generation = Some("generation-a".to_string());
        scheduler.system_shard = SystemShardJobState::Running {
            system_id: "active".to_string(),
            generation: Some("generation-a".to_string()),
            receiver: rx,
        };
        assert!(scheduler.request_system_shard(
            "c64".to_string(),
            SystemShardPriority::Prefetch,
            "menu",
            empty_arcade_catalog("/tmp"),
            1,
            Instant::now()
        ));

        let _ = scheduler.set_system_shard_generation(Some("generation-b"));

        assert!(scheduler.system_shard_queue.is_empty());
        assert!(!scheduler.system_shard_attempted("c64"));
    }

    #[test]
    fn explicit_retry_requeues_a_failed_attempt_as_urgent() {
        let (_tx, rx) = mpsc::channel();
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.system_shard_generation = Some("generation-a".to_string());
        scheduler.system_shard = SystemShardJobState::Running {
            system_id: "active".to_string(),
            generation: Some("generation-a".to_string()),
            receiver: rx,
        };
        scheduler.system_shard_attempted.insert("c64".to_string());

        assert!(scheduler.retry_system_shard(
            "c64".to_string(),
            "explicit-retry",
            empty_arcade_catalog("/tmp"),
            1,
            Instant::now()
        ));
        assert_eq!(scheduler.system_shard_queue.len(), 1);
        assert_eq!(
            scheduler.system_shard_queue[0].priority,
            SystemShardPriority::Urgent
        );
    }

    #[test]
    fn system_shard_request_requires_authoritative_generation() {
        let mut scheduler = LauncherScheduler::new(false);

        assert!(!scheduler.request_system_shard(
            "c64".to_string(),
            SystemShardPriority::Urgent,
            "open",
            empty_arcade_catalog("/tmp"),
            1,
            Instant::now()
        ));
        assert!(scheduler.system_shard_queue.is_empty());
        assert!(!scheduler.system_shard_attempted("c64"));
    }

    #[test]
    fn stale_generation_shard_completion_is_discarded() {
        let (tx, rx) = mpsc::channel();
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.system_shard_generation = Some("generation-a".to_string());
        scheduler.system_shard = SystemShardJobState::Running {
            system_id: "c64".to_string(),
            generation: Some("generation-a".to_string()),
            receiver: rx,
        };
        let _ = scheduler.set_system_shard_generation(Some("generation-b"));
        tx.send(CatalogWorkerMessage::SystemShardFailed {
            system_id: "c64".to_string(),
            error: "stale".to_string(),
        })
        .unwrap();
        let mut events = CatalogJobEventBuf::new();

        scheduler.poll_catalog(&mut events);

        assert!(events.events.is_empty());
        assert!(!scheduler.system_shard_loading("c64"));
    }

    #[test]
    fn terminal_failure_becomes_idle_before_same_frame_retry() {
        let (tx, rx) = mpsc::channel();
        let mut scheduler = LauncherScheduler::new(false);
        scheduler.system_shard_generation = Some("generation-a".to_string());
        scheduler.system_shard_attempted.insert("c64".to_string());
        scheduler.system_shard = SystemShardJobState::Running {
            system_id: "c64".to_string(),
            generation: Some("generation-a".to_string()),
            receiver: rx,
        };
        tx.send(CatalogWorkerMessage::SystemShardFailed {
            system_id: "c64".to_string(),
            error: "temporary".to_string(),
        })
        .unwrap();
        let mut events = CatalogJobEventBuf::new();

        scheduler.poll_catalog(&mut events);

        assert!(!scheduler.system_shard_loading("c64"));
        assert!(scheduler.retry_system_shard(
            "c64".to_string(),
            "explicit-retry",
            empty_arcade_catalog("/tmp"),
            1,
            Instant::now()
        ));
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
