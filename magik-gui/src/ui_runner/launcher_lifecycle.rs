use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogSource {
    SummaryProjection,
    FullSqlite,
    FreshBuild,
}

impl CatalogSource {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::SummaryProjection => "summary-projection",
            Self::FullSqlite => "full-sqlite",
            Self::FreshBuild => "fresh-build",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LauncherLifecycleState {
    BootSplash,
    CatalogBuilding {
        foreground: bool,
        has_stale_catalog: bool,
    },
    CatalogReady {
        source: CatalogSource,
        validating: bool,
    },
    Idle,
    Launching {
        phase: LaunchingPhase,
    },
    Handoff {
        spawned_mister: bool,
    },
    Recovered {
        reason: RecoveryReason,
    },
}

impl LauncherLifecycleState {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::BootSplash => "boot-splash",
            Self::CatalogBuilding { .. } => "catalog-building",
            Self::CatalogReady { .. } => "catalog-ready",
            Self::Idle => "idle",
            Self::Launching { .. } => "launching",
            Self::Handoff { .. } => "handoff",
            Self::Recovered { .. } => "recovered",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LaunchingPhase {
    LoadingFramePending { launch_ref: String },
    HandoffPending,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RecoveryReason {
    LaunchFailed(String),
    LaunchTimedOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BridgeSyncPlan {
    None,
    Light,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LauncherInputMode {
    Normal,
    Launching,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LauncherEffect {
    StartupEvent {
        name: &'static str,
        detail: String,
    },
    BeginLoadingFrame {
        launch_ref: String,
    },
    BeginLaunchHandoff {
        launch_ref: String,
        presented_at: Instant,
    },
    PresentRecoveryFrame,
    ReturnToIdle,
}

#[derive(Debug, Default)]
pub(super) struct LifecycleEffects {
    effects: Vec<LauncherEffect>,
}

impl LifecycleEffects {
    pub(super) fn new() -> Self {
        Self {
            effects: Vec::with_capacity(8),
        }
    }

    pub(super) fn clear(&mut self) {
        self.effects.clear();
    }

    pub(super) fn push(&mut self, effect: LauncherEffect) {
        self.effects.push(effect);
    }

    pub(super) fn startup_event(&mut self, name: &'static str, detail: impl Into<String>) {
        self.push(LauncherEffect::StartupEvent {
            name,
            detail: detail.into(),
        });
    }

    pub(super) fn drain(&mut self) -> impl Iterator<Item = LauncherEffect> + '_ {
        self.effects.drain(..)
    }

    #[cfg(test)]
    fn as_slice(&self) -> &[LauncherEffect] {
        &self.effects
    }

    #[cfg(test)]
    fn capacity(&self) -> usize {
        self.effects.capacity()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LauncherLifecycleConfig {
    pub(super) catalog_worker_enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StartupCatalogState {
    Ready {
        source: CatalogSource,
        validation_scheduled: bool,
    },
    Building {
        foreground_catalog_update: bool,
        has_stale_catalog: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LauncherLifecycleInput {
    CatalogReady {
        source: CatalogSource,
        validating: bool,
    },
    CatalogBuilding {
        foreground: bool,
        has_stale_catalog: bool,
    },
    CatalogValidationStarted,
    CatalogValidationFinished,
    LaunchRequested {
        launch_ref: String,
    },
    LaunchFailed {
        message: String,
    },
    LaunchSucceeded {
        spawned_mister: bool,
    },
    LaunchTimedOut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LauncherLifecycleStep {
    pub(super) state: LauncherLifecycleState,
    pub(super) bridge_sync: BridgeSyncPlan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LauncherView {
    pub(super) state: LauncherLifecycleState,
}

pub(super) struct LauncherLifecycle {
    state: LauncherLifecycleState,
    config: LauncherLifecycleConfig,
    boot_splash_presented: bool,
}

impl LauncherLifecycle {
    pub(super) fn new(config: LauncherLifecycleConfig, _now: Instant) -> Self {
        Self {
            state: LauncherLifecycleState::BootSplash,
            config,
            boot_splash_presented: false,
        }
    }

    pub(super) fn after_boot_splash_presented(
        &mut self,
        input: StartupCatalogState,
        out: &mut LifecycleEffects,
    ) -> LauncherLifecycleStep {
        self.boot_splash_presented = true;
        match input {
            StartupCatalogState::Ready {
                source,
                validation_scheduled,
            } => {
                self.transition(
                    LauncherLifecycleState::CatalogReady {
                        source,
                        validating: validation_scheduled,
                    },
                    out,
                    "boot_splash_presented",
                );
                if !validation_scheduled {
                    self.transition(LauncherLifecycleState::Idle, out, "catalog_idle");
                }
            }
            StartupCatalogState::Building {
                foreground_catalog_update,
                has_stale_catalog,
            } => {
                self.transition(
                    LauncherLifecycleState::CatalogBuilding {
                        foreground: foreground_catalog_update || self.config.catalog_worker_enabled,
                        has_stale_catalog,
                    },
                    out,
                    "boot_splash_presented",
                );
            }
        }
        self.step(BridgeSyncPlan::Full)
    }

    pub(super) fn input_mode(&self) -> LauncherInputMode {
        match self.state {
            LauncherLifecycleState::Launching { .. } | LauncherLifecycleState::Handoff { .. } => {
                LauncherInputMode::Launching
            }
            _ => LauncherInputMode::Normal,
        }
    }

    pub(super) fn handle(
        &mut self,
        input: LauncherLifecycleInput,
        out: &mut LifecycleEffects,
    ) -> LauncherLifecycleStep {
        if !self.boot_splash_presented {
            return self.step(BridgeSyncPlan::None);
        }
        match input {
            LauncherLifecycleInput::CatalogReady { source, validating } => {
                self.transition(
                    LauncherLifecycleState::CatalogReady { source, validating },
                    out,
                    "catalog_ready",
                );
                if !validating {
                    self.transition(LauncherLifecycleState::Idle, out, "catalog_idle");
                }
            }
            LauncherLifecycleInput::CatalogBuilding {
                foreground,
                has_stale_catalog,
            } => {
                self.transition(
                    LauncherLifecycleState::CatalogBuilding {
                        foreground,
                        has_stale_catalog,
                    },
                    out,
                    "catalog_building",
                );
            }
            LauncherLifecycleInput::CatalogValidationStarted => {
                if let LauncherLifecycleState::CatalogReady { source, .. } = self.state {
                    self.transition(
                        LauncherLifecycleState::CatalogReady {
                            source,
                            validating: true,
                        },
                        out,
                        "catalog_validation_started",
                    );
                }
            }
            LauncherLifecycleInput::CatalogValidationFinished => {
                if let LauncherLifecycleState::CatalogReady { source, .. } = self.state {
                    self.transition(
                        LauncherLifecycleState::CatalogReady {
                            source,
                            validating: false,
                        },
                        out,
                        "catalog_validation_finished",
                    );
                    self.transition(LauncherLifecycleState::Idle, out, "catalog_idle");
                }
            }
            LauncherLifecycleInput::LaunchRequested { launch_ref } => {
                if matches!(self.state, LauncherLifecycleState::Idle) {
                    out.push(LauncherEffect::BeginLoadingFrame {
                        launch_ref: launch_ref.clone(),
                    });
                    self.transition(
                        LauncherLifecycleState::Launching {
                            phase: LaunchingPhase::LoadingFramePending { launch_ref },
                        },
                        out,
                        "launch_requested",
                    );
                }
            }
            LauncherLifecycleInput::LaunchFailed { message } => {
                out.push(LauncherEffect::PresentRecoveryFrame);
                self.transition(
                    LauncherLifecycleState::Recovered {
                        reason: RecoveryReason::LaunchFailed(message),
                    },
                    out,
                    "launch_failed",
                );
            }
            LauncherLifecycleInput::LaunchSucceeded { spawned_mister } => {
                self.transition(
                    LauncherLifecycleState::Handoff { spawned_mister },
                    out,
                    "launch_handoff",
                );
            }
            LauncherLifecycleInput::LaunchTimedOut => {
                out.push(LauncherEffect::PresentRecoveryFrame);
                self.transition(
                    LauncherLifecycleState::Recovered {
                        reason: RecoveryReason::LaunchTimedOut,
                    },
                    out,
                    "launch_timed_out",
                );
            }
        }
        self.step(BridgeSyncPlan::None)
    }

    pub(super) fn loading_frame_presented(&mut self, at: Instant, out: &mut LifecycleEffects) {
        let launch_ref = match &self.state {
            LauncherLifecycleState::Launching {
                phase: LaunchingPhase::LoadingFramePending { launch_ref },
            } => launch_ref.clone(),
            _ => return,
        };
        out.push(LauncherEffect::BeginLaunchHandoff {
            launch_ref,
            presented_at: at,
        });
        self.transition(
            LauncherLifecycleState::Launching {
                phase: LaunchingPhase::HandoffPending,
            },
            out,
            format!("loading_frame_presented at_ms={}", at.elapsed().as_millis()),
        );
    }

    pub(super) fn recovery_frame_presented(&mut self, _at: Instant, out: &mut LifecycleEffects) {
        if matches!(self.state, LauncherLifecycleState::Recovered { .. }) {
            out.push(LauncherEffect::ReturnToIdle);
            self.transition(
                LauncherLifecycleState::Idle,
                out,
                "recovery_frame_presented",
            );
        }
    }

    pub(super) fn state(&self) -> &LauncherLifecycleState {
        &self.state
    }

    pub(super) fn view(&self) -> LauncherView {
        LauncherView {
            state: self.state.clone(),
        }
    }

    fn step(&self, bridge_sync: BridgeSyncPlan) -> LauncherLifecycleStep {
        LauncherLifecycleStep {
            state: self.state.clone(),
            bridge_sync,
        }
    }

    fn transition(
        &mut self,
        next: LauncherLifecycleState,
        out: &mut LifecycleEffects,
        reason: impl Into<String>,
    ) {
        if self.state == next {
            return;
        }
        let previous = self.state.label();
        let next_label = next.label();
        self.state = next;
        out.startup_event(
            "launcher_lifecycle_transition",
            format!("from={previous} to={next_label} reason={}", reason.into()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lifecycle() -> LauncherLifecycle {
        LauncherLifecycle::new(
            LauncherLifecycleConfig {
                catalog_worker_enabled: true,
            },
            Instant::now(),
        )
    }

    fn idle_lifecycle() -> (LauncherLifecycle, LifecycleEffects) {
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();
        lifecycle.after_boot_splash_presented(
            StartupCatalogState::Ready {
                source: CatalogSource::FullSqlite,
                validation_scheduled: false,
            },
            &mut effects,
        );
        effects.clear();
        (lifecycle, effects)
    }

    #[test]
    fn boot_splash_does_not_emit_work_before_presented() {
        let lifecycle = lifecycle();

        assert_eq!(lifecycle.state(), &LauncherLifecycleState::BootSplash);
    }

    #[test]
    fn launch_before_boot_splash_is_rejected() {
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.handle(
            LauncherLifecycleInput::LaunchRequested {
                launch_ref: "too-early.mra".to_string(),
            },
            &mut effects,
        );

        assert_eq!(lifecycle.state(), &LauncherLifecycleState::BootSplash);
        assert!(effects.as_slice().is_empty());
    }

    #[test]
    fn warm_summary_enters_catalog_ready_with_source() {
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.after_boot_splash_presented(
            StartupCatalogState::Ready {
                source: CatalogSource::SummaryProjection,
                validation_scheduled: true,
            },
            &mut effects,
        );

        assert_eq!(
            lifecycle.state(),
            &LauncherLifecycleState::CatalogReady {
                source: CatalogSource::SummaryProjection,
                validating: true,
            }
        );
        assert_eq!(effects.capacity(), 8);
    }

    #[test]
    fn cold_missing_catalog_enters_visible_building_state() {
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.after_boot_splash_presented(
            StartupCatalogState::Building {
                foreground_catalog_update: false,
                has_stale_catalog: false,
            },
            &mut effects,
        );

        assert_eq!(
            lifecycle.state(),
            &LauncherLifecycleState::CatalogBuilding {
                foreground: true,
                has_stale_catalog: false,
            }
        );
    }

    #[test]
    fn full_catalog_ready_without_validation_becomes_idle() {
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.after_boot_splash_presented(
            StartupCatalogState::Building {
                foreground_catalog_update: false,
                has_stale_catalog: false,
            },
            &mut effects,
        );
        effects.clear();

        lifecycle.handle(
            LauncherLifecycleInput::CatalogReady {
                source: CatalogSource::FreshBuild,
                validating: false,
            },
            &mut effects,
        );

        assert_eq!(lifecycle.state(), &LauncherLifecycleState::Idle);
    }

    #[test]
    fn startup_ready_without_validation_becomes_idle() {
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.after_boot_splash_presented(
            StartupCatalogState::Ready {
                source: CatalogSource::FullSqlite,
                validation_scheduled: false,
            },
            &mut effects,
        );

        assert_eq!(lifecycle.state(), &LauncherLifecycleState::Idle);
    }

    #[test]
    fn validation_finished_returns_to_idle_before_launch() {
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.after_boot_splash_presented(
            StartupCatalogState::Ready {
                source: CatalogSource::FullSqlite,
                validation_scheduled: true,
            },
            &mut effects,
        );
        effects.clear();

        lifecycle.handle(
            LauncherLifecycleInput::CatalogValidationFinished,
            &mut effects,
        );

        assert_eq!(lifecycle.state(), &LauncherLifecycleState::Idle);

        effects.clear();
        lifecycle.handle(
            LauncherLifecycleInput::LaunchRequested {
                launch_ref: "ready.mra".to_string(),
            },
            &mut effects,
        );

        assert_eq!(
            lifecycle.state(),
            &LauncherLifecycleState::Launching {
                phase: LaunchingPhase::LoadingFramePending {
                    launch_ref: "ready.mra".to_string()
                }
            }
        );
    }

    #[test]
    fn launch_handoff_waits_for_loading_frame() {
        let (mut lifecycle, mut effects) = idle_lifecycle();

        lifecycle.handle(
            LauncherLifecycleInput::LaunchRequested {
                launch_ref: "/media/fat/_Arcade/1942.mra".to_string(),
            },
            &mut effects,
        );

        assert_eq!(
            lifecycle.state(),
            &LauncherLifecycleState::Launching {
                phase: LaunchingPhase::LoadingFramePending {
                    launch_ref: "/media/fat/_Arcade/1942.mra".to_string()
                }
            }
        );
        assert!(matches!(
            effects.as_slice().first(),
            Some(LauncherEffect::BeginLoadingFrame { .. })
        ));

        effects.clear();
        lifecycle.loading_frame_presented(Instant::now(), &mut effects);

        assert_eq!(
            lifecycle.state(),
            &LauncherLifecycleState::Launching {
                phase: LaunchingPhase::HandoffPending
            }
        );
        assert!(matches!(
            effects.as_slice().first(),
            Some(LauncherEffect::BeginLaunchHandoff {
                launch_ref,
                ..
            }) if launch_ref == "/media/fat/_Arcade/1942.mra"
        ));
    }

    #[test]
    fn launch_failure_recovers_only_after_recovery_frame_presented() {
        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::LaunchRequested {
                launch_ref: "bad.mra".to_string(),
            },
            &mut effects,
        );
        lifecycle.loading_frame_presented(Instant::now(), &mut effects);
        effects.clear();

        lifecycle.handle(
            LauncherLifecycleInput::LaunchFailed {
                message: "missing file".to_string(),
            },
            &mut effects,
        );

        assert!(matches!(
            lifecycle.state(),
            LauncherLifecycleState::Recovered {
                reason: RecoveryReason::LaunchFailed(_)
            }
        ));
        assert!(matches!(
            effects.as_slice().first(),
            Some(LauncherEffect::PresentRecoveryFrame)
        ));

        effects.clear();
        lifecycle.recovery_frame_presented(Instant::now(), &mut effects);

        assert_eq!(lifecycle.state(), &LauncherLifecycleState::Idle);
        assert!(matches!(
            effects.as_slice().first(),
            Some(LauncherEffect::ReturnToIdle)
        ));
    }
}
