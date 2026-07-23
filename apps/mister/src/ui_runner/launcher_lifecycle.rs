// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogSource {
    ReturnCapsule,
    ShardedRegistry,
    SummaryProjection,
    NavigationProjection,
    FullSqlite,
    FreshBuild,
}

impl CatalogSource {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ReturnCapsule => "return-capsule",
            Self::ShardedRegistry => "sharded-registry",
            Self::SummaryProjection => "summary-projection",
            Self::NavigationProjection => "navigation-projection",
            Self::FullSqlite => "full-sqlite",
            Self::FreshBuild => "fresh-build",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LauncherLifecycleState {
    BootSplash,
    CatalogBuilding {
        mode: CatalogBuildMode,
        foreground: bool,
        has_stale_catalog: bool,
    },
    CatalogLoadFailed {
        error: String,
        has_stale_catalog: bool,
        selected: CatalogRecoveryChoice,
    },
    CatalogRetrying {
        has_stale_catalog: bool,
    },
    FreshRebuilding {
        phase: FreshRebuildPhase,
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
            Self::CatalogLoadFailed { .. } => "catalog-load-failed",
            Self::CatalogRetrying { .. } => "catalog-retrying",
            Self::FreshRebuilding { .. } => "catalog-fresh-rebuilding",
            Self::CatalogReady { .. } => "catalog-ready",
            Self::Idle => "idle",
            Self::Launching { .. } => "launching",
            Self::Handoff { .. } => "handoff",
            Self::Recovered { .. } => "recovered",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogBuildMode {
    FirstBuild,
    Update,
    FreshRecovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogRecoveryChoice {
    Retry,
    Rebuild,
}

impl CatalogRecoveryChoice {
    pub(super) fn selected_index(self) -> i32 {
        match self {
            Self::Retry => 0,
            Self::Rebuild => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FreshRebuildPhase {
    AwaitingLock,
    DeletingArtifacts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LaunchingPhase {
    LoadingFramePending { launch_ref: String },
    HandoffPending,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RecoveryReason {
    LaunchFailed {
        title: String,
        kind: launcher::LaunchFailureKind,
        detail: String,
    },
    LaunchTimedOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StartupMode {
    ColdNoCatalog,
    WarmCatalog,
    ReturnFromGame,
}

impl StartupMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ColdNoCatalog => "cold_no_catalog",
            Self::WarmCatalog => "warm_catalog",
            Self::ReturnFromGame => "return_from_game",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StartupRevealState {
    SplashVisible,
    CatalogProgressVisible,
    HoldBlack,
    HoldBlackReturn,
    HydrateReturnCatalog,
    RestoreContext,
    WaitRelevantPreview,
    RevealLauncher,
    InputEnabled,
}

impl StartupRevealState {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::SplashVisible => "splash_visible",
            Self::CatalogProgressVisible => "catalog_progress_visible",
            Self::HoldBlack => "hold_black",
            Self::HoldBlackReturn => "hold_black_return",
            Self::HydrateReturnCatalog => "hydrate_return_catalog",
            Self::RestoreContext => "restore_context",
            Self::WaitRelevantPreview => "wait_relevant_preview",
            Self::RevealLauncher => "reveal_launcher",
            Self::InputEnabled => "input_enabled",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StartupRevealStatus {
    pub(super) mode: StartupMode,
    pub(super) state: StartupRevealState,
    pub(super) revealed: bool,
    pub(super) input_enabled: bool,
    pub(super) reveal_ms: u64,
    pub(super) input_enabled_ms: u64,
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
    StartCatalogRetry {
        root: String,
    },
    StartFreshCatalogBuild {
        root: String,
    },
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LauncherLifecycleConfig {
    pub(super) catalog_worker_enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum StartupCatalogState {
    Ready {
        source: CatalogSource,
        validation_scheduled: bool,
    },
    Building {
        mode: CatalogBuildMode,
        foreground_catalog_update: bool,
        has_stale_catalog: bool,
    },
    LoadFailed {
        error: String,
        has_stale_catalog: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum LauncherLifecycleInput {
    StartupRevealReady {
        preview_state: &'static str,
    },
    StartupReturnCatalogHydrationNeeded,
    StartupReturnContextRestored {
        screen: &'static str,
        system_id: String,
        filter: String,
        game_path: String,
        game_index: usize,
        visual_index: f32,
        preview_expected: bool,
    },
    StartupReturnPreviewReady {
        preview_state: &'static str,
    },
    CatalogReady {
        source: CatalogSource,
        validating: bool,
    },
    CatalogBuilding {
        mode: CatalogBuildMode,
        foreground: bool,
        has_stale_catalog: bool,
    },
    CatalogLoadFailed {
        error: String,
        has_stale_catalog: bool,
    },
    CatalogRecoveryLeft,
    CatalogRecoveryRight,
    CatalogRecoveryConfirm,
    CatalogFreshCleanupStarted,
    CatalogFreshCleanupCompleted,
    CatalogValidationStarted,
    CatalogValidationFinished,
    LaunchRequested {
        launch_ref: String,
    },
    LaunchFailed {
        title: String,
        kind: launcher::LaunchFailureKind,
        detail: String,
    },
    LaunchFailureAcknowledge,
    LaunchSucceeded {
        spawned_mister: bool,
    },
    BenchmarkLaunchCompleted,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CatalogRecoveryDialog {
    pub(super) error: String,
    pub(super) selected: CatalogRecoveryChoice,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LaunchFailureDialog {
    pub(super) title: String,
    pub(super) message: &'static str,
}

impl LauncherView {
    pub(super) fn catalog_recovery_dialog(&self) -> Option<CatalogRecoveryDialog> {
        match &self.state {
            LauncherLifecycleState::CatalogLoadFailed {
                error, selected, ..
            } => Some(CatalogRecoveryDialog {
                error: error.clone(),
                selected: *selected,
            }),
            _ => None,
        }
    }

    pub(super) fn launch_failure_dialog(&self) -> Option<LaunchFailureDialog> {
        let (title, kind) = match &self.state {
            LauncherLifecycleState::Recovered {
                reason: RecoveryReason::LaunchFailed { title, kind, .. },
            } => (title.as_str(), *kind),
            LauncherLifecycleState::Recovered {
                reason: RecoveryReason::LaunchTimedOut,
            } => ("This game", launcher::LaunchFailureKind::HandoffRejected),
            _ => return None,
        };
        let message = match kind {
            launcher::LaunchFailureKind::UnreadablePayload => {
                "The game file could not be read. Check that the storage is connected and the file still exists."
            }
            launcher::LaunchFailureKind::DamagedArchive => {
                "The archive is damaged or unsupported. Replace it with a valid ZIP file and try again."
            }
            launcher::LaunchFailureKind::MissingCore => {
                "The required core is not installed. Update or reinstall the system core, then try again."
            }
            launcher::LaunchFailureKind::HandoffRejected => {
                "MiSTer did not accept the launch request. Return to the list and try again."
            }
            launcher::LaunchFailureKind::Internal => {
                "The launch could not be prepared. Return to the list and choose the game again."
            }
        };
        Some(LaunchFailureDialog {
            title: format!("Couldn't launch {title}"),
            message,
        })
    }
}

pub(super) struct LauncherLifecycle {
    state: LauncherLifecycleState,
    config: LauncherLifecycleConfig,
    boot_splash_presented: bool,
    startup_mode: StartupMode,
    startup_reveal_state: StartupRevealState,
    startup_started_at: Instant,
    startup_revealed_at: Option<Instant>,
    startup_input_enabled_at: Option<Instant>,
    catalog_root: String,
}

impl LauncherLifecycle {
    pub(super) const COLD_SPLASH_DURATION: Duration = Duration::from_secs(2);
    pub(super) const RETURN_PREVIEW_HOLD_TIMEOUT: Duration = Duration::from_millis(250);
    pub(super) const RETURN_BLACK_SCREEN_TIMEOUT: Duration = Duration::from_secs(5);

    pub(super) fn new(config: LauncherLifecycleConfig, now: Instant) -> Self {
        Self {
            state: LauncherLifecycleState::BootSplash,
            config,
            boot_splash_presented: false,
            startup_mode: StartupMode::WarmCatalog,
            startup_reveal_state: StartupRevealState::HoldBlack,
            startup_started_at: now,
            startup_revealed_at: None,
            startup_input_enabled_at: None,
            catalog_root: arcade_catalog::DEFAULT_ARCADE_ROOT.to_string(),
        }
    }

    pub(super) fn set_catalog_root(&mut self, root: String) {
        self.catalog_root = root;
    }

    pub(super) fn begin_startup_reveal(
        &mut self,
        mode: StartupMode,
        now: Instant,
        out: &mut LifecycleEffects,
    ) {
        self.startup_mode = mode;
        self.startup_started_at = now;
        self.startup_revealed_at = None;
        self.startup_input_enabled_at = None;
        self.startup_reveal_state = match mode {
            StartupMode::ColdNoCatalog => StartupRevealState::SplashVisible,
            StartupMode::WarmCatalog => StartupRevealState::HoldBlack,
            StartupMode::ReturnFromGame => StartupRevealState::HoldBlackReturn,
        };
        out.startup_event("startup_entry_classified", format!("mode={}", mode.label()));
        match self.startup_reveal_state {
            StartupRevealState::SplashVisible => {
                out.startup_event("startup_splash_visible", "mode=cold_no_catalog");
            }
            StartupRevealState::HoldBlack => {
                out.startup_event("startup_hold_black", "mode=warm_catalog");
            }
            StartupRevealState::HoldBlackReturn => {
                out.startup_event("startup_hold_black", "mode=return_from_game");
            }
            _ => {}
        }
    }

    pub(super) fn tick_startup_reveal(
        &mut self,
        now: Instant,
        catalog_ready: bool,
        out: &mut LifecycleEffects,
    ) {
        if self.startup_input_enabled_at.is_some() {
            return;
        }
        let startup_elapsed = now.saturating_duration_since(self.startup_started_at);
        if self.startup_mode == StartupMode::ReturnFromGame
            && !self.startup_can_present_frame()
            && startup_elapsed >= Self::RETURN_BLACK_SCREEN_TIMEOUT
        {
            let state = self.startup_reveal_state.label();
            out.startup_event(
                "return_black_screen_timeout",
                format!("elapsed_ms={} state={state}", startup_elapsed.as_millis()),
            );
            self.mark_reveal_ready("preview_state=return_black_screen_timeout", out);
            return;
        }
        match self.startup_reveal_state {
            StartupRevealState::SplashVisible if startup_elapsed >= Self::COLD_SPLASH_DURATION => {
                self.startup_reveal_state = StartupRevealState::CatalogProgressVisible;
                out.startup_event(
                    "startup_splash_done",
                    format!("elapsed_ms={}", startup_elapsed.as_millis()),
                );
                out.startup_event("catalog_progress_revealed", "mode=cold_no_catalog");
            }
            StartupRevealState::CatalogProgressVisible if catalog_ready => {
                self.mark_reveal_ready("preview_state=not_required", out);
            }
            StartupRevealState::HoldBlack if catalog_ready => {
                self.mark_reveal_ready("preview_state=not_required", out);
            }
            StartupRevealState::WaitRelevantPreview
                if startup_elapsed >= Self::RETURN_PREVIEW_HOLD_TIMEOUT =>
            {
                self.mark_reveal_ready("preview_state=timeout", out);
            }
            _ => {}
        }
    }

    pub(super) fn startup_should_show_splash(&self) -> bool {
        self.startup_reveal_state == StartupRevealState::SplashVisible
    }

    pub(super) fn startup_can_present_frame(&self) -> bool {
        matches!(
            self.startup_reveal_state,
            StartupRevealState::SplashVisible
                | StartupRevealState::CatalogProgressVisible
                | StartupRevealState::RevealLauncher
                | StartupRevealState::InputEnabled
        )
    }

    pub(super) fn startup_input_enabled(&self) -> bool {
        self.startup_input_enabled_at.is_some()
    }

    pub(super) fn startup_waiting_for_return_catalog(&self) -> bool {
        self.startup_mode == StartupMode::ReturnFromGame
            && self.startup_reveal_state == StartupRevealState::HydrateReturnCatalog
    }

    pub(super) fn catalog_worker_start_delay(&self, default_delay: Duration) -> Duration {
        if self.startup_waiting_for_return_catalog() {
            Duration::ZERO
        } else {
            default_delay
        }
    }

    pub(super) fn startup_status(&self) -> StartupRevealStatus {
        StartupRevealStatus {
            mode: self.startup_mode,
            state: self.startup_reveal_state,
            revealed: self.startup_revealed_at.is_some(),
            input_enabled: self.startup_input_enabled_at.is_some(),
            reveal_ms: self
                .startup_revealed_at
                .map(|at| {
                    at.saturating_duration_since(self.startup_started_at)
                        .as_millis() as u64
                })
                .unwrap_or(0),
            input_enabled_ms: self
                .startup_input_enabled_at
                .map(|at| {
                    at.saturating_duration_since(self.startup_started_at)
                        .as_millis() as u64
                })
                .unwrap_or(0),
        }
    }

    pub(super) fn note_startup_frame_presented(
        &mut self,
        frame: u64,
        now: Instant,
        out: &mut LifecycleEffects,
    ) {
        if self.startup_reveal_state != StartupRevealState::RevealLauncher {
            return;
        }
        if self.startup_revealed_at.is_none() {
            self.startup_revealed_at = Some(now);
            out.startup_event(
                "launcher_revealed",
                format!("mode={} frame={frame}", self.startup_mode.label()),
            );
        }
        if self.startup_input_enabled_at.is_none() {
            self.startup_reveal_state = StartupRevealState::InputEnabled;
            self.startup_input_enabled_at = Some(now);
            out.startup_event(
                "launcher_input_enabled",
                format!("mode={} frame={frame}", self.startup_mode.label()),
            );
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
                mode,
                foreground_catalog_update,
                has_stale_catalog,
            } => {
                self.transition(
                    LauncherLifecycleState::CatalogBuilding {
                        mode,
                        foreground: foreground_catalog_update || self.config.catalog_worker_enabled,
                        has_stale_catalog,
                    },
                    out,
                    "boot_splash_presented",
                );
            }
            StartupCatalogState::LoadFailed {
                error,
                has_stale_catalog,
            } => {
                self.transition(
                    LauncherLifecycleState::CatalogLoadFailed {
                        error,
                        has_stale_catalog,
                        selected: CatalogRecoveryChoice::Retry,
                    },
                    out,
                    "catalog_load_failed",
                );
                self.mark_reveal_ready("catalog_load_failed", out);
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
        match &input {
            LauncherLifecycleInput::StartupRevealReady { preview_state } => {
                self.mark_reveal_ready(&format!("preview_state={preview_state}"), out);
                return self.step(BridgeSyncPlan::None);
            }
            LauncherLifecycleInput::StartupReturnCatalogHydrationNeeded => {
                if self.startup_mode == StartupMode::ReturnFromGame
                    && self.startup_reveal_state == StartupRevealState::HoldBlackReturn
                {
                    self.startup_reveal_state = StartupRevealState::HydrateReturnCatalog;
                    out.startup_event("return_catalog_hydration_needed", "mode=return_from_game");
                }
                return self.step(BridgeSyncPlan::None);
            }
            LauncherLifecycleInput::StartupReturnContextRestored {
                screen,
                system_id,
                filter,
                game_path,
                game_index,
                visual_index,
                preview_expected,
            } => {
                self.startup_reveal_state = StartupRevealState::RestoreContext;
                out.startup_event("startup_restore_context", "mode=return_from_game");
                self.startup_reveal_state = StartupRevealState::WaitRelevantPreview;
                out.startup_event(
                    "return_context_restored",
                    format!(
                        "screen={screen} system_id={system_id} filter={filter} game_path={game_path} game_index={game_index} visual_index={visual_index:.3} preview_expected={preview_expected}"
                    ),
                );
                return self.step(BridgeSyncPlan::None);
            }
            LauncherLifecycleInput::StartupReturnPreviewReady { preview_state } => {
                out.startup_event(
                    "return_preview_ready",
                    format!("preview_state={preview_state}"),
                );
                self.mark_reveal_ready(&format!("preview_state={preview_state}"), out);
                return self.step(BridgeSyncPlan::None);
            }
            _ => {}
        }
        if !self.boot_splash_presented {
            return self.step(BridgeSyncPlan::None);
        }
        match input {
            LauncherLifecycleInput::CatalogReady { source, validating } => {
                if matches!(
                    self.state,
                    LauncherLifecycleState::CatalogBuilding { .. }
                        | LauncherLifecycleState::CatalogRetrying { .. }
                        | LauncherLifecycleState::CatalogReady { .. }
                        | LauncherLifecycleState::Idle
                ) {
                    self.transition(
                        LauncherLifecycleState::CatalogReady { source, validating },
                        out,
                        "catalog_ready",
                    );
                    if !validating {
                        self.transition(LauncherLifecycleState::Idle, out, "catalog_idle");
                    }
                }
            }
            LauncherLifecycleInput::CatalogBuilding {
                mode,
                foreground,
                has_stale_catalog,
            } => {
                if matches!(
                    self.state,
                    LauncherLifecycleState::CatalogBuilding { .. }
                        | LauncherLifecycleState::CatalogReady { .. }
                        | LauncherLifecycleState::Idle
                ) {
                    self.transition(
                        LauncherLifecycleState::CatalogBuilding {
                            mode,
                            foreground,
                            has_stale_catalog,
                        },
                        out,
                        "catalog_building",
                    );
                }
            }
            LauncherLifecycleInput::CatalogLoadFailed {
                error,
                has_stale_catalog,
            } => {
                if matches!(
                    self.state,
                    LauncherLifecycleState::CatalogBuilding { .. }
                        | LauncherLifecycleState::CatalogRetrying { .. }
                        | LauncherLifecycleState::FreshRebuilding { .. }
                        | LauncherLifecycleState::CatalogReady { .. }
                        | LauncherLifecycleState::Idle
                ) {
                    self.transition(
                        LauncherLifecycleState::CatalogLoadFailed {
                            error,
                            has_stale_catalog,
                            selected: CatalogRecoveryChoice::Retry,
                        },
                        out,
                        "catalog_load_failed",
                    );
                    self.mark_reveal_ready("catalog_load_failed", out);
                }
            }
            LauncherLifecycleInput::CatalogRecoveryLeft => {
                if let LauncherLifecycleState::CatalogLoadFailed {
                    error,
                    has_stale_catalog,
                    ..
                } = &self.state
                {
                    self.transition(
                        LauncherLifecycleState::CatalogLoadFailed {
                            error: error.clone(),
                            has_stale_catalog: *has_stale_catalog,
                            selected: CatalogRecoveryChoice::Retry,
                        },
                        out,
                        "catalog_recovery_left",
                    );
                }
            }
            LauncherLifecycleInput::CatalogRecoveryRight => {
                if let LauncherLifecycleState::CatalogLoadFailed {
                    error,
                    has_stale_catalog,
                    ..
                } = &self.state
                {
                    self.transition(
                        LauncherLifecycleState::CatalogLoadFailed {
                            error: error.clone(),
                            has_stale_catalog: *has_stale_catalog,
                            selected: CatalogRecoveryChoice::Rebuild,
                        },
                        out,
                        "catalog_recovery_right",
                    );
                }
            }
            LauncherLifecycleInput::CatalogRecoveryConfirm => {
                if let LauncherLifecycleState::CatalogLoadFailed {
                    has_stale_catalog,
                    selected,
                    ..
                } = self.state.clone()
                {
                    match selected {
                        CatalogRecoveryChoice::Retry => {
                            self.transition(
                                LauncherLifecycleState::CatalogRetrying { has_stale_catalog },
                                out,
                                "catalog_retry_requested",
                            );
                            out.push(LauncherEffect::StartCatalogRetry {
                                root: self.catalog_root.clone(),
                            });
                        }
                        CatalogRecoveryChoice::Rebuild => {
                            self.transition(
                                LauncherLifecycleState::FreshRebuilding {
                                    phase: FreshRebuildPhase::AwaitingLock,
                                    has_stale_catalog,
                                },
                                out,
                                "catalog_fresh_rebuild_requested",
                            );
                            out.push(LauncherEffect::StartFreshCatalogBuild {
                                root: self.catalog_root.clone(),
                            });
                        }
                    }
                }
            }
            LauncherLifecycleInput::CatalogFreshCleanupStarted => {
                if let LauncherLifecycleState::FreshRebuilding {
                    phase: FreshRebuildPhase::AwaitingLock,
                    has_stale_catalog,
                } = self.state
                {
                    self.transition(
                        LauncherLifecycleState::FreshRebuilding {
                            phase: FreshRebuildPhase::DeletingArtifacts,
                            has_stale_catalog,
                        },
                        out,
                        "catalog_fresh_cleanup_started",
                    );
                }
            }
            LauncherLifecycleInput::CatalogFreshCleanupCompleted => {
                if let LauncherLifecycleState::FreshRebuilding {
                    phase: FreshRebuildPhase::DeletingArtifacts,
                    has_stale_catalog,
                } = self.state
                {
                    self.transition(
                        LauncherLifecycleState::CatalogBuilding {
                            mode: CatalogBuildMode::FreshRecovery,
                            foreground: true,
                            has_stale_catalog,
                        },
                        out,
                        "catalog_fresh_cleanup_completed",
                    );
                }
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
                if matches!(self.state, LauncherLifecycleState::Idle)
                    && self.startup_input_enabled()
                {
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
            LauncherLifecycleInput::LaunchFailed {
                title,
                kind,
                detail,
            } => {
                if matches!(self.state, LauncherLifecycleState::Launching { .. }) {
                    out.push(LauncherEffect::PresentRecoveryFrame);
                    self.transition(
                        LauncherLifecycleState::Recovered {
                            reason: RecoveryReason::LaunchFailed {
                                title,
                                kind,
                                detail,
                            },
                        },
                        out,
                        "launch_failed",
                    );
                }
            }
            LauncherLifecycleInput::LaunchFailureAcknowledge => {
                if matches!(self.state, LauncherLifecycleState::Recovered { .. }) {
                    out.push(LauncherEffect::ReturnToIdle);
                    self.transition(
                        LauncherLifecycleState::Idle,
                        out,
                        "launch_failure_acknowledged",
                    );
                }
            }
            LauncherLifecycleInput::LaunchSucceeded { spawned_mister } => {
                if matches!(self.state, LauncherLifecycleState::Launching { .. }) {
                    self.transition(
                        LauncherLifecycleState::Handoff { spawned_mister },
                        out,
                        "launch_handoff",
                    );
                }
            }
            LauncherLifecycleInput::BenchmarkLaunchCompleted => {
                if matches!(
                    self.state,
                    LauncherLifecycleState::Launching { .. }
                        | LauncherLifecycleState::Handoff { .. }
                ) {
                    out.push(LauncherEffect::ReturnToIdle);
                    self.transition(
                        LauncherLifecycleState::Idle,
                        out,
                        "benchmark_launch_completed",
                    );
                }
            }
            LauncherLifecycleInput::LaunchTimedOut => {
                if matches!(self.state, LauncherLifecycleState::Launching { .. }) {
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
            LauncherLifecycleInput::StartupRevealReady { .. }
            | LauncherLifecycleInput::StartupReturnCatalogHydrationNeeded
            | LauncherLifecycleInput::StartupReturnContextRestored { .. }
            | LauncherLifecycleInput::StartupReturnPreviewReady { .. } => {}
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

    pub(super) fn recovery_frame_presented(&mut self, _at: Instant, _out: &mut LifecycleEffects) {}

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

    fn mark_reveal_ready(&mut self, detail: &str, out: &mut LifecycleEffects) {
        if matches!(
            self.startup_reveal_state,
            StartupRevealState::RevealLauncher | StartupRevealState::InputEnabled
        ) {
            return;
        }
        self.startup_reveal_state = StartupRevealState::RevealLauncher;
        out.startup_event(
            "launcher_reveal_ready",
            format!("mode={} {detail}", self.startup_mode.label()),
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
        let now = Instant::now();
        lifecycle.begin_startup_reveal(StartupMode::WarmCatalog, now, &mut effects);
        lifecycle.tick_startup_reveal(now, true, &mut effects);
        lifecycle.note_startup_frame_presented(0, now, &mut effects);
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

    fn effect_names(effects: &LifecycleEffects) -> Vec<&'static str> {
        effects
            .as_slice()
            .iter()
            .filter_map(|effect| match effect {
                LauncherEffect::StartupEvent { name, .. } => Some(*name),
                _ => None,
            })
            .collect()
    }

    fn effect_detail<'a>(effects: &'a LifecycleEffects, expected_name: &str) -> Option<&'a str> {
        effects.as_slice().iter().find_map(|effect| match effect {
            LauncherEffect::StartupEvent { name, detail } if *name == expected_name => {
                Some(detail.as_str())
            }
            _ => None,
        })
    }

    fn assert_input_ignored(
        lifecycle: &mut LauncherLifecycle,
        effects: &mut LifecycleEffects,
        input: LauncherLifecycleInput,
    ) {
        let before = lifecycle.state().clone();
        effects.clear();

        lifecycle.handle(input, effects);

        assert_eq!(lifecycle.state(), &before);
        assert!(effects.as_slice().is_empty());
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
    fn cold_start_shows_splash_for_two_seconds_before_progress() {
        let now = Instant::now();
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.begin_startup_reveal(StartupMode::ColdNoCatalog, now, &mut effects);
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::SplashVisible
        );
        assert!(lifecycle.startup_should_show_splash());
        assert!(lifecycle.startup_can_present_frame());
        assert!(!lifecycle.startup_input_enabled());
        assert!(effect_names(&effects).contains(&"startup_splash_visible"));
        effects.clear();

        lifecycle.tick_startup_reveal(
            now + LauncherLifecycle::COLD_SPLASH_DURATION - Duration::from_millis(1),
            false,
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::SplashVisible
        );
        assert!(effects.as_slice().is_empty());

        lifecycle.tick_startup_reveal(
            now + LauncherLifecycle::COLD_SPLASH_DURATION,
            false,
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::CatalogProgressVisible
        );
        assert!(!lifecycle.startup_should_show_splash());
        assert!(effect_names(&effects).contains(&"startup_splash_done"));
        assert!(effect_names(&effects).contains(&"catalog_progress_revealed"));
    }

    #[test]
    fn warm_start_holds_black_until_reveal_and_input_enable() {
        let now = Instant::now();
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.begin_startup_reveal(StartupMode::WarmCatalog, now, &mut effects);
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::HoldBlack
        );
        assert!(!lifecycle.startup_can_present_frame());
        effects.clear();

        lifecycle.tick_startup_reveal(now + Duration::from_millis(10), true, &mut effects);
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::RevealLauncher
        );
        assert!(lifecycle.startup_can_present_frame());
        assert!(effect_names(&effects).contains(&"launcher_reveal_ready"));
        effects.clear();

        lifecycle.note_startup_frame_presented(0, now + Duration::from_millis(37), &mut effects);
        let status = lifecycle.startup_status();
        assert_eq!(status.state, StartupRevealState::InputEnabled);
        assert!(status.revealed);
        assert!(status.input_enabled);
        assert_eq!(status.reveal_ms, 37);
        assert_eq!(status.input_enabled_ms, 37);
        assert!(effect_names(&effects).contains(&"launcher_revealed"));
        assert!(effect_names(&effects).contains(&"launcher_input_enabled"));
    }

    #[test]
    fn return_start_waits_for_restored_context_and_preview() {
        let now = Instant::now();
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.begin_startup_reveal(StartupMode::ReturnFromGame, now, &mut effects);
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::HoldBlackReturn
        );
        assert!(!lifecycle.startup_can_present_frame());
        effects.clear();

        lifecycle.handle(
            LauncherLifecycleInput::StartupReturnCatalogHydrationNeeded,
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::HydrateReturnCatalog
        );
        assert!(lifecycle.startup_waiting_for_return_catalog());
        assert_eq!(
            lifecycle.catalog_worker_start_delay(Duration::from_secs(2)),
            Duration::ZERO
        );
        assert!(!lifecycle.startup_can_present_frame());
        assert!(effect_names(&effects).contains(&"return_catalog_hydration_needed"));
        effects.clear();

        lifecycle.handle(
            LauncherLifecycleInput::StartupReturnContextRestored {
                screen: "arcade",
                system_id: "arcade".to_string(),
                filter: "all".to_string(),
                game_path: "/media/fat/_Arcade/Air Gallet (Europe).mra".to_string(),
                game_index: 17,
                visual_index: 17.0,
                preview_expected: true,
            },
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::WaitRelevantPreview
        );
        assert!(!lifecycle.startup_can_present_frame());
        assert!(effect_names(&effects).contains(&"return_context_restored"));
        assert_eq!(
            effect_detail(&effects, "return_context_restored"),
            Some(
                "screen=arcade system_id=arcade filter=all game_path=/media/fat/_Arcade/Air Gallet (Europe).mra game_index=17 visual_index=17.000 preview_expected=true"
            )
        );
        effects.clear();

        lifecycle.handle(
            LauncherLifecycleInput::StartupReturnPreviewReady {
                preview_state: "exact",
            },
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::RevealLauncher
        );
        assert!(lifecycle.startup_can_present_frame());
        assert!(effect_names(&effects).contains(&"return_preview_ready"));
        assert!(effect_names(&effects).contains(&"launcher_reveal_ready"));
    }

    #[test]
    fn return_start_reveals_if_preview_never_becomes_ready() {
        let now = Instant::now();
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.begin_startup_reveal(StartupMode::ReturnFromGame, now, &mut effects);
        lifecycle.handle(
            LauncherLifecycleInput::StartupReturnCatalogHydrationNeeded,
            &mut effects,
        );
        lifecycle.handle(
            LauncherLifecycleInput::StartupReturnContextRestored {
                screen: "arcade",
                system_id: "neogeo".to_string(),
                filter: "manufacturer:SNK".to_string(),
                game_path: "/media/fat/_Arcade/Metal Slug.mra".to_string(),
                game_index: 144,
                visual_index: 144.0,
                preview_expected: true,
            },
            &mut effects,
        );
        effects.clear();

        lifecycle.tick_startup_reveal(
            now + LauncherLifecycle::RETURN_PREVIEW_HOLD_TIMEOUT - Duration::from_millis(1),
            true,
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::WaitRelevantPreview
        );
        assert!(!lifecycle.startup_can_present_frame());
        assert!(effects.as_slice().is_empty());

        lifecycle.tick_startup_reveal(
            now + LauncherLifecycle::RETURN_PREVIEW_HOLD_TIMEOUT,
            true,
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::RevealLauncher
        );
        assert!(lifecycle.startup_can_present_frame());
        assert!(effect_names(&effects).contains(&"launcher_reveal_ready"));
    }

    #[test]
    fn return_start_reveals_after_black_screen_timeout_without_return_state() {
        let now = Instant::now();
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.begin_startup_reveal(StartupMode::ReturnFromGame, now, &mut effects);
        effects.clear();

        lifecycle.tick_startup_reveal(
            now + LauncherLifecycle::RETURN_BLACK_SCREEN_TIMEOUT - Duration::from_millis(1),
            false,
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::HoldBlackReturn
        );
        assert!(!lifecycle.startup_can_present_frame());
        assert!(effects.as_slice().is_empty());

        lifecycle.tick_startup_reveal(
            now + LauncherLifecycle::RETURN_BLACK_SCREEN_TIMEOUT,
            false,
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::RevealLauncher
        );
        assert!(lifecycle.startup_can_present_frame());
        assert_eq!(
            effect_detail(&effects, "return_black_screen_timeout"),
            Some("elapsed_ms=5000 state=hold_black_return")
        );
        assert!(effect_names(&effects).contains(&"launcher_reveal_ready"));
    }

    #[test]
    fn return_catalog_hydration_cannot_hold_black_past_timeout() {
        let now = Instant::now();
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.begin_startup_reveal(StartupMode::ReturnFromGame, now, &mut effects);
        lifecycle.handle(
            LauncherLifecycleInput::StartupReturnCatalogHydrationNeeded,
            &mut effects,
        );
        effects.clear();

        lifecycle.tick_startup_reveal(
            now + LauncherLifecycle::RETURN_BLACK_SCREEN_TIMEOUT,
            false,
            &mut effects,
        );

        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::RevealLauncher
        );
        assert!(lifecycle.startup_can_present_frame());
        assert_eq!(
            effect_detail(&effects, "return_black_screen_timeout"),
            Some("elapsed_ms=5000 state=hydrate_return_catalog")
        );
    }

    #[test]
    fn return_black_screen_timeout_does_not_apply_to_warm_start() {
        let now = Instant::now();
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.begin_startup_reveal(StartupMode::WarmCatalog, now, &mut effects);
        effects.clear();
        lifecycle.tick_startup_reveal(
            now + LauncherLifecycle::RETURN_BLACK_SCREEN_TIMEOUT,
            false,
            &mut effects,
        );

        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::HoldBlack
        );
        assert!(effects.as_slice().is_empty());
    }

    #[test]
    fn warm_start_keeps_background_catalog_delay() {
        let now = Instant::now();
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.begin_startup_reveal(StartupMode::WarmCatalog, now, &mut effects);

        assert_eq!(
            lifecycle.catalog_worker_start_delay(Duration::from_secs(2)),
            Duration::from_secs(2)
        );
        assert!(!lifecycle.startup_waiting_for_return_catalog());
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
        assert_eq!(
            effect_names(&effects),
            vec!["launcher_lifecycle_transition"]
        );
    }

    #[test]
    fn cold_missing_catalog_enters_visible_building_state() {
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.after_boot_splash_presented(
            StartupCatalogState::Building {
                mode: CatalogBuildMode::FirstBuild,
                foreground_catalog_update: false,
                has_stale_catalog: false,
            },
            &mut effects,
        );

        assert_eq!(
            lifecycle.state(),
            &LauncherLifecycleState::CatalogBuilding {
                mode: CatalogBuildMode::FirstBuild,
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
                mode: CatalogBuildMode::FirstBuild,
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
        let now = Instant::now();

        lifecycle.begin_startup_reveal(StartupMode::WarmCatalog, now, &mut effects);
        lifecycle.tick_startup_reveal(now, true, &mut effects);
        lifecycle.note_startup_frame_presented(0, now, &mut effects);
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
    fn launch_before_startup_input_enabled_is_rejected() {
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();
        let now = Instant::now();

        lifecycle.begin_startup_reveal(StartupMode::WarmCatalog, now, &mut effects);
        lifecycle.after_boot_splash_presented(
            StartupCatalogState::Ready {
                source: CatalogSource::FullSqlite,
                validation_scheduled: false,
            },
            &mut effects,
        );
        assert_eq!(lifecycle.state(), &LauncherLifecycleState::Idle);
        effects.clear();

        lifecycle.handle(
            LauncherLifecycleInput::LaunchRequested {
                launch_ref: "hidden-startup.mra".to_string(),
            },
            &mut effects,
        );

        assert_eq!(lifecycle.state(), &LauncherLifecycleState::Idle);
        assert!(effects.as_slice().is_empty());
    }

    #[test]
    fn launch_during_catalog_validation_is_rejected() {
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.after_boot_splash_presented(
            StartupCatalogState::Ready {
                source: CatalogSource::SummaryProjection,
                validation_scheduled: true,
            },
            &mut effects,
        );
        effects.clear();

        lifecycle.handle(
            LauncherLifecycleInput::LaunchRequested {
                launch_ref: "validating.mra".to_string(),
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
        assert!(effects.as_slice().is_empty());
    }

    #[test]
    fn stale_launch_terminal_events_are_ignored_while_idle() {
        let (mut lifecycle, mut effects) = idle_lifecycle();

        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::LaunchFailed {
                title: "Test".to_string(),
                kind: launcher::LaunchFailureKind::Internal,
                detail: "late failure".to_string(),
            },
        );
        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::LaunchSucceeded {
                spawned_mister: true,
            },
        );
        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::LaunchTimedOut,
        );
        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::BenchmarkLaunchCompleted,
        );
    }

    #[test]
    fn stale_launch_terminal_events_are_ignored_during_catalog_validation() {
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.after_boot_splash_presented(
            StartupCatalogState::Ready {
                source: CatalogSource::SummaryProjection,
                validation_scheduled: true,
            },
            &mut effects,
        );

        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::LaunchFailed {
                title: "Test".to_string(),
                kind: launcher::LaunchFailureKind::Internal,
                detail: "late failure".to_string(),
            },
        );
        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::LaunchSucceeded {
                spawned_mister: false,
            },
        );
        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::LaunchTimedOut,
        );
    }

    #[test]
    fn stale_launch_terminal_events_are_ignored_after_recovery() {
        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::LaunchRequested {
                launch_ref: "bad.mra".to_string(),
            },
            &mut effects,
        );
        lifecycle.loading_frame_presented(Instant::now(), &mut effects);
        lifecycle.handle(
            LauncherLifecycleInput::LaunchFailed {
                title: "Test".to_string(),
                kind: launcher::LaunchFailureKind::UnreadablePayload,
                detail: "missing file".to_string(),
            },
            &mut effects,
        );
        assert!(matches!(
            lifecycle.state(),
            LauncherLifecycleState::Recovered { .. }
        ));

        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::LaunchSucceeded {
                spawned_mister: true,
            },
        );
        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::LaunchTimedOut,
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
    fn launch_failure_remains_visible_until_acknowledged() {
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
                title: "Test".to_string(),
                kind: launcher::LaunchFailureKind::UnreadablePayload,
                detail: "missing file".to_string(),
            },
            &mut effects,
        );

        assert!(matches!(
            lifecycle.state(),
            LauncherLifecycleState::Recovered {
                reason: RecoveryReason::LaunchFailed { .. }
            }
        ));
        assert!(matches!(
            effects.as_slice().first(),
            Some(LauncherEffect::PresentRecoveryFrame)
        ));

        effects.clear();
        lifecycle.recovery_frame_presented(Instant::now(), &mut effects);

        assert!(matches!(
            lifecycle.state(),
            LauncherLifecycleState::Recovered { .. }
        ));
        assert!(effects.as_slice().is_empty());
        let dialog = lifecycle
            .view()
            .launch_failure_dialog()
            .expect("failure dialog");
        assert_eq!(dialog.title, "Couldn't launch Test");
        assert!(dialog.message.contains("game file could not be read"));

        lifecycle.handle(
            LauncherLifecycleInput::LaunchFailureAcknowledge,
            &mut effects,
        );
        assert_eq!(lifecycle.state(), &LauncherLifecycleState::Idle);
        assert!(matches!(
            effects.as_slice().first(),
            Some(LauncherEffect::ReturnToIdle)
        ));
    }

    #[test]
    fn launch_failure_kinds_map_to_helpful_non_technical_copy() {
        for (kind, expected) in [
            (
                launcher::LaunchFailureKind::UnreadablePayload,
                "game file could not be read",
            ),
            (
                launcher::LaunchFailureKind::DamagedArchive,
                "archive is damaged or unsupported",
            ),
            (
                launcher::LaunchFailureKind::MissingCore,
                "required core is not installed",
            ),
            (
                launcher::LaunchFailureKind::HandoffRejected,
                "MiSTer did not accept",
            ),
        ] {
            let view = LauncherView {
                state: LauncherLifecycleState::Recovered {
                    reason: RecoveryReason::LaunchFailed {
                        title: "Game".to_string(),
                        kind,
                        detail: "/technical/path".to_string(),
                    },
                },
            };
            let dialog = view.launch_failure_dialog().expect("failure dialog");
            assert!(dialog.message.contains(expected));
            assert!(!dialog.message.contains("/technical/path"));
        }
    }

    #[test]
    fn catalog_load_failure_retry_and_repeated_failure_follow_state_chart() {
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();
        lifecycle.begin_startup_reveal(StartupMode::ColdNoCatalog, Instant::now(), &mut effects);
        lifecycle.after_boot_splash_presented(
            StartupCatalogState::LoadFailed {
                error: "corrupt sqlite".to_string(),
                has_stale_catalog: false,
            },
            &mut effects,
        );

        let dialog = lifecycle
            .view()
            .catalog_recovery_dialog()
            .expect("recovery dialog");
        assert_eq!(dialog.error, "corrupt sqlite");
        assert_eq!(dialog.selected, CatalogRecoveryChoice::Retry);

        effects.clear();
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryConfirm, &mut effects);
        assert_eq!(
            lifecycle.state(),
            &LauncherLifecycleState::CatalogRetrying {
                has_stale_catalog: false
            }
        );
        assert!(
            effects
                .as_slice()
                .iter()
                .any(|effect| matches!(effect, LauncherEffect::StartCatalogRetry { .. }))
        );

        effects.clear();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogLoadFailed {
                error: "still corrupt".to_string(),
                has_stale_catalog: false,
            },
            &mut effects,
        );
        let dialog = lifecycle
            .view()
            .catalog_recovery_dialog()
            .expect("repeated recovery dialog");
        assert_eq!(dialog.error, "still corrupt");
        assert_eq!(dialog.selected, CatalogRecoveryChoice::Retry);
    }

    #[test]
    fn catalog_retry_success_returns_to_idle() {
        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogLoadFailed {
                error: "temporarily unavailable".to_string(),
                has_stale_catalog: false,
            },
            &mut effects,
        );
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryConfirm, &mut effects);
        effects.clear();

        lifecycle.handle(
            LauncherLifecycleInput::CatalogReady {
                source: CatalogSource::FullSqlite,
                validating: false,
            },
            &mut effects,
        );

        assert_eq!(lifecycle.state(), &LauncherLifecycleState::Idle);
    }

    #[test]
    fn catalog_fresh_rebuild_cleanup_phases_follow_state_chart() {
        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogLoadFailed {
                error: "unreadable".to_string(),
                has_stale_catalog: true,
            },
            &mut effects,
        );
        effects.clear();
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryRight, &mut effects);
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryConfirm, &mut effects);
        assert_eq!(
            lifecycle.state(),
            &LauncherLifecycleState::FreshRebuilding {
                phase: FreshRebuildPhase::AwaitingLock,
                has_stale_catalog: true,
            }
        );
        assert!(
            effects
                .as_slice()
                .iter()
                .any(|effect| matches!(effect, LauncherEffect::StartFreshCatalogBuild { .. }))
        );

        effects.clear();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogFreshCleanupStarted,
            &mut effects,
        );
        assert_eq!(
            lifecycle.state(),
            &LauncherLifecycleState::FreshRebuilding {
                phase: FreshRebuildPhase::DeletingArtifacts,
                has_stale_catalog: true,
            }
        );
        lifecycle.handle(
            LauncherLifecycleInput::CatalogFreshCleanupCompleted,
            &mut effects,
        );
        assert_eq!(
            lifecycle.state(),
            &LauncherLifecycleState::CatalogBuilding {
                mode: CatalogBuildMode::FreshRecovery,
                foreground: true,
                has_stale_catalog: true,
            }
        );
    }

    #[test]
    fn cleanup_events_are_ignored_outside_fresh_rebuild() {
        let (mut lifecycle, mut effects) = idle_lifecycle();
        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::CatalogFreshCleanupCompleted,
        );
    }

    #[test]
    fn cleanup_completed_is_rejected_until_deletion_started() {
        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogLoadFailed {
                error: "unreadable".to_string(),
                has_stale_catalog: true,
            },
            &mut effects,
        );
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryRight, &mut effects);
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryConfirm, &mut effects);
        effects.clear();

        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::CatalogFreshCleanupCompleted,
        );
    }

    #[test]
    fn cleanup_started_is_rejected_after_deletion_started() {
        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogLoadFailed {
                error: "unreadable".to_string(),
                has_stale_catalog: false,
            },
            &mut effects,
        );
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryRight, &mut effects);
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryConfirm, &mut effects);
        lifecycle.handle(
            LauncherLifecycleInput::CatalogFreshCleanupStarted,
            &mut effects,
        );
        effects.clear();

        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::CatalogFreshCleanupStarted,
        );
    }

    #[test]
    fn stale_catalog_events_cannot_interrupt_launching() {
        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::LaunchRequested {
                launch_ref: "game.mra".to_string(),
            },
            &mut effects,
        );

        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::CatalogReady {
                source: CatalogSource::FreshBuild,
                validating: false,
            },
        );
        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::CatalogBuilding {
                mode: CatalogBuildMode::Update,
                foreground: true,
                has_stale_catalog: true,
            },
        );
        assert_input_ignored(
            &mut lifecycle,
            &mut effects,
            LauncherLifecycleInput::CatalogLoadFailed {
                error: "stale worker".to_string(),
                has_stale_catalog: true,
            },
        );
    }
}
