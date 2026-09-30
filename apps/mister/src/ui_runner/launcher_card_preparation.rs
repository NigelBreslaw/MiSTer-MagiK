// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! One bounded owner of card preparation, producer construction and retirement.
use super::{ASIDE_LEVELS, CardLevelSnapshot, LauncherFonts, native_render_ahead, prepare_cached};
use crate::ui_runner::launcher_card_pipeline::{CardPipelineCounters, LauncherCardRenderAhead};
use mister_magik_framebuffer_scenes::launcher::{
    LauncherFaceCache, LauncherScene, PreparedLauncher,
};
use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::JoinHandle,
};

const JOBS: usize = ASIDE_LEVELS + 2;
const RETIRED: usize = ASIDE_LEVELS + 2;

pub(super) struct PreparedContent {
    pub(super) prepared: Box<PreparedLauncher>,
    pub(super) pipeline: Option<LauncherCardRenderAhead>,
    pub(super) retirement_baseline: CardPipelineCounters,
}
struct Request {
    id: u64,
    scene: LauncherScene,
    level: CardLevelSnapshot,
    selected: usize,
    clock: String,
}
struct State {
    next_id: u64,
    interested: Vec<u64>,
    pending: VecDeque<Request>,
    ready: Vec<(u64, PreparedContent)>,
    retired: Vec<PreparedContent>,
    counters: CardPipelineCounters,
    stopped: bool,
    failure: Option<Box<dyn std::any::Any + Send>>,
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
        initial_cache: LauncherFaceCache,
    ) -> Result<Self, String> {
        Self::start(fonts, initial_level, initial_cache, |_| {})
    }
    pub(super) fn start(
        fonts: Arc<LauncherFonts>,
        initial_level: String,
        initial_cache: LauncherFaceCache,
        before_build: impl Fn(u64) + Send + 'static,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                next_id: 0,
                interested: Vec::with_capacity(JOBS),
                pending: VecDeque::with_capacity(JOBS),
                ready: Vec::with_capacity(JOBS + 1),
                retired: Vec::with_capacity(RETIRED + JOBS),
                counters: CardPipelineCounters::default(),
                stopped: false,
                failure: None,
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
                        let (request, stopped) = {
                            let mut state = worker_shared
                                .state
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            loop {
                                let cancelled = state
                                    .ready
                                    .iter()
                                    .any(|(id, _)| !state.interested.contains(id));
                                if state.stopped
                                    || !state.retired.is_empty()
                                    || !state.pending.is_empty()
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
                            let request = if state.stopped {
                                None
                            } else {
                                state.pending.pop_front()
                            };
                            (request, state.stopped)
                        };
                        let mut counters = CardPipelineCounters::default();
                        for mut content in retired.drain(..) {
                            if let Some(pipeline) = content.pipeline.as_mut() {
                                pipeline.stop();
                                counters.add_assign(
                                    pipeline.counters().delta(content.retirement_baseline),
                                );
                            }
                            drop(content);
                        }
                        worker_shared
                            .state
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .counters
                            .add_assign(counters);
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
                                    LauncherFaceCache::default(),
                                ));
                                caches.len() - 1
                            }
                        };
                        let mut cache = caches.remove(cache_index);
                        let build = |face_cache: &mut LauncherFaceCache| {
                            before_build(request.id);
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
                                cache.1 = LauncherFaceCache::default();
                                build(&mut cache.1)
                            }
                        };
                        caches.push(cache);
                        let pipeline = native_render_ahead(request.scene, &prepared);
                        let mut content = Some(PreparedContent {
                            prepared: Box::new(prepared),
                            pipeline,
                            retirement_baseline: CardPipelineCounters::default(),
                        });
                        {
                            let mut state = worker_shared
                                .state
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            if !state.stopped && state.interested.contains(&request.id) {
                                state.ready.push((request.id, content.take().unwrap()));
                            }
                        }
                        // A cancelled completion is destroyed here, including joins.
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
        };
        if visible {
            state.pending.push_front(request);
        } else {
            state.pending.push_back(request);
        }
        self.shared.wake.notify_one();
        Some(id)
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
    #[cfg(feature = "tooling")]
    pub(super) fn take_retired_counters(&self) -> CardPipelineCounters {
        std::mem::take(&mut self.lock_state().counters)
    }
    pub(super) fn shutdown(&self, contents: impl IntoIterator<Item = PreparedContent>) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.stopped = true;
        state.pending.clear();
        state.interested.clear();
        // Shutdown moves at most the current level, five aside levels and
        // a trick destination. Its extra slots were reserved at construction.
        state.retired.extend(contents);
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
        self.shutdown(std::iter::empty());
        // The worker owns all remaining pixels, faces and producer joins.
        // Dropping its handle does not wait on the launcher thread.
        drop(self.worker.take());
    }
}
