// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::{arcade_catalog, launcher};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogSource {
    ReturnCapsule,
    ShardedRegistry,
    SummaryProjection,
    NavigationProjection,
    FullSqlite,
    FreshBuild,
}

impl CatalogSource {
    pub fn label(self) -> &'static str {
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
pub enum LauncherLifecycleState {
    StartupCatalogPending,
    CatalogBuilding {
        mode: CatalogBuildMode,
        foreground: bool,
        has_stale_catalog: bool,
    },
    CatalogLoadFailed {
        error: String,
        has_stale_catalog: bool,
        mode: CatalogRecoveryMode,
        selected: CatalogRecoveryChoice,
    },
    CatalogRetrying {
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
    pub fn label(&self) -> &'static str {
        match self {
            Self::StartupCatalogPending => "startup-catalog-pending",
            Self::CatalogBuilding { .. } => "catalog-building",
            Self::CatalogLoadFailed { .. } => "catalog-load-failed",
            Self::CatalogRetrying { .. } => "catalog-retrying",
            Self::CatalogReady { .. } => "catalog-ready",
            Self::Idle => "idle",
            Self::Launching { .. } => "launching",
            Self::Handoff { .. } => "handoff",
            Self::Recovered { .. } => "recovered",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogBuildMode {
    FirstBuild,
    Update,
    FreshRecovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogRecoveryChoice {
    Left,
    Right,
}

impl CatalogRecoveryChoice {
    pub fn selected_index(self) -> i32 {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogRecoveryMode {
    InputsChanged,
    UpgradeRequired,
    RepairRequired,
    LoadFailure { transient: bool },
    PersistenceFailure { transient: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogRecoveryAction {
    Continue,
    Retry,
    AtomicRebuild,
    FreshRebuild,
    ExitToMister,
}

impl CatalogRecoveryMode {
    fn title(self) -> &'static str {
        match self {
            Self::InputsChanged => "Library changed",
            Self::UpgradeRequired => "Catalog update required",
            Self::RepairRequired => "Catalog repair required",
            Self::LoadFailure { .. } => "Catalog unavailable",
            Self::PersistenceFailure { .. } => "Catalog rebuild failed",
        }
    }

    fn action(
        self,
        has_stale_catalog: bool,
        choice: CatalogRecoveryChoice,
    ) -> CatalogRecoveryAction {
        match (self, has_stale_catalog, choice) {
            (Self::InputsChanged | Self::UpgradeRequired, true, CatalogRecoveryChoice::Left) => {
                CatalogRecoveryAction::Continue
            }
            (Self::InputsChanged | Self::UpgradeRequired, true, CatalogRecoveryChoice::Right) => {
                CatalogRecoveryAction::AtomicRebuild
            }
            (
                Self::LoadFailure { transient: true }
                | Self::PersistenceFailure { transient: true },
                false,
                CatalogRecoveryChoice::Left,
            ) => CatalogRecoveryAction::Retry,
            (_, true, CatalogRecoveryChoice::Left) => CatalogRecoveryAction::Continue,
            (_, false, CatalogRecoveryChoice::Left) => CatalogRecoveryAction::ExitToMister,
            (_, _, CatalogRecoveryChoice::Right) => CatalogRecoveryAction::FreshRebuild,
        }
    }

    pub fn label(self, has_stale_catalog: bool, choice: CatalogRecoveryChoice) -> &'static str {
        match self.action(has_stale_catalog, choice) {
            CatalogRecoveryAction::Continue => "Continue",
            CatalogRecoveryAction::Retry => "Retry",
            CatalogRecoveryAction::AtomicRebuild => "Rebuild",
            CatalogRecoveryAction::FreshRebuild => "Full rebuild",
            CatalogRecoveryAction::ExitToMister => "Exit to MiSTer",
        }
    }

    pub fn diagnostic_code(self) -> &'static str {
        match self {
            Self::InputsChanged => "catalog_inputs_changed",
            Self::UpgradeRequired => "projection_upgrade_required",
            Self::RepairRequired => "catalog_repair_required",
            Self::LoadFailure { .. } => "catalog_load_failed",
            Self::PersistenceFailure { .. } => "catalog_persistence_failed",
        }
    }

    pub fn diagnostic_stage(self) -> &'static str {
        match self {
            Self::InputsChanged | Self::UpgradeRequired | Self::RepairRequired => "validate",
            Self::LoadFailure { .. } => "load",
            Self::PersistenceFailure { .. } => "persist",
        }
    }

    pub fn diagnostic_operation(self) -> &'static str {
        match self {
            Self::InputsChanged | Self::UpgradeRequired | Self::RepairRequired => "check",
            Self::LoadFailure { .. } => "load",
            Self::PersistenceFailure { .. } => "rebuild",
        }
    }
}

fn catalog_recovery_message(
    mode: CatalogRecoveryMode,
    detail: &str,
    has_stale_catalog: bool,
) -> String {
    let safety = if has_stale_catalog {
        " Available games remain playable."
    } else {
        " No usable generated catalog is currently available."
    };
    let rebuild = match mode {
        CatalogRecoveryMode::InputsChanged | CatalogRecoveryMode::UpgradeRequired => {
            " Rebuild creates and validates a replacement before switching catalogs."
        }
        _ => {
            " Full rebuild deletes generated catalog data only; games, screenshots, and media are untouched."
        }
    };
    let report = match mode {
        CatalogRecoveryMode::InputsChanged | CatalogRecoveryMode::UpgradeRequired => "",
        _ => " Support report: diagnostics/catalog/latest.json.",
    };
    format!("{detail}{safety}{rebuild}{report}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchingPhase {
    LoadingFramePending { launch_ref: String },
    HandoffPending,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryReason {
    LaunchFailed {
        title: String,
        kind: launcher::LaunchFailureKind,
        detail: String,
    },
    LaunchTimedOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupMode {
    ColdNoCatalog,
    WarmCatalog,
    WarmCatalogHydrating,
    ReturnFromGame,
}

impl StartupMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::ColdNoCatalog => "cold_no_catalog",
            Self::WarmCatalog => "warm_catalog",
            Self::WarmCatalogHydrating => "warm_catalog_hydrating",
            Self::ReturnFromGame => "return_from_game",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupRevealState {
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
    pub fn label(self) -> &'static str {
        match self {
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
pub struct StartupRevealStatus {
    pub mode: StartupMode,
    pub state: StartupRevealState,
    pub revealed: bool,
    pub input_enabled: bool,
    pub reveal_ms: u64,
    pub input_enabled_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeSyncPlan {
    None,
    Light,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LauncherInputMode {
    Normal,
    Launching,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LauncherEffect {
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
    StartCatalogRebuild {
        root: String,
    },
    StartFreshCatalogBuild {
        root: String,
    },
    ExitToMister,
}

#[derive(Debug, Default)]
pub struct LifecycleEffects {
    effects: Vec<LauncherEffect>,
}

impl LifecycleEffects {
    pub fn new() -> Self {
        Self {
            effects: Vec::with_capacity(8),
        }
    }

    pub fn clear(&mut self) {
        self.effects.clear();
    }

    pub fn push(&mut self, effect: LauncherEffect) {
        self.effects.push(effect);
    }

    pub fn startup_event(&mut self, name: &'static str, detail: impl Into<String>) {
        self.push(LauncherEffect::StartupEvent {
            name,
            detail: detail.into(),
        });
    }

    pub fn has_startup_event(&self, expected_name: &str) -> bool {
        self.effects.iter().any(|effect| {
            matches!(
                effect,
                LauncherEffect::StartupEvent { name, .. } if *name == expected_name
            )
        })
    }

    pub fn drain(&mut self) -> impl Iterator<Item = LauncherEffect> + '_ {
        self.effects.drain(..)
    }

    #[cfg(test)]
    fn as_slice(&self) -> &[LauncherEffect] {
        &self.effects
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LauncherLifecycleConfig {
    pub catalog_worker_enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartupCatalogState {
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
        transient: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum LauncherLifecycleInput {
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
        restored_at: Instant,
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
        transient: bool,
    },
    CatalogRecoveryRequired {
        error: String,
        has_stale_catalog: bool,
        mode: CatalogRecoveryMode,
    },
    CatalogRecoveryLeft,
    CatalogRecoveryRight,
    CatalogRecoveryConfirm,
    CatalogRecoveryCancel,
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
pub struct LauncherLifecycleStep {
    pub state: LauncherLifecycleState,
    pub bridge_sync: BridgeSyncPlan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherView {
    pub state: LauncherLifecycleState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogRecoveryDialog {
    pub title: &'static str,
    pub message: String,
    pub left_label: &'static str,
    pub right_label: &'static str,
    pub selected: CatalogRecoveryChoice,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchFailureDialog {
    pub title: String,
    pub message: &'static str,
}

impl LauncherView {
    pub fn catalog_recovery_dialog(&self) -> Option<CatalogRecoveryDialog> {
        match &self.state {
            LauncherLifecycleState::CatalogLoadFailed {
                error,
                has_stale_catalog,
                mode,
                selected,
            } => Some(CatalogRecoveryDialog {
                title: mode.title(),
                message: catalog_recovery_message(*mode, error, *has_stale_catalog),
                left_label: mode.label(*has_stale_catalog, CatalogRecoveryChoice::Left),
                right_label: mode.label(*has_stale_catalog, CatalogRecoveryChoice::Right),
                selected: *selected,
            }),
            _ => None,
        }
    }

    pub fn launch_failure_dialog(&self) -> Option<LaunchFailureDialog> {
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

pub struct LauncherLifecycle {
    state: LauncherLifecycleState,
    config: LauncherLifecycleConfig,
    startup_catalog_classified: bool,
    startup_mode: StartupMode,
    startup_reveal_state: StartupRevealState,
    startup_started_at: Instant,
    return_preview_wait_started_at: Option<Instant>,
    startup_revealed_at: Option<Instant>,
    startup_input_enabled_at: Option<Instant>,
    catalog_root: String,
}

impl LauncherLifecycle {
    pub const COLD_STARTUP_MAX_DURATION: Duration = Duration::from_secs(20);
    pub const RETURN_PREVIEW_HOLD_TIMEOUT: Duration = Duration::from_millis(250);
    pub const RETURN_BLACK_SCREEN_TIMEOUT: Duration = Duration::from_secs(5);

    pub fn new(config: LauncherLifecycleConfig, now: Instant) -> Self {
        Self {
            state: LauncherLifecycleState::StartupCatalogPending,
            config,
            startup_catalog_classified: false,
            startup_mode: StartupMode::WarmCatalog,
            startup_reveal_state: StartupRevealState::HoldBlack,
            startup_started_at: now,
            return_preview_wait_started_at: None,
            startup_revealed_at: None,
            startup_input_enabled_at: None,
            catalog_root: arcade_catalog::DEFAULT_ARCADE_ROOT.to_string(),
        }
    }

    pub fn set_catalog_root(&mut self, root: String) {
        self.catalog_root = root;
    }

    pub fn begin_startup_reveal(
        &mut self,
        mode: StartupMode,
        now: Instant,
        out: &mut LifecycleEffects,
    ) {
        self.startup_mode = mode;
        self.startup_started_at = now;
        self.return_preview_wait_started_at = None;
        self.startup_revealed_at = None;
        self.startup_input_enabled_at = None;
        self.startup_reveal_state = match mode {
            StartupMode::ColdNoCatalog => StartupRevealState::CatalogProgressVisible,
            StartupMode::WarmCatalog => StartupRevealState::RevealLauncher,
            StartupMode::WarmCatalogHydrating => StartupRevealState::HoldBlack,
            StartupMode::ReturnFromGame => StartupRevealState::HoldBlackReturn,
        };
        out.startup_event("startup_entry_classified", format!("mode={}", mode.label()));
        match self.startup_reveal_state {
            StartupRevealState::CatalogProgressVisible => {
                out.startup_event("catalog_progress_revealed", "mode=cold_no_catalog");
            }
            StartupRevealState::RevealLauncher => {
                out.startup_event("startup_shell_visible", "mode=warm_catalog");
                out.startup_event(
                    "launcher_reveal_ready",
                    "mode=warm_catalog catalog_state=hydrating",
                );
            }
            StartupRevealState::HoldBlack => {
                out.startup_event("startup_hold_black", format!("mode={}", mode.label()));
            }
            StartupRevealState::HoldBlackReturn => {
                out.startup_event("startup_hold_black", "mode=return_from_game");
            }
            _ => {}
        }
    }

    pub fn tick_startup_reveal(
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
            StartupRevealState::CatalogProgressVisible if catalog_ready => {
                self.mark_reveal_ready("preview_state=not_required", out);
            }
            StartupRevealState::CatalogProgressVisible | StartupRevealState::HoldBlack
                if self.startup_hard_deadline_reached(now) =>
            {
                out.startup_event(
                    "startup_hard_timeout",
                    format!("elapsed_ms={}", startup_elapsed.as_millis()),
                );
                self.mark_reveal_ready("preview_state=startup_hard_timeout", out);
            }
            StartupRevealState::HoldBlack if catalog_ready => {
                self.mark_reveal_ready("preview_state=not_required", out);
            }
            StartupRevealState::WaitRelevantPreview
                if self.return_preview_wait_started_at.is_some_and(|started| {
                    now.saturating_duration_since(started) >= Self::RETURN_PREVIEW_HOLD_TIMEOUT
                }) =>
            {
                out.startup_event(
                    "return_preview_timeout",
                    format!(
                        "elapsed_ms={}",
                        self.return_preview_wait_started_at
                            .map(|started| now.saturating_duration_since(started).as_millis())
                            .unwrap_or(0)
                    ),
                );
                self.mark_reveal_ready("preview_state=return_preview_timeout", out);
            }
            _ => {}
        }
    }

    pub fn startup_can_present_frame(&self) -> bool {
        matches!(
            self.startup_reveal_state,
            StartupRevealState::CatalogProgressVisible
                | StartupRevealState::RevealLauncher
                | StartupRevealState::InputEnabled
        )
    }

    pub fn startup_input_enabled(&self) -> bool {
        self.startup_input_enabled_at.is_some()
    }

    pub fn startup_hard_deadline_reached(&self, now: Instant) -> bool {
        matches!(
            self.startup_mode,
            StartupMode::ColdNoCatalog | StartupMode::WarmCatalogHydrating
        ) && now.saturating_duration_since(self.startup_started_at)
            >= Self::COLD_STARTUP_MAX_DURATION
    }

    pub fn startup_waiting_for_initial_catalog(&self) -> bool {
        self.startup_mode == StartupMode::WarmCatalogHydrating
            && self.startup_reveal_state == StartupRevealState::HoldBlack
    }

    pub fn startup_waiting_for_return_catalog(&self) -> bool {
        self.startup_mode == StartupMode::ReturnFromGame
            && self.startup_reveal_state == StartupRevealState::HydrateReturnCatalog
    }

    pub fn catalog_worker_start_delay(&self, default_delay: Duration) -> Duration {
        if self.startup_waiting_for_return_catalog() {
            Duration::ZERO
        } else {
            default_delay
        }
    }

    pub fn startup_status(&self) -> StartupRevealStatus {
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

    pub fn note_startup_frame_presented(
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

    pub fn classify_startup_catalog(
        &mut self,
        input: StartupCatalogState,
        out: &mut LifecycleEffects,
    ) -> LauncherLifecycleStep {
        self.startup_catalog_classified = true;
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
                    "startup_catalog_classified",
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
                    "startup_catalog_classified",
                );
            }
            StartupCatalogState::LoadFailed {
                error,
                has_stale_catalog,
                transient,
            } => {
                self.transition(
                    LauncherLifecycleState::CatalogLoadFailed {
                        error,
                        has_stale_catalog,
                        mode: CatalogRecoveryMode::LoadFailure { transient },
                        selected: CatalogRecoveryChoice::Left,
                    },
                    out,
                    "catalog_load_failed",
                );
                self.mark_reveal_ready("catalog_load_failed", out);
            }
        }
        self.step(BridgeSyncPlan::Full)
    }

    pub fn input_mode(&self) -> LauncherInputMode {
        match self.state {
            LauncherLifecycleState::Launching { .. } | LauncherLifecycleState::Handoff { .. } => {
                LauncherInputMode::Launching
            }
            _ => LauncherInputMode::Normal,
        }
    }

    pub fn handle(
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
                restored_at,
            } => {
                self.startup_reveal_state = StartupRevealState::RestoreContext;
                out.startup_event("startup_restore_context", "mode=return_from_game");
                out.startup_event(
                    "return_context_restored",
                    format!(
                        "screen={screen} system_id={system_id} filter={filter} game_path={game_path} game_index={game_index} visual_index={visual_index:.3} preview_expected={preview_expected}"
                    ),
                );
                if *preview_expected {
                    self.startup_reveal_state = StartupRevealState::WaitRelevantPreview;
                    self.return_preview_wait_started_at = Some(*restored_at);
                } else {
                    self.mark_reveal_ready("preview_state=not_required", out);
                }
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
        if !self.startup_catalog_classified {
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
                transient,
            } => {
                if matches!(
                    self.state,
                    LauncherLifecycleState::CatalogBuilding { .. }
                        | LauncherLifecycleState::CatalogRetrying { .. }
                        | LauncherLifecycleState::CatalogReady { .. }
                        | LauncherLifecycleState::Idle
                ) {
                    self.transition(
                        LauncherLifecycleState::CatalogLoadFailed {
                            error,
                            has_stale_catalog,
                            mode: CatalogRecoveryMode::LoadFailure { transient },
                            selected: CatalogRecoveryChoice::Left,
                        },
                        out,
                        "catalog_load_failed",
                    );
                    self.mark_reveal_ready("catalog_load_failed", out);
                }
            }
            LauncherLifecycleInput::CatalogRecoveryRequired {
                error,
                has_stale_catalog,
                mode,
            } => {
                if matches!(
                    self.state,
                    LauncherLifecycleState::CatalogBuilding { .. }
                        | LauncherLifecycleState::CatalogRetrying { .. }
                        | LauncherLifecycleState::CatalogReady { .. }
                        | LauncherLifecycleState::Idle
                ) {
                    self.transition(
                        LauncherLifecycleState::CatalogLoadFailed {
                            error,
                            has_stale_catalog,
                            mode,
                            selected: CatalogRecoveryChoice::Left,
                        },
                        out,
                        "catalog_recovery_required",
                    );
                    self.mark_reveal_ready("catalog_recovery_required", out);
                }
            }
            LauncherLifecycleInput::CatalogRecoveryLeft => {
                if let LauncherLifecycleState::CatalogLoadFailed {
                    error,
                    has_stale_catalog,
                    mode,
                    ..
                } = &self.state
                {
                    self.transition(
                        LauncherLifecycleState::CatalogLoadFailed {
                            error: error.clone(),
                            has_stale_catalog: *has_stale_catalog,
                            mode: *mode,
                            selected: CatalogRecoveryChoice::Left,
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
                    mode,
                    ..
                } = &self.state
                {
                    self.transition(
                        LauncherLifecycleState::CatalogLoadFailed {
                            error: error.clone(),
                            has_stale_catalog: *has_stale_catalog,
                            mode: *mode,
                            selected: CatalogRecoveryChoice::Right,
                        },
                        out,
                        "catalog_recovery_right",
                    );
                }
            }
            LauncherLifecycleInput::CatalogRecoveryConfirm => {
                if let LauncherLifecycleState::CatalogLoadFailed {
                    has_stale_catalog,
                    mode,
                    selected,
                    ..
                } = self.state.clone()
                {
                    match mode.action(has_stale_catalog, selected) {
                        CatalogRecoveryAction::Continue => {
                            self.transition(LauncherLifecycleState::Idle, out, "catalog_continue");
                            out.push(LauncherEffect::ReturnToIdle);
                        }
                        CatalogRecoveryAction::Retry => {
                            self.transition(
                                LauncherLifecycleState::CatalogRetrying { has_stale_catalog },
                                out,
                                "catalog_retry_requested",
                            );
                            out.push(LauncherEffect::StartCatalogRetry {
                                root: self.catalog_root.clone(),
                            });
                        }
                        CatalogRecoveryAction::AtomicRebuild => {
                            self.transition(
                                LauncherLifecycleState::CatalogBuilding {
                                    mode: CatalogBuildMode::Update,
                                    foreground: true,
                                    has_stale_catalog,
                                },
                                out,
                                "catalog_rebuild_requested",
                            );
                            out.push(LauncherEffect::StartCatalogRebuild {
                                root: self.catalog_root.clone(),
                            });
                        }
                        CatalogRecoveryAction::FreshRebuild => {
                            self.transition(
                                LauncherLifecycleState::CatalogBuilding {
                                    mode: CatalogBuildMode::FreshRecovery,
                                    foreground: true,
                                    has_stale_catalog,
                                },
                                out,
                                "catalog_fresh_rebuild_requested",
                            );
                            out.push(LauncherEffect::StartFreshCatalogBuild {
                                root: self.catalog_root.clone(),
                            });
                        }
                        CatalogRecoveryAction::ExitToMister => {
                            self.transition(LauncherLifecycleState::Idle, out, "catalog_exit");
                            out.push(LauncherEffect::ExitToMister);
                        }
                    }
                }
            }
            LauncherLifecycleInput::CatalogRecoveryCancel => {
                if let LauncherLifecycleState::CatalogLoadFailed {
                    has_stale_catalog, ..
                } = self.state
                {
                    if has_stale_catalog {
                        self.transition(LauncherLifecycleState::Idle, out, "catalog_continue");
                        out.push(LauncherEffect::ReturnToIdle);
                    } else {
                        self.transition(LauncherLifecycleState::Idle, out, "catalog_exit");
                        out.push(LauncherEffect::ExitToMister);
                    }
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
                if matches!(
                    self.state,
                    LauncherLifecycleState::Idle | LauncherLifecycleState::CatalogReady { .. }
                ) && self.startup_input_enabled()
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

    pub fn loading_frame_presented(&mut self, at: Instant, out: &mut LifecycleEffects) {
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

    pub fn recovery_frame_presented(&mut self, _at: Instant, _out: &mut LifecycleEffects) {}

    pub fn state(&self) -> &LauncherLifecycleState {
        &self.state
    }

    pub fn view(&self) -> LauncherView {
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
        lifecycle.classify_startup_catalog(
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
    fn startup_catalog_is_pending_before_classification() {
        let lifecycle = lifecycle();

        assert_eq!(
            lifecycle.state(),
            &LauncherLifecycleState::StartupCatalogPending
        );
    }

    #[test]
    fn launch_before_startup_catalog_classification_is_rejected() {
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.handle(
            LauncherLifecycleInput::LaunchRequested {
                launch_ref: "too-early.mra".to_string(),
            },
            &mut effects,
        );

        assert_eq!(
            lifecycle.state(),
            &LauncherLifecycleState::StartupCatalogPending
        );
        assert!(effects.as_slice().is_empty());
    }

    #[test]
    fn cold_start_shows_catalog_progress_immediately() {
        let now = Instant::now();
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.begin_startup_reveal(StartupMode::ColdNoCatalog, now, &mut effects);
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::CatalogProgressVisible
        );
        assert!(lifecycle.startup_can_present_frame());
        assert!(!lifecycle.startup_input_enabled());
        assert!(effect_names(&effects).contains(&"catalog_progress_revealed"));
        effects.clear();

        lifecycle.tick_startup_reveal(now + Duration::from_millis(1), false, &mut effects);
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::CatalogProgressVisible
        );
        assert!(effects.as_slice().is_empty());
    }

    #[test]
    fn cold_start_becomes_interactive_at_twenty_seconds_without_a_catalog() {
        let now = Instant::now();
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();
        lifecycle.begin_startup_reveal(StartupMode::ColdNoCatalog, now, &mut effects);
        effects.clear();

        lifecycle.tick_startup_reveal(
            now + LauncherLifecycle::COLD_STARTUP_MAX_DURATION - Duration::from_millis(1),
            false,
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::CatalogProgressVisible
        );

        lifecycle.tick_startup_reveal(
            now + LauncherLifecycle::COLD_STARTUP_MAX_DURATION,
            false,
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::RevealLauncher
        );
        assert!(effect_names(&effects).contains(&"startup_hard_timeout"));
        lifecycle.note_startup_frame_presented(
            1,
            now + LauncherLifecycle::COLD_STARTUP_MAX_DURATION,
            &mut effects,
        );
        assert!(lifecycle.startup_input_enabled());
    }

    #[test]
    fn warm_start_reveals_shell_before_catalog_hydration() {
        let now = Instant::now();
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.begin_startup_reveal(StartupMode::WarmCatalog, now, &mut effects);
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::RevealLauncher
        );
        assert!(lifecycle.startup_can_present_frame());
        assert!(effect_names(&effects).contains(&"startup_shell_visible"));
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
    fn warm_catalog_hydration_holds_black_until_catalog_ready() {
        let now = Instant::now();
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.begin_startup_reveal(StartupMode::WarmCatalogHydrating, now, &mut effects);
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::HoldBlack
        );
        assert!(!lifecycle.startup_can_present_frame());
        assert!(!lifecycle.startup_input_enabled());
        assert!(lifecycle.startup_waiting_for_initial_catalog());
        assert!(effect_names(&effects).contains(&"startup_hold_black"));
        assert!(!effect_names(&effects).contains(&"launcher_reveal_ready"));
        effects.clear();

        lifecycle.tick_startup_reveal(now + Duration::from_millis(1), false, &mut effects);
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::HoldBlack
        );
        assert!(effects.as_slice().is_empty());

        lifecycle.tick_startup_reveal(now + Duration::from_millis(2), true, &mut effects);
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::RevealLauncher
        );
        assert!(!lifecycle.startup_waiting_for_initial_catalog());
        assert!(effect_names(&effects).contains(&"launcher_reveal_ready"));
    }

    #[test]
    fn warm_catalog_hydration_fails_open_after_startup_deadline() {
        let now = Instant::now();
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.begin_startup_reveal(StartupMode::WarmCatalogHydrating, now, &mut effects);
        effects.clear();

        lifecycle.tick_startup_reveal(
            now + LauncherLifecycle::COLD_STARTUP_MAX_DURATION - Duration::from_millis(1),
            false,
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::HoldBlack
        );

        lifecycle.tick_startup_reveal(
            now + LauncherLifecycle::COLD_STARTUP_MAX_DURATION,
            false,
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::RevealLauncher
        );
        assert!(effect_names(&effects).contains(&"startup_hard_timeout"));
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
                restored_at: now,
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
    fn return_start_reveals_when_no_preview_is_expected() {
        let now = Instant::now();
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.begin_startup_reveal(StartupMode::ReturnFromGame, now, &mut effects);
        lifecycle.handle(
            LauncherLifecycleInput::StartupReturnCatalogHydrationNeeded,
            &mut effects,
        );
        effects.clear();

        lifecycle.handle(
            LauncherLifecycleInput::StartupReturnContextRestored {
                screen: "library",
                system_id: "nes".to_string(),
                filter: "all".to_string(),
                game_path: String::new(),
                game_index: 0,
                visual_index: 0.0,
                preview_expected: false,
                restored_at: now,
            },
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
    fn return_start_reveals_black_if_preview_never_becomes_ready() {
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
                restored_at: now,
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
        assert!(effect_names(&effects).contains(&"return_preview_timeout"));
        assert!(effect_names(&effects).contains(&"launcher_reveal_ready"));
    }

    #[test]
    fn return_preview_short_timeout_reveals_loading_surface() {
        let now = Instant::now();
        let restored_at = now + Duration::from_secs(1);
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();

        lifecycle.begin_startup_reveal(StartupMode::ReturnFromGame, now, &mut effects);
        lifecycle.handle(
            LauncherLifecycleInput::StartupReturnContextRestored {
                screen: "arcade",
                system_id: "neogeo".to_string(),
                filter: "all".to_string(),
                game_path: "/media/fat/_Arcade/Metal Slug.mra".to_string(),
                game_index: 144,
                visual_index: 144.0,
                preview_expected: true,
                restored_at,
            },
            &mut effects,
        );
        effects.clear();

        lifecycle.tick_startup_reveal(
            restored_at + LauncherLifecycle::RETURN_PREVIEW_HOLD_TIMEOUT - Duration::from_millis(1),
            true,
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::WaitRelevantPreview
        );

        lifecycle.tick_startup_reveal(
            restored_at + LauncherLifecycle::RETURN_PREVIEW_HOLD_TIMEOUT,
            true,
            &mut effects,
        );
        assert_eq!(
            lifecycle.startup_status().state,
            StartupRevealState::RevealLauncher
        );
        assert!(lifecycle.startup_can_present_frame());
        assert_eq!(
            effect_detail(&effects, "return_preview_timeout"),
            Some("elapsed_ms=250")
        );
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
    fn return_black_screen_timeout_leaves_warm_start_revealed() {
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
            StartupRevealState::RevealLauncher
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

        lifecycle.classify_startup_catalog(
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

        lifecycle.classify_startup_catalog(
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

        lifecycle.classify_startup_catalog(
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

        lifecycle.classify_startup_catalog(
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
        lifecycle.classify_startup_catalog(
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
        lifecycle.classify_startup_catalog(
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
    fn launch_during_catalog_validation_uses_published_catalog() {
        let mut lifecycle = lifecycle();
        let mut effects = LifecycleEffects::new();
        let now = Instant::now();

        lifecycle.begin_startup_reveal(StartupMode::WarmCatalog, now, &mut effects);
        lifecycle.tick_startup_reveal(now, true, &mut effects);
        lifecycle.note_startup_frame_presented(0, now, &mut effects);

        lifecycle.classify_startup_catalog(
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
            &LauncherLifecycleState::Launching {
                phase: LaunchingPhase::LoadingFramePending {
                    launch_ref: "validating.mra".to_string()
                }
            }
        );
        assert!(matches!(
            effects.as_slice().first(),
            Some(LauncherEffect::BeginLoadingFrame { launch_ref })
                if launch_ref == "validating.mra"
        ));

        effects.clear();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogValidationFinished,
            &mut effects,
        );

        assert_eq!(
            lifecycle.state(),
            &LauncherLifecycleState::Launching {
                phase: LaunchingPhase::LoadingFramePending {
                    launch_ref: "validating.mra".to_string()
                }
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

        lifecycle.classify_startup_catalog(
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
        lifecycle.classify_startup_catalog(
            StartupCatalogState::LoadFailed {
                error: "corrupt sqlite".to_string(),
                has_stale_catalog: false,
                transient: true,
            },
            &mut effects,
        );

        let dialog = lifecycle
            .view()
            .catalog_recovery_dialog()
            .expect("recovery dialog");
        assert!(dialog.message.contains("corrupt sqlite"));
        assert_eq!(dialog.selected, CatalogRecoveryChoice::Left);

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
                transient: false,
            },
            &mut effects,
        );
        let dialog = lifecycle
            .view()
            .catalog_recovery_dialog()
            .expect("repeated recovery dialog");
        assert!(dialog.message.contains("still corrupt"));
        assert_eq!(dialog.selected, CatalogRecoveryChoice::Left);
    }

    #[test]
    fn catalog_retry_success_returns_to_idle() {
        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogLoadFailed {
                error: "temporarily unavailable".to_string(),
                has_stale_catalog: false,
                transient: true,
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
    fn upgrade_with_stale_catalog_defaults_to_continue_and_can_rebuild_atomically() {
        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogRecoveryRequired {
                error: "installed rich-game-v1, required rich-game-v2".to_string(),
                has_stale_catalog: true,
                mode: CatalogRecoveryMode::UpgradeRequired,
            },
            &mut effects,
        );
        let dialog = lifecycle.view().catalog_recovery_dialog().unwrap();
        assert_eq!(dialog.title, "Catalog update required");
        assert_eq!(dialog.left_label, "Continue");
        assert_eq!(dialog.right_label, "Rebuild");
        assert_eq!(dialog.selected, CatalogRecoveryChoice::Left);
        effects.clear();
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryConfirm, &mut effects);
        assert_eq!(lifecycle.state(), &LauncherLifecycleState::Idle);
        assert!(
            effects
                .as_slice()
                .iter()
                .any(|effect| matches!(effect, LauncherEffect::ReturnToIdle))
        );

        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogRecoveryRequired {
                error: "installed rich-game-v1, required rich-game-v2".to_string(),
                has_stale_catalog: true,
                mode: CatalogRecoveryMode::UpgradeRequired,
            },
            &mut effects,
        );
        effects.clear();
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryRight, &mut effects);
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryConfirm, &mut effects);
        assert!(
            effects
                .as_slice()
                .iter()
                .any(|effect| matches!(effect, LauncherEffect::StartCatalogRebuild { .. }))
        );
    }

    #[test]
    fn failed_rebuild_with_stale_catalog_offers_continue_or_full_rebuild() {
        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogRecoveryRequired {
                error: "publish failed".to_string(),
                has_stale_catalog: true,
                mode: CatalogRecoveryMode::PersistenceFailure { transient: false },
            },
            &mut effects,
        );
        let dialog = lifecycle.view().catalog_recovery_dialog().unwrap();
        assert_eq!(dialog.left_label, "Continue");
        assert_eq!(dialog.right_label, "Full rebuild");
        effects.clear();
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryRight, &mut effects);
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryConfirm, &mut effects);
        assert!(
            effects
                .as_slice()
                .iter()
                .any(|effect| matches!(effect, LauncherEffect::StartFreshCatalogBuild { .. }))
        );
    }

    #[test]
    fn no_catalog_recovery_distinguishes_retry_from_exit_and_cancel_exits() {
        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogRecoveryRequired {
                error: "temporarily unavailable".to_string(),
                has_stale_catalog: false,
                mode: CatalogRecoveryMode::LoadFailure { transient: true },
            },
            &mut effects,
        );
        let dialog = lifecycle.view().catalog_recovery_dialog().unwrap();
        assert_eq!(dialog.left_label, "Retry");
        assert_eq!(dialog.right_label, "Full rebuild");
        effects.clear();
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryCancel, &mut effects);
        assert!(
            effects
                .as_slice()
                .iter()
                .any(|effect| matches!(effect, LauncherEffect::ExitToMister))
        );

        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogRecoveryRequired {
                error: "unsupported schema".to_string(),
                has_stale_catalog: false,
                mode: CatalogRecoveryMode::LoadFailure { transient: false },
            },
            &mut effects,
        );
        let dialog = lifecycle.view().catalog_recovery_dialog().unwrap();
        assert_eq!(dialog.left_label, "Exit to MiSTer");
        effects.clear();
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryConfirm, &mut effects);
        assert!(
            effects
                .as_slice()
                .iter()
                .any(|effect| matches!(effect, LauncherEffect::ExitToMister))
        );
    }

    #[test]
    fn upgrade_without_a_usable_catalog_offers_only_exit_or_full_rebuild() {
        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogRecoveryRequired {
                error: "unsupported shard schema: expected 3, found 2".to_string(),
                has_stale_catalog: false,
                mode: CatalogRecoveryMode::UpgradeRequired,
            },
            &mut effects,
        );
        let dialog = lifecycle.view().catalog_recovery_dialog().unwrap();
        assert_eq!(dialog.left_label, "Exit to MiSTer");
        assert_eq!(dialog.right_label, "Full rebuild");
        effects.clear();
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryRight, &mut effects);
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryConfirm, &mut effects);
        assert!(
            effects
                .as_slice()
                .iter()
                .any(|effect| matches!(effect, LauncherEffect::StartFreshCatalogBuild { .. }))
        );
    }

    #[test]
    fn catalog_fresh_rebuild_enters_building_state_immediately() {
        let (mut lifecycle, mut effects) = idle_lifecycle();
        lifecycle.handle(
            LauncherLifecycleInput::CatalogLoadFailed {
                error: "unreadable".to_string(),
                has_stale_catalog: true,
                transient: false,
            },
            &mut effects,
        );
        effects.clear();
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryRight, &mut effects);
        lifecycle.handle(LauncherLifecycleInput::CatalogRecoveryConfirm, &mut effects);
        assert_eq!(
            lifecycle.state(),
            &LauncherLifecycleState::CatalogBuilding {
                mode: CatalogBuildMode::FreshRecovery,
                foreground: true,
                has_stale_catalog: true,
            }
        );
        assert!(
            effects
                .as_slice()
                .iter()
                .any(|effect| matches!(effect, LauncherEffect::StartFreshCatalogBuild { .. }))
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
                transient: true,
            },
        );
    }
}
