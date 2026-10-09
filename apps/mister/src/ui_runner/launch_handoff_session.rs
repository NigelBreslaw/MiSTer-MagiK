// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

const PMU_CAPSULE_CONSTRUCTION: &str = "launch.return-capsule-construction";
const PMU_LAUNCH_PREPARATION: &str = "launch.preparation";

type LaunchWorkerResult = Result<bool, launcher::LaunchError>;

#[derive(Debug)]
struct PendingLaunch {
    title: String,
    rx: mpsc::Receiver<LaunchWorkerResult>,
}

#[derive(Debug)]
struct StagedLaunch {
    title: String,
    launch_target: LaunchTarget,
    return_state: Option<launcher::LaunchReturnState>,
    return_catalog: Option<return_catalog_capsule::PreparedReturnCatalogCapsule>,
    user_game: Option<mister_magik_catalog::user_state::UserGameIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LaunchHandoffRuntimeAction {
    ArcadeCoreRunning,
    TimedOut,
}

#[derive(Debug)]
pub(super) enum LaunchHandoffCompletion {
    Success,
    Failure {
        title: String,
        error: launcher::LaunchError,
    },
}

#[derive(Debug)]
struct LaunchWorkerRequest {
    launch_target: LaunchTarget,
    user_game: Option<mister_magik_catalog::user_state::UserGameIdentity>,
}

type LaunchWorkerSpawner = fn(LaunchWorkerRequest) -> mpsc::Receiver<LaunchWorkerResult>;
type ArcadeCoreProbe = fn() -> bool;

pub(super) struct LaunchHandoffSession {
    pending: Option<PendingLaunch>,
    staged: Option<StagedLaunch>,
    loading_title: String,
    launch_started: Instant,
    spawned_mister: bool,
    spawn_worker: LaunchWorkerSpawner,
    arcade_core_running: ArcadeCoreProbe,
}

impl LaunchHandoffSession {
    pub(super) fn from_env() -> Self {
        Self {
            pending: None,
            staged: None,
            loading_title: String::new(),
            launch_started: Instant::now(),
            spawned_mister: false,
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

    pub(super) fn begin_launch(
        &mut self,
        nav: &LauncherNav,
        catalog: &ArcadeCatalog,
        durable_catalog_fingerprint: Option<&str>,
        launch_ref: &str,
    ) -> bool {
        if self.pending.is_some() || self.staged.is_some() {
            return false;
        }

        let launch_target = catalog.launch_target_for_ref(launch_ref);
        let title = launcher::game_title(catalog, launch_ref);
        self.loading_title = format!("Loading {title}…");
        let user_game = catalog.user_game_identity_for_ref(launch_ref);
        let return_state = launcher::capture_launch_return_state(nav, catalog, launch_ref);
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
            launch_target,
            return_state,
            return_catalog,
            user_game,
        });
        true
    }

    pub(super) fn complete_loading_frame(&mut self) {
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
            launch_target: staged.launch_target,
            user_game: staged.user_game,
        });
        self.pending = Some(PendingLaunch {
            title: staged.title,
            rx,
        });
    }

    pub(super) fn poll_completion(
        &mut self,
        result_received: Instant,
    ) -> Option<LaunchHandoffCompletion> {
        let result = match self.pending.as_ref()?.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err(launcher::LaunchError::internal(
                "launch worker disconnected before reporting a result",
            )),
        };
        let pending = self.pending.take().expect("pending launch result");
        self.launch_started = result_received;
        match result {
            Ok(spawned) => {
                self.spawned_mister = spawned;
                Some(LaunchHandoffCompletion::Success)
            }
            Err(error) => {
                launcher::remove_launch_return_state();
                return_catalog_capsule::remove_return_catalog_capsule();
                self.spawned_mister |= error.spawned_mister();
                self.loading_title.clear();
                launcher::reset_launch();
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

    pub(super) fn finish_failure_recovery(&mut self) {
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
    fn with_worker_for_test(spawn_worker: LaunchWorkerSpawner) -> Self {
        let mut session = Self::from_env();
        session.spawn_worker = spawn_worker;
        session
    }

    #[cfg(test)]
    fn with_worker_and_core_probe_for_test(
        spawn_worker: LaunchWorkerSpawner,
        arcade_core_running: ArcadeCoreProbe,
    ) -> Self {
        let mut session = Self::with_worker_for_test(spawn_worker);
        session.arcade_core_running = arcade_core_running;
        session
    }
}

fn spawn_launch_worker(request: LaunchWorkerRequest) -> mpsc::Receiver<LaunchWorkerResult> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("launch-handoff".to_string())
        .spawn(move || {
            let prep_pmu = mister_magik_perf_events::sampled_span(PMU_LAUNCH_PREPARATION);
            let prepared = crate::launch_preparation::prepare_launch_target(&request.launch_target);
            drop(prep_pmu);
            let result = match prepared {
                Ok(launch_target) => {
                    let result = launcher::execute_game_launch(&launch_target);
                    if result.is_ok()
                        && let Some(game) = request.user_game.as_ref()
                        && let Err(error) = record_successful_launch(game)
                    {
                        crate::ui_errln!("user-state: failed to record successful launch: {error}");
                    }
                    result
                }
                Err(error) => Err(launcher::LaunchError::preparation(error)),
            };
            if result.is_err() {
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
        tx.send(Ok(false)).expect("send success result");
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
        tx.send(launcher::execute_game_launch(&LaunchTarget::Path(
            "/tmp/mister-magik-test-missing-target.mra".into(),
        )))
        .expect("send failure result");
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
        let mut session = LaunchHandoffSession::with_worker_for_test(pending_worker);
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        let catalog = one_game_catalog();

        assert!(session.begin_launch(&nav, &catalog, None, "/media/fat/_Arcade/1942.mra",));

        assert_eq!(session.loading_title(), "Loading 1942…");
        assert!(session.is_active());
        assert!(!session.has_pending_launch());
    }

    #[test]
    fn complete_loading_frame_starts_pending_handoff() {
        let _guard = lock_launch_handoff_tests();
        launcher::remove_launch_return_state();
        let mut session = LaunchHandoffSession::with_worker_for_test(pending_worker);
        let nav = LauncherNav::new();
        let catalog = one_game_catalog();

        assert!(session.begin_launch(&nav, &catalog, None, "/media/fat/_Arcade/1942.mra",));
        session.complete_loading_frame();

        assert!(session.has_pending_launch());
        assert_eq!(session.loading_title(), "Loading 1942…");
        assert!(!Path::new(launcher::LAUNCH_RETURN_STATE_PATH).exists());
    }

    #[test]
    fn disconnected_launch_worker_finishes_as_an_internal_failure() {
        let _guard = lock_launch_handoff_tests();
        launcher::remove_launch_return_state();
        let mut session = LaunchHandoffSession::with_worker_for_test(disconnected_worker);
        let nav = LauncherNav::new();
        let catalog = one_game_catalog();

        assert!(session.begin_launch(&nav, &catalog, None, "/media/fat/_Arcade/1942.mra",));
        session.complete_loading_frame();

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
        );
        let nav = LauncherNav::new();
        let catalog = one_game_catalog();

        assert!(session.begin_launch(&nav, &catalog, None, "/media/fat/_Arcade/1942.mra",));
        session.complete_loading_frame();

        assert!(matches!(
            session.poll_completion(Instant::now()),
            Some(LaunchHandoffCompletion::Success)
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
        let mut session = LaunchHandoffSession::with_worker_for_test(pending_worker);

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
        let mut session = LaunchHandoffSession::with_worker_for_test(missing_target_failure_worker);
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        let catalog = one_game_catalog();

        assert!(session.begin_launch(&nav, &catalog, None, "/media/fat/_Arcade/1942.mra",));
        session.complete_loading_frame();
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
    fn runtime_action_waits_for_core_or_timeout_after_success() {
        let _guard = lock_launch_handoff_tests();
        launcher::reset_launch();
        launcher::remove_launch_return_state();
        let mut idle_session = LaunchHandoffSession::with_worker_and_core_probe_for_test(
            success_worker,
            arcade_core_idle,
        );
        let nav = LauncherNav::new();
        let catalog = one_game_catalog();
        let start = Instant::now();

        assert!(idle_session.begin_launch(&nav, &catalog, None, "/media/fat/_Arcade/1942.mra",));
        idle_session.complete_loading_frame();
        assert!(matches!(
            idle_session.poll_completion(start),
            Some(LaunchHandoffCompletion::Success)
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
        );
        assert!(core_session.begin_launch(&nav, &catalog, None, "/media/fat/_Arcade/1942.mra",));
        core_session.complete_loading_frame();
        assert!(matches!(
            core_session.poll_completion(start),
            Some(LaunchHandoffCompletion::Success)
        ));
        assert_eq!(
            core_session.runtime_action(start + Duration::from_millis(600)),
            Some(LaunchHandoffRuntimeAction::ArcadeCoreRunning)
        );
        assert!(!Path::new(launcher::LAUNCH_RETURN_STATE_PATH).exists());
    }
}
