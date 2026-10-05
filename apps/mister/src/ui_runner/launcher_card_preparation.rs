// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! One bounded owner of card preparation and background retirement.
use super::{ASIDE_LEVELS, CardLevelSnapshot, LauncherFonts, prepare_cached};
use crate::launcher_artwork::CardFaceCache;
use mister_magik_framebuffer_scenes::launcher::{LauncherScene, PreparedLauncher};
use mister_magik_framebuffer_scenes::launcher_parallel::ParallelLauncherRenderer;
use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::JoinHandle,
};

const JOBS: usize = ASIDE_LEVELS + 2;
const RETIRED: usize = ASIDE_LEVELS + 2;

pub(super) type PreparedContent = Box<PreparedLauncher>;
struct Request {
    id: u64,
    scene: LauncherScene,
    level: CardLevelSnapshot,
    selected: usize,
    clock: String,
    retry_artwork: bool,
    foreground: bool,
    #[cfg(feature = "tooling")]
    queued_at: Option<std::time::Instant>,
}
struct State {
    next_id: u64,
    interested: Vec<u64>,
    pending: VecDeque<Request>,
    ready: Vec<(u64, PreparedContent)>,
    retired: Vec<PreparedContent>,
    retiring_renderer: Option<Box<ParallelLauncherRenderer>>,
    stopped: bool,
    background_allowed: bool,
    busy: bool,
    failure: Option<Box<dyn std::any::Any + Send>>,
    #[cfg(feature = "tooling")]
    preparation_profile: Vec<serde_json::Value>,
    #[cfg(feature = "tooling")]
    profile_overflow: usize,
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

pub(super) struct HomePreparation {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}
impl HomePreparation {
    pub(super) fn new(
        fonts: Arc<LauncherFonts>,
        initial_level: String,
        initial_cache: CardFaceCache,
    ) -> Result<Self, String> {
        Self::start(fonts, initial_level, initial_cache, |_| {})
    }
    pub(super) fn start(
        fonts: Arc<LauncherFonts>,
        initial_level: String,
        initial_cache: CardFaceCache,
        before_build: impl Fn(u64) + Send + 'static,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                next_id: 0,
                interested: Vec::with_capacity(JOBS),
                pending: VecDeque::with_capacity(JOBS),
                ready: Vec::with_capacity(JOBS + 1),
                retired: Vec::with_capacity(RETIRED + JOBS),
                retiring_renderer: None,
                stopped: false,
                background_allowed: true,
                busy: false,
                failure: None,
                #[cfg(feature = "tooling")]
                preparation_profile: Vec::new(),
                #[cfg(feature = "tooling")]
                profile_overflow: 0,
            }),
            wake: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("card-home-prepare".into())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    use mister_magik_catalog::runtime_thread::{
                        RuntimeThreadRole, apply_runtime_thread_policy,
                    };
                    apply_runtime_thread_policy(RuntimeThreadRole::SystemEntryPrepare);
                    let mut caches = vec![(initial_level, initial_cache)];
                    let mut retired = Vec::with_capacity(RETIRED + JOBS);
                    loop {
                        // Move cancellation and retirement onto this thread before
                        // picking another job. The UI never drops a ready raster.
                        let (request, stopped, renderer) = {
                            let mut state = worker_shared
                                .state
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            state.busy = false;
                            loop {
                                let cancelled = state
                                    .ready
                                    .iter()
                                    .any(|(id, _)| !state.interested.contains(id));
                                if state.stopped
                                    || !state.retired.is_empty()
                                    || state.retiring_renderer.is_some()
                                    || state.pending.iter().any(|request| request.foreground || state.background_allowed)
                                    || cancelled
                                {
                                    break;
                                }
                                state = worker_shared
                                    .wake
                                    .wait(state)
                                    .unwrap_or_else(|e| e.into_inner());
                            }
                            std::mem::swap(&mut retired, &mut state.retired);
                            let mut index = 0;
                            while index < state.ready.len() {
                                if state.stopped
                                    || !state.interested.contains(&state.ready[index].0)
                                {
                                    retired.push(state.ready.swap_remove(index).1);
                                } else {
                                    index += 1;
                                }
                            }
                            let request = if state.stopped { None } else {
                                state.pending.iter().position(|request| request.foreground || state.background_allowed)
                                    .and_then(|index| state.pending.remove(index))
                            };
                            let renderer = state.retiring_renderer.take();
                            state.busy = request.is_some() || !retired.is_empty() || renderer.is_some();
                            (request, state.stopped, renderer)
                        };
                        retired.clear();
                        drop(renderer);
                        if stopped {
                            break;
                        }
                        let Some(request) = request else {
                            continue;
                        };
                        if !worker_shared
                            .state
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .interested
                            .contains(&request.id)
                        {
                            continue;
                        }
                        let cache_index = match caches
                            .iter()
                            .position(|(menu, _)| menu == &request.level.menu_id)
                        {
                            Some(index) => index,
                            None => {
                                if caches.len() > ASIDE_LEVELS {
                                    caches.remove(0);
                                }
                                caches.push((
                                    request.level.menu_id.clone(),
                                    CardFaceCache::default(),
                                ));
                                caches.len() - 1
                            }
                        };
                        #[cfg(feature = "tooling")]
                        let profiling = mister_magik_framebuffer_scenes::launcher_profile::enabled();
                        #[cfg(feature = "tooling")]
                        let profile = profiling.then(|| {
                            let _ = mister_magik_framebuffer_scenes::launcher_profile::take();
                            (
                                std::time::Instant::now(),
                                crate::ui_runner::launcher_frame_accounting::cpu_thread_us(),
                                crate::ui_runner::launcher_frame_accounting::thread_run_delay_us(),
                            )
                        });
                        let mut cache = caches.remove(cache_index);
                        let build = |face_cache: &mut CardFaceCache| {
                            before_build(request.id);
                            if request.retry_artwork {
                                face_cache.retry_failed_artwork();
                            }
                            prepare_cached(
                                request.scene,
                                &request.level,
                                request.selected,
                                &request.clock,
                                &fonts,
                                face_cache,
                            )
                        };
                        let prepared = match catch_unwind(AssertUnwindSafe(|| build(&mut cache.1)))
                        {
                            Ok(prepared) => prepared,
                            Err(_) => {
                                // A failed build may leave partially updated faces.
                                // Retry once with cold state, still on this worker.
                                cache.1 = CardFaceCache::default();
                                build(&mut cache.1)
                            }
                        };
                        caches.push(cache);
                        #[cfg(feature = "tooling")]
                        let profile = profile.map(|(started, cpu, delay)| {
                            let cpu_end = crate::ui_runner::launcher_frame_accounting::cpu_thread_us();
                            let delay_end = crate::ui_runner::launcher_frame_accounting::thread_run_delay_us();
                            serde_json::json!({
                                "id": request.id,
                                "menu": request.level.menu_id,
                                "cards": request.level.cards.len(),
                                "queue_us": request.queued_at.map(|queued| started.saturating_duration_since(queued).as_micros() as u64),
                                "wall_us": started.elapsed().as_micros() as u64,
                                "cpu_us": cpu.zip(cpu_end).map(|(a,b)| b.saturating_sub(a)),
                                "run_delay_us": delay.zip(delay_end).map(|(a,b)| b.saturating_sub(a)),
                                "stages": mister_magik_framebuffer_scenes::launcher_profile::take(),
                            })
                        });
                        let mut content = Some(Box::new(prepared));
                        {
                            let mut state = worker_shared
                                .state
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            if !state.stopped && state.interested.contains(&request.id) {
                                state.ready.push((request.id, content.take().unwrap()));
                            }
                            #[cfg(feature = "tooling")]
                            if let Some(profile) = profile {
                                if state.preparation_profile.len() < 128 {
                                    state.preparation_profile.push(profile);
                                } else {
                                    state.profile_overflow += 1;
                                }
                            }
                        }
                        // A cancelled completion is destroyed on this worker.
                        drop(content);
                    }
                }));
                if let Err(failure) = outcome {
                    let mut state = worker_shared
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    state.stopped = true;
                    state.failure = Some(failure);
                }
            })
            .map_err(|e| format!("start card preparation worker: {e}"))?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }
    fn lock_state(&self) -> MutexGuard<'_, State> {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(failure) = state.failure.take() {
            // A second panic (or another worker failure) must reach the caller,
            // rather than leave a permanently pending ticket or accept more jobs.
            drop(state);
            resume_unwind(failure);
        }
        state
    }
    pub(super) fn request(
        &self,
        scene: LauncherScene,
        level: &CardLevelSnapshot,
        selected: usize,
        clock: &str,
        visible: bool,
    ) -> Option<u64> {
        self.enqueue(scene, level, selected, clock, visible, false)
    }
    pub(super) fn retry_artwork(
        &self,
        scene: LauncherScene,
        level: &CardLevelSnapshot,
        selected: usize,
        clock: &str,
    ) -> Option<u64> {
        self.enqueue(scene, level, selected, clock, false, true)
    }
    #[allow(clippy::too_many_arguments)]
    fn enqueue(
        &self,
        scene: LauncherScene,
        level: &CardLevelSnapshot,
        selected: usize,
        clock: &str,
        visible: bool,
        retry_artwork: bool,
    ) -> Option<u64> {
        let mut state = self.lock_state();
        if state.stopped || state.interested.len() >= JOBS {
            return None;
        }
        state.next_id = state.next_id.wrapping_add(1).max(1);
        let id = state.next_id;
        state.interested.push(id);
        let request = Request {
            id,
            scene,
            level: level.clone(),
            selected,
            clock: clock.into(),
            retry_artwork,
            foreground: visible,
            #[cfg(feature = "tooling")]
            queued_at: mister_magik_framebuffer_scenes::launcher_profile::enabled()
                .then(std::time::Instant::now),
        };
        if visible {
            state.pending.push_front(request);
        } else {
            state.pending.push_back(request);
        }
        self.shared.wake.notify_one();
        Some(id)
    }
    pub(super) fn allow_background(&self, allowed: bool) {
        let mut state = self.lock_state();
        if state.background_allowed != allowed {
            state.background_allowed = allowed;
            self.shared.wake.notify_one();
        }
    }
    pub(super) fn prioritize(&self, id: u64) {
        let mut state = self.lock_state();
        if let Some(index) = state.pending.iter().position(|request| request.id == id) {
            let mut request = state.pending.remove(index).unwrap();
            request.foreground = true;
            state.pending.push_front(request);
            self.shared.wake.notify_one();
        }
    }
    pub(super) fn quiescent(&self) -> bool {
        let state = self.lock_state();
        !state.busy
            && state.retired.is_empty()
            && state.retiring_renderer.is_none()
            && !state.pending.iter().any(|request| request.foreground)
    }

    #[cfg(feature = "tooling")]
    pub(super) fn take_preparation_profile(&self) -> serde_json::Value {
        let mut state = self.lock_state();
        serde_json::json!({
            "records": std::mem::take(&mut state.preparation_profile),
            "overflow": std::mem::take(&mut state.profile_overflow),
            "scope": "per preparation request; stage wall times are inclusive, not additive",
        })
    }
    pub(super) fn cancel(&self, id: u64) {
        let mut state = self.lock_state();
        state.interested.retain(|candidate| *candidate != id);
        state.pending.retain(|request| request.id != id);
        self.shared.wake.notify_one();
    }
    pub(super) fn take(&self, id: u64) -> Option<PreparedContent> {
        let mut state = self.lock_state();
        let index = state
            .ready
            .iter()
            .position(|(candidate, _)| *candidate == id)?;
        state.interested.retain(|candidate| *candidate != id);
        Some(state.ready.swap_remove(index).1)
    }
    pub(super) fn can_retire(&self, count: usize) -> bool {
        self.lock_state().retired.len() + count <= RETIRED
    }
    pub(super) fn retire(&self, content: PreparedContent) {
        let mut state = self.lock_state();
        assert!(
            state.retired.len() < RETIRED,
            "card retirement ownership exceeded its bound"
        );
        state.retired.push(content);
        self.shared.wake.notify_one();
    }

    pub(super) fn shutdown(
        &self,
        contents: impl IntoIterator<Item = PreparedContent>,
        renderer: Option<Box<ParallelLauncherRenderer>>,
    ) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.stopped = true;
        state.pending.clear();
        state.interested.clear();
        // Shutdown moves at most the current level, five aside levels and
        // a trick destination. Its extra slots were reserved at construction.
        state.retired.extend(contents);
        if renderer.is_some() {
            assert!(state.retiring_renderer.is_none());
            state.retiring_renderer = renderer;
        }
        assert!(state.retired.len() <= RETIRED + JOBS);
        self.shared.wake.notify_one();
    }
    #[cfg(test)]
    pub(super) fn ownership_is_bounded(&self) -> bool {
        let state = self.shared.state.lock().unwrap();
        state.interested.len() <= JOBS
            && state.pending.len() <= JOBS
            && state.ready.len() <= JOBS + 1
            && state.retired.len() <= RETIRED
    }
    #[cfg(test)]
    pub(super) fn has_failed(&self) -> bool {
        self.shared.state.lock().unwrap().failure.is_some()
    }
    #[cfg(test)]
    pub(super) fn is_ready(&self, id: u64) -> bool {
        self.shared
            .state
            .lock()
            .unwrap()
            .ready
            .iter()
            .any(|(candidate, _)| *candidate == id)
    }
}
impl Drop for HomePreparation {
    fn drop(&mut self) {
        self.shutdown(std::iter::empty(), None);
        // The worker owns all remaining pixels, faces and the renderer join.
        // Dropping its handle does not wait on the launcher thread.
        drop(self.worker.take());
    }
}
