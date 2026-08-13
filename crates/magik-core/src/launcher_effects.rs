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

    #[derive(Default)]
    struct FakeEffects {
        handoffs: Vec<LaunchHandoffRequest>,
        display_mode: String,
        runtime: MainRuntimeState,
        return_state: Option<String>,
        settings: bool,
        policies: Vec<InputPolicy>,
        rebuild_requested: bool,
    }

    impl LaunchHandoff for FakeEffects {
        fn handoff(
            &mut self,
            request: &LaunchHandoffRequest,
        ) -> Result<LaunchHandoffOutcome, LauncherEffectFailure> {
            self.handoffs.push(request.clone());
            Ok(LaunchHandoffOutcome {
                started_main: false,
            })
        }
    }

    impl DisplayControl for FakeEffects {
        fn state(
            &mut self,
            _read: DisplayStateRead,
        ) -> Result<DisplayState, LauncherEffectFailure> {
            Ok(DisplayState {
                active_mode: self.display_mode.clone(),
                pending_mode: None,
                remaining_secs: 0,
                phase: DisplayTransactionPhase::Idle,
                error: None,
                return_to_settings: false,
            })
        }

        fn apply(&mut self, mode: &str) -> Result<(), LauncherEffectFailure> {
            self.display_mode = mode.to_string();
            Ok(())
        }

        fn confirm(&mut self) -> Result<(), LauncherEffectFailure> {
            Ok(())
        }

        fn cancel(&mut self) -> Result<(), LauncherEffectFailure> {
            Ok(())
        }
    }

    impl RuntimeState for FakeEffects {
        fn main_state(&mut self) -> Result<MainRuntimeState, LauncherEffectFailure> {
            Ok(self.runtime)
        }
    }

    impl LauncherPersistence for FakeEffects {
        type ReturnState = String;
        type Settings = bool;

        fn load_return_state(
            &mut self,
        ) -> Result<Option<Self::ReturnState>, LauncherEffectFailure> {
            Ok(self.return_state.clone())
        }

        fn save_return_state(
            &mut self,
            state: &Self::ReturnState,
        ) -> Result<(), LauncherEffectFailure> {
            self.return_state = Some(state.clone());
            Ok(())
        }

        fn clear_return_state(&mut self) -> Result<(), LauncherEffectFailure> {
            self.return_state = None;
            Ok(())
        }

        fn load_settings(&mut self) -> Result<Self::Settings, LauncherEffectFailure> {
            Ok(self.settings)
        }

        fn save_settings(
            &mut self,
            settings: &Self::Settings,
        ) -> Result<(), LauncherEffectFailure> {
            self.settings = *settings;
            Ok(())
        }

        fn set_input_policy(&mut self, policy: InputPolicy) -> Result<(), LauncherEffectFailure> {
            self.policies.push(policy);
            Ok(())
        }

        fn request_library_rebuild(&mut self) -> Result<(), LauncherEffectFailure> {
            self.rebuild_requested = true;
            Ok(())
        }

        fn consume_library_rebuild(&mut self) -> Result<bool, LauncherEffectFailure> {
            Ok(std::mem::take(&mut self.rebuild_requested))
        }
    }

    #[test]
    fn narrow_fakes_run_without_platform_io() {
        let mut effects = FakeEffects::default();
        let request = LaunchHandoffRequest {
            selection: LaunchSelection::CatalogPath {
                target: "arcade:test".to_string(),
            },
            simple_joystick_handling: false,
        };
        assert_eq!(
            effects.handoff(&request).unwrap(),
            LaunchHandoffOutcome {
                started_main: false
            }
        );

        effects.apply("hdmi-720p60").unwrap();
        assert_eq!(
            effects.state(DisplayStateRead::Try).unwrap().active_mode,
            "hdmi-720p60"
        );

        effects
            .save_return_state(&"return-state".to_string())
            .unwrap();
        assert_eq!(
            effects.load_return_state().unwrap().as_deref(),
            Some("return-state")
        );
        effects.set_input_policy(InputPolicy::Simple).unwrap();
        effects.request_library_rebuild().unwrap();
        assert!(effects.consume_library_rebuild().unwrap());
        assert!(!effects.consume_library_rebuild().unwrap());

        assert_eq!(effects.handoffs, [request]);
        assert_eq!(effects.policies, [InputPolicy::Simple]);
    }

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
