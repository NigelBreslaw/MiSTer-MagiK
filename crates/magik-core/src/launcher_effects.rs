// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Narrow, portable capabilities used by launcher orchestration.
//!
//! These contracts describe domain intent. Implementations own device paths,
//! file descriptors, command strings, serialization, and process control.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LauncherEffectFailureKind {
    Rejected,
    TimedOut,
    Unavailable,
    MalformedResponse,
    RecoveryRequired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherEffectFailure {
    kind: LauncherEffectFailureKind,
    detail: String,
    recovery_required: bool,
}

impl LauncherEffectFailure {
    pub fn new(kind: LauncherEffectFailureKind, detail: impl Into<String>) -> Self {
        Self {
            recovery_required: kind == LauncherEffectFailureKind::RecoveryRequired,
            kind,
            detail: detail.into(),
        }
    }

    pub fn with_recovery_required(mut self, recovery_required: bool) -> Self {
        self.recovery_required = recovery_required;
        self
    }

    pub fn kind(&self) -> LauncherEffectFailureKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub fn recovery_required(&self) -> bool {
        self.recovery_required
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchSelection {
    CatalogPath { target: String },
    Structured(StructuredLaunchSelection),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructuredLaunchSelection {
    pub launch_ref: String,
    pub title: String,
    pub system_id: String,
    pub core: String,
    pub payload: String,
    pub mount_kind: String,
    pub mount_index: u8,
    pub delay_secs: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchHandoffRequest {
    pub selection: LaunchSelection,
    pub simple_joystick_handling: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaunchHandoffOutcome {
    pub started_main: bool,
}

pub trait LaunchHandoff {
    fn handoff(
        &mut self,
        request: &LaunchHandoffRequest,
    ) -> Result<LaunchHandoffOutcome, LauncherEffectFailure>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayTransactionPhase {
    Idle,
    Provisional,
    Persisting,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayState {
    pub active_mode: String,
    pub pending_mode: Option<String>,
    pub remaining_secs: u8,
    pub phase: DisplayTransactionPhase,
    pub error: Option<String>,
    pub return_to_settings: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayStateRead {
    Wait,
    Try,
}

pub trait DisplayControl {
    fn state(&mut self, read: DisplayStateRead) -> Result<DisplayState, LauncherEffectFailure>;

    fn apply(&mut self, mode: &str) -> Result<(), LauncherEffectFailure>;

    fn confirm(&mut self) -> Result<(), LauncherEffectFailure>;

    fn cancel(&mut self) -> Result<(), LauncherEffectFailure>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MainRuntimeState {
    pub running: bool,
    pub magik_owned: bool,
    pub arcade_core: bool,
    pub heartbeat_boot_ms: Option<u64>,
}

pub trait RuntimeState {
    fn main_state(&mut self) -> Result<MainRuntimeState, LauncherEffectFailure>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputPolicy {
    Stock,
    Simple,
}

/// Launcher-owned persistence operations.
///
/// Associated types keep application state models out of this portable crate;
/// callers cannot supply paths or arbitrary persistence keys.
pub trait LauncherPersistence {
    type ReturnState;
    type Settings;

    fn load_return_state(&mut self) -> Result<Option<Self::ReturnState>, LauncherEffectFailure>;

    fn save_return_state(&mut self, state: &Self::ReturnState)
    -> Result<(), LauncherEffectFailure>;

    fn clear_return_state(&mut self) -> Result<(), LauncherEffectFailure>;

    fn load_settings(&mut self) -> Result<Self::Settings, LauncherEffectFailure>;

    fn save_settings(&mut self, settings: &Self::Settings) -> Result<(), LauncherEffectFailure>;

    fn set_input_policy(&mut self, policy: InputPolicy) -> Result<(), LauncherEffectFailure>;

    fn request_library_rebuild(&mut self) -> Result<(), LauncherEffectFailure>;

    fn consume_library_rebuild(&mut self) -> Result<bool, LauncherEffectFailure>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_keep_domain_classification_and_recovery_meaning() {
        let failure = LauncherEffectFailure::new(
            LauncherEffectFailureKind::TimedOut,
            "Main handoff timed out",
        )
        .with_recovery_required(true);

        assert_eq!(failure.kind(), LauncherEffectFailureKind::TimedOut);
        assert_eq!(failure.detail(), "Main handoff timed out");
        assert!(failure.recovery_required());
    }
}
