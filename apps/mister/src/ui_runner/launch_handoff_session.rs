// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use std::io::Write;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_LAUNCH_HANDOFF_BENCH_DELAY: Duration = Duration::from_millis(750);
const PMU_CAPSULE_CONSTRUCTION: &str = "launch.return-capsule-construction";
const PMU_LAUNCH_PREPARATION: &str = "launch.preparation";

#[derive(Debug)]
struct LaunchWorkerResult {
    result: Result<bool, launcher::LaunchError>,
    bench: Option<launcher::LaunchHandoffBenchResult>,
}

#[derive(Debug)]
struct PendingLaunch {
    title: String,
    rx: mpsc::Receiver<LaunchWorkerResult>,
    action_start: Instant,
    loading_presented: Instant,
    bench_iteration: Option<usize>,
    loading_frames: u64,
    max_frame_gap_us: u64,
    last_loop_start: Option<Instant>,
}

impl PendingLaunch {
    fn record_loading_frame(&mut self, loop_start: Instant) {
        self.loading_frames = self.loading_frames.saturating_add(1);
        if let Some(previous) = self.last_loop_start {
            let gap = loop_start.saturating_duration_since(previous).as_micros() as u64;
            self.max_frame_gap_us = self.max_frame_gap_us.max(gap);
        } else {
            let gap = loop_start
                .saturating_duration_since(self.loading_presented)
                .as_micros() as u64;
            self.max_frame_gap_us = self.max_frame_gap_us.max(gap);
        }
        self.last_loop_start = Some(loop_start);
    }
}

#[derive(Debug)]
struct StagedLaunch {
    title: String,
    launch_ref: String,
    launch_target: LaunchTarget,
    action_start: Instant,
    return_state: Option<launcher::LaunchReturnState>,
    return_catalog: Option<return_catalog_capsule::PreparedReturnCatalogCapsule>,
    bench_iteration: Option<usize>,
    user_game: Option<mister_magik_catalog::user_state::UserGameIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LaunchHandoffRuntimeAction {
    ArcadeCoreRunning,
    TimedOut,
}

#[derive(Debug)]
pub(super) enum LaunchHandoffCompletion {
    Success {
        benchmark_terminal: bool,
    },
    Failure {
        title: String,
        error: launcher::LaunchError,
    },
}

#[derive(Debug)]
struct LaunchHandoffBenchConfig {
    enabled: bool,
    label: String,
    trace_path: Option<String>,
    delay: Duration,
    iterations: usize,
    launched: usize,
    mode: launcher::LaunchHandoffBenchMode,
}

impl LaunchHandoffBenchConfig {
    fn from_env(enabled: bool) -> Self {
        let mode = match std::env::var("MISTER_LAUNCH_HANDOFF_MODE")
            .unwrap_or_else(|_| "slow-fail".to_string())
            .trim()
        {
            "success" => launcher::LaunchHandoffBenchMode::Success,
            _ => launcher::LaunchHandoffBenchMode::SlowFail,
        };
        Self {
            enabled,
            label: std::env::var("MISTER_LAUNCH_HANDOFF_LABEL")
                .unwrap_or_else(|_| "launch-handoff".to_string()),
            trace_path: std::env::var("MISTER_LAUNCH_HANDOFF_TRACE")
                .ok()
                .filter(|path| !path.trim().is_empty()),
            delay: std::env::var("MISTER_LAUNCH_HANDOFF_DELAY_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_millis)
                .unwrap_or(DEFAULT_LAUNCH_HANDOFF_BENCH_DELAY),
            iterations: std::env::var("MISTER_LAUNCH_HANDOFF_ITERATIONS")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(1)
                .max(1),
            launched: 0,
            mode,
        }
    }

    fn should_request_launch(&self) -> bool {
        self.enabled && self.launched < self.iterations
    }

    fn begin_launch(&mut self) -> Option<usize> {
        if !self.enabled {
            return None;
        }
        self.launched = self.launched.saturating_add(1);
        Some(self.launched)
    }

    fn write_sample(
        &self,
        sample: LaunchHandoffBenchSample,
        recovery_presented: Instant,
        result: &'static str,
        recovery: bool,
    ) {
        let Some(iteration) = sample.iteration else {
            return;
        };
        let launch_action_to_loading_us = sample
            .loading_presented
            .saturating_duration_since(sample.action_start)
            .as_micros() as u64;
        let failure_recovery_us = recovery_presented
            .saturating_duration_since(sample.result_received)
            .as_micros() as u64;
        let handoff_complete_us = sample
            .result_received
            .saturating_duration_since(sample.action_start)
            .as_micros() as u64;
        let first_ack_us = if result == "ok" {
            sample.launch_prep_us
        } else {
            0
        };
        let line = format!(
            "launch_handoff_sample\t{}\t{}\tlaunch_action_to_loading_us={}\tmax_frame_gap_us={}\tloading_frames_before_result={}\tfailure_recovery_us={}\tlaunch_prep_us={}\thandoff_wait_us={}\tresult={result}\thandoff_complete_us={handoff_complete_us}\tfirst_ack_us={first_ack_us}\trecovery={}",
            self.label,
            iteration,
            launch_action_to_loading_us,
            sample.max_frame_gap_us,
            sample.loading_frames_before_result,
            failure_recovery_us,
            sample.launch_prep_us,
            sample.handoff_wait_us,
            u8::from(recovery),
        );
        crate::ui_logln!("{line}");
        if let Some(path) = self.trace_path.as_deref() {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(file, "{line}");
            }
        }
    }
}

#[derive(Debug)]
struct LaunchWorkerRequest {
    launch_ref: String,
    launch_target: LaunchTarget,
    bench_iteration: Option<usize>,
    bench_delay: Duration,
    bench_mode: launcher::LaunchHandoffBenchMode,
    user_game: Option<mister_magik_catalog::user_state::UserGameIdentity>,
}

type LaunchWorkerSpawner = fn(LaunchWorkerRequest) -> mpsc::Receiver<LaunchWorkerResult>;
type ArcadeCoreProbe = fn() -> bool;

#[derive(Debug)]
struct LaunchHandoffBenchSample {
    iteration: Option<usize>,
    action_start: Instant,
    loading_presented: Instant,
    max_frame_gap_us: u64,
    loading_frames_before_result: u64,
    result_received: Instant,
    launch_prep_us: u64,
    handoff_wait_us: u64,
}

pub(super) struct LaunchHandoffSession {
    pending: Option<PendingLaunch>,
    staged: Option<StagedLaunch>,
    loading_title: String,
    launch_started: Instant,
    spawned_mister: bool,
    bench: LaunchHandoffBenchConfig,
    pending_bench_sample: Option<LaunchHandoffBenchSample>,
    spawn_worker: LaunchWorkerSpawner,
    arcade_core_running: ArcadeCoreProbe,
}

impl LaunchHandoffSession {
    pub(super) fn from_env(bench_enabled: bool) -> Self {
        Self {
            pending: None,
            staged: None,
            loading_title: String::new(),
            launch_started: Instant::now(),
            spawned_mister: false,
            bench: LaunchHandoffBenchConfig::from_env(bench_enabled),
            pending_bench_sample: None,
            spawn_worker: spawn_launch_worker,
            arcade_core_running: launcher::mister_running_arcade_core,
        }
    }

    pub(super) fn loading_title(&self) -> &str {
        &self.loading_title
    }

    pub(super) fn visible_loading_title<'a>(&'a self, fallback: &'a str) -> &'a str {
        if self.loading_title.is_empty() {
            fallback
        } else {
            &self.loading_title
        }
    }

    pub(super) fn is_active(&self) -> bool {
        launcher::launch_in_progress() || !self.loading_title.is_empty()
    }

    pub(super) fn recover_stale_transport(&mut self, lifecycle_launch_active: bool) -> bool {
        if lifecycle_launch_active || self.pending.is_some() || self.staged.is_some() {
            return false;
        }
        let stale = launcher::launch_in_progress() || !self.loading_title.is_empty();
        if stale {
            launcher::reset_launch();
            self.loading_title.clear();
            self.spawned_mister = false;
        }
        stale
    }

    pub(super) fn has_pending_launch(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn benchmark_enabled(&self) -> bool {
        self.bench.enabled
    }

    pub(super) fn should_request_benchmark_launch(&self) -> bool {
        self.bench.should_request_launch()
    }

    pub(super) fn record_loading_frame(&mut self, loop_start: Instant) {
        if let Some(pending) = self.pending.as_mut() {
            pending.record_loading_frame(loop_start);
        }
    }

    pub(super) fn begin_launch(
        &mut self,
        nav: &LauncherNav,
        catalog: &ArcadeCatalog,
        durable_catalog_fingerprint: Option<&str>,
        launch_ref: &str,
        now: Instant,
    ) -> bool {
        if self.pending.is_some() || self.staged.is_some() {
            return false;
        }

        let launch_target = catalog.launch_target_for_ref(launch_ref);
        let title = launcher::game_title(catalog, launch_ref);
        self.loading_title = format!("Loading {title}…");
        let bench_iteration = self.bench.begin_launch();
        let user_game = bench_iteration
            .is_none()
            .then(|| catalog.user_game_identity_for_ref(launch_ref))
            .flatten();
        let return_state = if bench_iteration.is_some() {
            None
        } else {
            launcher::capture_launch_return_state(nav, catalog, launch_ref)
        };
        let return_catalog = return_state.as_ref().and_then(|state| {
            let durable_catalog_fingerprint = durable_catalog_fingerprint?;
            let collection_id = state.collection_id()?;
            let _pmu = mister_magik_perf_events::sampled_span(PMU_CAPSULE_CONSTRUCTION);
            match return_catalog_capsule::prepare_return_catalog_capsule(
                catalog,
                collection_id,
                state.game_path(),
                durable_catalog_fingerprint,
            ) {
                Ok(capsule) => Some(capsule),
                Err(e) => {
                    crate::ui_errln!("return catalog capsule unavailable: {e}");
                    None
                }
            }
        });
        self.staged = Some(StagedLaunch {
            title,
            launch_ref: launch_ref.to_string(),
            launch_target,
            action_start: now,
            return_state,
            return_catalog,
            bench_iteration,
            user_game,
        });
        true
    }

    pub(super) fn complete_loading_frame(&mut self, loading_presented: Instant) {
        let Some(staged) = self.staged.take() else {
            return;
        };
        let return_state_saved = staged.return_state.is_some_and(|state| {
            if let Err(e) = launcher::save_launch_return_state(&state) {
                crate::ui_errln!("failed to save launch return state: {e}");
                false
            } else {
                true
            }
        });
        if return_state_saved {
            if let Some(capsule) = staged.return_catalog {
                if let Err(e) = return_catalog_capsule::save_return_catalog_capsule(&capsule) {
                    crate::ui_errln!("failed to save return catalog capsule: {e}");
                }
            } else {
                return_catalog_capsule::remove_return_catalog_capsule();
            }
        } else {
            return_catalog_capsule::remove_return_catalog_capsule();
        }
        let rx = (self.spawn_worker)(LaunchWorkerRequest {
            launch_ref: staged.launch_ref,
            launch_target: staged.launch_target,
            bench_iteration: staged.bench_iteration,
            bench_delay: self.bench.delay,
            bench_mode: self.bench.mode,
            user_game: staged.user_game,
        });
        self.pending = Some(PendingLaunch {
            title: staged.title,
            rx,
            action_start: staged.action_start,
            loading_presented,
            bench_iteration: staged.bench_iteration,
            loading_frames: 1,
            max_frame_gap_us: 0,
            last_loop_start: None,
        });
    }

    pub(super) fn poll_completion(
        &mut self,
        result_received: Instant,
    ) -> Option<LaunchHandoffCompletion> {
        let worker_result = match self.pending.as_ref()?.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => LaunchWorkerResult {
                result: Err(launcher::LaunchError::internal(
                    "launch worker disconnected before reporting a result",
                )),
                bench: None,
            },
        };
        let pending = self.pending.take().expect("pending launch result");
        match worker_result.result {
            Ok(spawned) => {
                self.launch_started = result_received;
                self.spawned_mister = spawned;
                let mut benchmark_terminal = false;
                if let (Some(bench), Some(iteration)) =
                    (worker_result.bench.as_ref(), pending.bench_iteration)
                {
                    let sample = LaunchHandoffBenchSample {
                        iteration: Some(iteration),
                        action_start: pending.action_start,
                        loading_presented: pending.loading_presented,
                        max_frame_gap_us: pending.max_frame_gap_us,
                        loading_frames_before_result: pending.loading_frames.max(1),
                        result_received,
                        launch_prep_us: bench.prepare_us,
                        handoff_wait_us: bench.handoff_us,
                    };
                    self.bench
                        .write_sample(sample, result_received, "ok", false);
                    self.loading_title.clear();
                    launcher::reset_launch();
                    benchmark_terminal = true;
                }
                Some(LaunchHandoffCompletion::Success { benchmark_terminal })
            }
            Err(error) => {
                self.launch_started = result_received;
                if worker_result.bench.is_none() {
                    launcher::remove_launch_return_state();
                    return_catalog_capsule::remove_return_catalog_capsule();
                }
                self.spawned_mister |= error.spawned_mister();
                self.loading_title.clear();
                launcher::reset_launch();
                if let (Some(bench), Some(iteration)) =
                    (worker_result.bench.as_ref(), pending.bench_iteration)
                {
                    self.pending_bench_sample = Some(LaunchHandoffBenchSample {
                        iteration: Some(iteration),
                        action_start: pending.action_start,
                        loading_presented: pending.loading_presented,
                        max_frame_gap_us: pending.max_frame_gap_us,
                        loading_frames_before_result: pending.loading_frames.max(1),
                        result_received,
                        launch_prep_us: bench.prepare_us,
                        handoff_wait_us: bench.handoff_us,
                    });
                }
                Some(LaunchHandoffCompletion::Failure {
                    title: pending.title,
                    error,
                })
            }
        }
    }

    pub(super) fn stop_spawned_mister_for_recovery(&mut self) -> bool {
        if self.spawned_mister {
            launcher::stop_mister();
            self.spawned_mister = false;
            true
        } else {
            false
        }
    }

    pub(super) fn finish_failure_recovery(&mut self, recovery_presented: Instant) {
        if let Some(sample) = self.pending_bench_sample.take() {
            self.bench
                .write_sample(sample, recovery_presented, "error", true);
        }
        self.loading_title.clear();
    }

    pub(super) fn runtime_action(&self, now: Instant) -> Option<LaunchHandoffRuntimeAction> {
        if self.pending.is_some() || !self.is_active() {
            return None;
        }
        if (self.arcade_core_running)()
            && now.saturating_duration_since(self.launch_started) > Duration::from_millis(500)
        {
            Some(LaunchHandoffRuntimeAction::ArcadeCoreRunning)
        } else if now.saturating_duration_since(self.launch_started) > Duration::from_secs(90) {
            Some(LaunchHandoffRuntimeAction::TimedOut)
        } else {
            None
        }
    }

    #[cfg(test)]
    fn with_worker_for_test(spawn_worker: LaunchWorkerSpawner, bench_enabled: bool) -> Self {
        let mut session = Self::from_env(bench_enabled);
        session.spawn_worker = spawn_worker;
        session
    }

    #[cfg(test)]
    fn with_worker_and_core_probe_for_test(
        spawn_worker: LaunchWorkerSpawner,
        arcade_core_running: ArcadeCoreProbe,
        bench_enabled: bool,
    ) -> Self {
        let mut session = Self::with_worker_for_test(spawn_worker, bench_enabled);
        session.arcade_core_running = arcade_core_running;
        session
    }
}

fn spawn_launch_worker(request: LaunchWorkerRequest) -> mpsc::Receiver<LaunchWorkerResult> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("launch-handoff".to_string())
        .spawn(move || {
            let prep_started = Instant::now();
            let prep_pmu = mister_magik_perf_events::sampled_span(PMU_LAUNCH_PREPARATION);
            let prepared = crate::launch_preparation::prepare_launch_target(&request.launch_target);
            drop(prep_pmu);
            let prep_us = prep_started.elapsed().as_micros() as u64;
            let result = match prepared {
                Ok(launch_target) if request.bench_iteration.is_some() => {
                    let mut bench = launcher::execute_game_launch_handoff_bench(
                        &launch_target,
                        request.bench_delay,
                        request.bench_mode,
                    );
                    bench.prepare_us = bench.prepare_us.saturating_add(prep_us);
                    LaunchWorkerResult {
                        result: bench.result.clone(),
                        bench: Some(bench),
                    }
                }
                Ok(launch_target) => {
                    let result = launcher::execute_game_launch(&launch_target);
                    if result.is_ok()
                        && let Some(game) = request.user_game.as_ref()
                        && let Err(error) = record_successful_launch(game)
                    {
                        crate::ui_errln!("user-state: failed to record successful launch: {error}");
                    }
                    LaunchWorkerResult {
                        result,
                        bench: None,
                    }
                }
                Err(error) => {
                    let result = Err(launcher::LaunchError::preparation(error));
                    let bench =
                        request
                            .bench_iteration
                            .map(|_| launcher::LaunchHandoffBenchResult {
                                result: result.clone(),
                                prepare_us: prep_us,
                                handoff_us: 0,
                            });
                    LaunchWorkerResult { result, bench }
                }
            };
            if result.result.is_err() {
                crate::launch_preparation::cleanup_archive_launch_staging();
            }
            mister_magik_perf_events::submit_thread_profile("launch-handoff-worker");
            let _ = tx.send(result);
        })
        .expect("spawn launch-handoff");
    rx
}

fn record_successful_launch(
    game: &mister_magik_catalog::user_state::UserGameIdentity,
) -> Result<(), String> {
    let played_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock before Unix epoch: {error}"))?
        .as_secs();
    record_successful_launch_at(
        game,
        &mister_magik_catalog::catalog_config::default_user_state_path(),
        i64::try_from(played_at).unwrap_or(i64::MAX),
    )
}

fn record_successful_launch_at(
    game: &mister_magik_catalog::user_state::UserGameIdentity,
    path: &Path,
    played_at: i64,
) -> Result<(), String> {
    mister_magik_catalog::user_state::UserStateStore::open(path)?.record_play(game, played_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{arcade_catalog, arcade_game, arcade_system};
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn launch_handoff_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn lock_launch_handoff_tests() -> MutexGuard<'static, ()> {
        launch_handoff_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn launch_profile_phase_ownership_keeps_ui_and_worker_work_separate() {
        assert_eq!(
            PMU_CAPSULE_CONSTRUCTION,
            "launch.return-capsule-construction"
        );
        assert_eq!(PMU_LAUNCH_PREPARATION, "launch.preparation");
        assert!(PMU_CAPSULE_CONSTRUCTION.starts_with("launch.return-capsule"));
        assert!(!PMU_LAUNCH_PREPARATION.starts_with("launch.return-capsule"));
    }

    #[test]
    fn successful_launch_history_is_durable_and_unique_mru() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mister-magik-launch-history-{}-{nonce}.sqlite3",
            std::process::id()
        ));
        let game = mister_magik_catalog::user_state::UserGameIdentity {
            system_id: "snes".to_string(),
            stable_key: "snes-game".to_string(),
            title: "SNES Game".to_string(),
            launch_ref: "/games/SNES/game.sfc".to_string(),
            payload_path: "/games/SNES/game.sfc".to_string(),
        };
        record_successful_launch_at(&game, &path, 10).unwrap();
        record_successful_launch_at(&game, &path, 20).unwrap();
        let store = mister_magik_catalog::user_state::UserStateStore::open(&path).unwrap();
        let recent = store.recent_unique("snes", 16).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].play_count, 2);
        assert_eq!(recent[0].last_played_at, 20);
    }

    fn one_game_catalog() -> ArcadeCatalog {
        arcade_catalog(
            vec![
                arcade_game("1942")
                    .path("/media/fat/_Arcade/1942.mra")
                    .build(),
            ],
            vec![arcade_system("arcade", 1)],
        )
    }

    fn pending_worker(_request: LaunchWorkerRequest) -> mpsc::Receiver<LaunchWorkerResult> {
        let (_tx, rx) = mpsc::channel();
        rx
    }

    fn success_worker(_request: LaunchWorkerRequest) -> mpsc::Receiver<LaunchWorkerResult> {
        let (tx, rx) = mpsc::channel();
        tx.send(LaunchWorkerResult {
            result: Ok(false),
            bench: None,
        })
        .expect("send success result");
        rx
    }

    fn disconnected_worker(_request: LaunchWorkerRequest) -> mpsc::Receiver<LaunchWorkerResult> {
        let (_tx, rx) = mpsc::channel();
        rx
    }

    fn missing_target_failure_worker(
        _request: LaunchWorkerRequest,
    ) -> mpsc::Receiver<LaunchWorkerResult> {
        let (tx, rx) = mpsc::channel();
        tx.send(LaunchWorkerResult {
            result: launcher::execute_game_launch(&LaunchTarget::Path(
                "/tmp/mister-magik-test-missing-target.mra".into(),
            )),
            bench: None,
        })
        .expect("send failure result");
        rx
    }

    fn benchmark_failure_worker(
        request: LaunchWorkerRequest,
    ) -> mpsc::Receiver<LaunchWorkerResult> {
        let (tx, rx) = mpsc::channel();
        let bench = launcher::execute_game_launch_handoff_bench(
            &request.launch_target,
            Duration::ZERO,
            launcher::LaunchHandoffBenchMode::SlowFail,
        );
        tx.send(LaunchWorkerResult {
            result: bench.result.clone(),
            bench: Some(bench),
        })
        .expect("send benchmark failure result");
        rx
    }

    fn benchmark_success_worker(
        request: LaunchWorkerRequest,
    ) -> mpsc::Receiver<LaunchWorkerResult> {
        let (tx, rx) = mpsc::channel();
        let bench = launcher::execute_game_launch_handoff_bench(
            &request.launch_target,
            Duration::ZERO,
            launcher::LaunchHandoffBenchMode::Success,
        );
        tx.send(LaunchWorkerResult {
            result: bench.result.clone(),
            bench: Some(bench),
        })
        .expect("send benchmark success result");
        rx
    }

    fn arcade_core_running() -> bool {
        true
    }

    fn arcade_core_idle() -> bool {
        false
    }

    #[test]
    fn begin_launch_sets_loading_before_worker_handoff() {
        let mut session = LaunchHandoffSession::with_worker_for_test(pending_worker, false);
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        let catalog = one_game_catalog();

        assert!(session.begin_launch(
            &nav,
            &catalog,
            None,
            "/media/fat/_Arcade/1942.mra",
            Instant::now(),
        ));

        assert_eq!(session.loading_title(), "Loading 1942…");
        assert!(session.is_active());
        assert!(!session.has_pending_launch());
    }

    #[test]
    fn complete_loading_frame_starts_pending_handoff() {
        let _guard = lock_launch_handoff_tests();
        launcher::remove_launch_return_state();
        let mut session = LaunchHandoffSession::with_worker_for_test(pending_worker, false);
        let nav = LauncherNav::new();
        let catalog = one_game_catalog();

        assert!(session.begin_launch(
            &nav,
            &catalog,
            None,
            "/media/fat/_Arcade/1942.mra",
            Instant::now(),
        ));
        session.complete_loading_frame(Instant::now());

        assert!(session.has_pending_launch());
        assert_eq!(session.loading_title(), "Loading 1942…");
        assert!(!Path::new(launcher::LAUNCH_RETURN_STATE_PATH).exists());
    }

    #[test]
    fn disconnected_launch_worker_finishes_as_an_internal_failure() {
        let _guard = lock_launch_handoff_tests();
        launcher::remove_launch_return_state();
        let mut session = LaunchHandoffSession::with_worker_for_test(disconnected_worker, false);
        let nav = LauncherNav::new();
        let catalog = one_game_catalog();

        assert!(session.begin_launch(
            &nav,
            &catalog,
            None,
            "/media/fat/_Arcade/1942.mra",
            Instant::now(),
        ));
        session.complete_loading_frame(Instant::now());

        let completion = session
            .poll_completion(Instant::now())
            .expect("disconnected worker should complete");
        let LaunchHandoffCompletion::Failure { error, .. } = completion else {
            panic!("disconnected worker should fail");
        };
        assert_eq!(error.kind(), launcher::LaunchFailureKind::Internal);
        assert!(error.to_string().contains("worker disconnected"));
        assert!(!session.has_pending_launch());
        assert!(session.loading_title().is_empty());
    }

    #[test]
    fn successful_handoff_keeps_loading_until_main_takes_over() {
        let _guard = lock_launch_handoff_tests();
        launcher::reset_launch();
        launcher::remove_launch_return_state();
        let mut session = LaunchHandoffSession::with_worker_and_core_probe_for_test(
            success_worker,
            arcade_core_idle,
            false,
        );
        let nav = LauncherNav::new();
        let catalog = one_game_catalog();

        assert!(session.begin_launch(
            &nav,
            &catalog,
            None,
            "/media/fat/_Arcade/1942.mra",
            Instant::now(),
        ));
        session.complete_loading_frame(Instant::now());

        assert!(matches!(
            session.poll_completion(Instant::now()),
            Some(LaunchHandoffCompletion::Success {
                benchmark_terminal: false
            })
        ));
        assert_eq!(session.loading_title(), "Loading 1942…");
        assert!(session.is_active());
        assert_eq!(session.runtime_action(Instant::now()), None);
        assert!(!Path::new(launcher::LAUNCH_RETURN_STATE_PATH).exists());
    }

    #[test]
    fn idle_lifecycle_repairs_stale_launch_sent_without_pending_handoff() {
        let _guard = lock_launch_handoff_tests();
        launcher::reset_launch();
        launcher::mark_launch_sent_for_test();
        let mut session = LaunchHandoffSession::with_worker_for_test(pending_worker, false);

        assert_eq!(session.loading_title(), "");
        assert!(!session.has_pending_launch());
        assert!(session.recover_stale_transport(false));
        assert!(!launcher::launch_in_progress());
        assert!(!session.is_active());
        assert!(!session.recover_stale_transport(false));
    }

    #[test]
    fn non_bench_failure_removes_saved_return_state_and_clears_loading() {
        let _guard = lock_launch_handoff_tests();
        launcher::reset_launch();
        launcher::remove_launch_return_state();
        let mut session =
            LaunchHandoffSession::with_worker_for_test(missing_target_failure_worker, false);
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        let catalog = one_game_catalog();

        assert!(session.begin_launch(
            &nav,
            &catalog,
            None,
            "/media/fat/_Arcade/1942.mra",
            Instant::now(),
        ));
        session.complete_loading_frame(Instant::now());
        assert!(
            Path::new(launcher::LAUNCH_RETURN_STATE_PATH).exists(),
            "return state is saved after loading frame"
        );

        let completion = session.poll_completion(Instant::now());
        assert!(matches!(
            completion,
            Some(LaunchHandoffCompletion::Failure { .. })
        ));
        assert!(!Path::new(launcher::LAUNCH_RETURN_STATE_PATH).exists());
        assert_eq!(session.loading_title(), "");
        assert!(!session.is_active());
        launcher::remove_launch_return_state();
    }

    #[test]
    fn benchmark_failure_writes_stable_trace_fields() {
        let _guard = lock_launch_handoff_tests();
        launcher::reset_launch();
        launcher::remove_launch_return_state();
        let trace_path = std::env::temp_dir().join(format!(
            "mister-magik-launch-handoff-test-{}.tsv",
            std::process::id()
        ));
        let target_path = std::env::temp_dir().join(format!(
            "mister-magik-launch-handoff-test-{}.mra",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&trace_path);
        std::fs::write(&target_path, "").expect("write launch target");
        let mut session =
            LaunchHandoffSession::with_worker_for_test(benchmark_failure_worker, true);
        session.bench.label = "UNIT-HANDOFF".to_string();
        session.bench.trace_path = Some(trace_path.display().to_string());
        session.bench.delay = Duration::ZERO;
        session.bench.iterations = 1;
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        let catalog = one_game_catalog();
        let start = Instant::now();

        assert!(session.begin_launch(&nav, &catalog, None, target_path.to_str().unwrap(), start));
        session.complete_loading_frame(start + Duration::from_millis(1));
        assert!(matches!(
            session.poll_completion(start + Duration::from_millis(2)),
            Some(LaunchHandoffCompletion::Failure { .. })
        ));
        session.finish_failure_recovery(start + Duration::from_millis(3));

        let trace = std::fs::read_to_string(&trace_path).expect("read trace");
        let fields: Vec<&str> = trace.trim().split('\t').collect();
        assert_eq!(fields[0], "launch_handoff_sample");
        assert_eq!(fields[1], "UNIT-HANDOFF");
        assert_eq!(fields[2], "1");
        assert!(fields[3].starts_with("launch_action_to_loading_us="));
        assert!(fields[4].starts_with("max_frame_gap_us="));
        assert!(fields[5].starts_with("loading_frames_before_result="));
        assert!(fields[6].starts_with("failure_recovery_us="));
        assert!(fields[7].starts_with("launch_prep_us="));
        assert!(fields[8].starts_with("handoff_wait_us="));
        assert_eq!(fields[9], "result=error");
        assert!(fields[10].starts_with("handoff_complete_us="));
        assert_eq!(fields[11], "first_ack_us=0");
        assert_eq!(fields[12], "recovery=1");
        assert!(!Path::new(launcher::LAUNCH_RETURN_STATE_PATH).exists());

        let _ = std::fs::remove_file(&trace_path);
        let _ = std::fs::remove_file(&target_path);
        launcher::remove_launch_return_state();
    }

    #[test]
    fn benchmark_success_writes_terminal_trace_fields() {
        let _guard = lock_launch_handoff_tests();
        launcher::reset_launch();
        launcher::remove_launch_return_state();
        let trace_path = std::env::temp_dir().join(format!(
            "mister-magik-launch-handoff-success-test-{}.tsv",
            std::process::id()
        ));
        let target_path = std::env::temp_dir().join(format!(
            "mister-magik-launch-handoff-success-test-{}.mra",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&trace_path);
        std::fs::write(&target_path, "").expect("write launch target");
        let mut session =
            LaunchHandoffSession::with_worker_for_test(benchmark_success_worker, true);
        session.bench.label = "UNIT-HANDOFF-SUCCESS".to_string();
        session.bench.trace_path = Some(trace_path.display().to_string());
        session.bench.delay = Duration::ZERO;
        session.bench.iterations = 1;
        session.bench.mode = launcher::LaunchHandoffBenchMode::Success;
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        let catalog = one_game_catalog();
        let start = Instant::now();

        assert!(session.begin_launch(&nav, &catalog, None, target_path.to_str().unwrap(), start));
        session.complete_loading_frame(start + Duration::from_millis(1));
        assert!(matches!(
            session.poll_completion(start + Duration::from_millis(2)),
            Some(LaunchHandoffCompletion::Success {
                benchmark_terminal: true
            })
        ));

        let trace = std::fs::read_to_string(&trace_path).expect("read trace");
        let fields: Vec<&str> = trace.trim().split('\t').collect();
        assert_eq!(fields[0], "launch_handoff_sample");
        assert_eq!(fields[1], "UNIT-HANDOFF-SUCCESS");
        assert_eq!(fields[2], "1");
        assert!(fields[3].starts_with("launch_action_to_loading_us="));
        assert!(fields[4].starts_with("max_frame_gap_us="));
        assert!(fields[5].starts_with("loading_frames_before_result="));
        assert_eq!(fields[6], "failure_recovery_us=0");
        assert!(fields[7].starts_with("launch_prep_us="));
        assert!(fields[8].starts_with("handoff_wait_us="));
        assert_eq!(fields[9], "result=ok");
        assert!(fields[10].starts_with("handoff_complete_us="));
        assert!(fields[11].starts_with("first_ack_us="));
        assert_eq!(fields[12], "recovery=0");
        assert_eq!(session.loading_title(), "");
        assert!(!session.is_active());

        let _ = std::fs::remove_file(&trace_path);
        let _ = std::fs::remove_file(&target_path);
        launcher::remove_launch_return_state();
    }

    #[test]
    fn runtime_action_waits_for_core_or_timeout_after_success() {
        let _guard = lock_launch_handoff_tests();
        launcher::reset_launch();
        launcher::remove_launch_return_state();
        let mut idle_session = LaunchHandoffSession::with_worker_and_core_probe_for_test(
            success_worker,
            arcade_core_idle,
            false,
        );
        let nav = LauncherNav::new();
        let catalog = one_game_catalog();
        let start = Instant::now();

        assert!(idle_session.begin_launch(
            &nav,
            &catalog,
            None,
            "/media/fat/_Arcade/1942.mra",
            start,
        ));
        idle_session.complete_loading_frame(start);
        assert!(matches!(
            idle_session.poll_completion(start),
            Some(LaunchHandoffCompletion::Success {
                benchmark_terminal: false
            })
        ));
        assert_eq!(
            idle_session.runtime_action(start + Duration::from_millis(600)),
            None
        );
        assert_eq!(
            idle_session.runtime_action(start + Duration::from_secs(91)),
            Some(LaunchHandoffRuntimeAction::TimedOut)
        );

        let mut core_session = LaunchHandoffSession::with_worker_and_core_probe_for_test(
            success_worker,
            arcade_core_running,
            false,
        );
        assert!(core_session.begin_launch(
            &nav,
            &catalog,
            None,
            "/media/fat/_Arcade/1942.mra",
            start,
        ));
        core_session.complete_loading_frame(start);
        assert!(matches!(
            core_session.poll_completion(start),
            Some(LaunchHandoffCompletion::Success {
                benchmark_terminal: false
            })
        ));
        assert_eq!(
            core_session.runtime_action(start + Duration::from_millis(600)),
            Some(LaunchHandoffRuntimeAction::ArcadeCoreRunning)
        );
        assert!(!Path::new(launcher::LAUNCH_RETURN_STATE_PATH).exists());
    }
}
