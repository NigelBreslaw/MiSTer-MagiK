// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Test-only characterization of the launcher frame pipeline.
//!
//! The production loop records these boundaries only in test builds. The
//! contract deliberately describes current behavior, including early yields
//! and route-specific presentation resolution; it is not a target rewrite.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LauncherFramePhase {
    Begin,
    StartupCatalogReplay,
    LaunchRecoveryApplied,
    PreInputMaintenance,
    InputCaptured,
    InputConsumed,
    InputRouted,
    IdleWait,
    FullScreenTransition,
    FramePlanned,
    FrameSubmitted,
    CompatibilityResolved,
    PostSubmitAccounted,
    ConfirmationInterrupted,
    ActiveConfirmed,
    ReadinessSourceAcknowledged,
    FrameAccounted,
    PresentationAcknowledged,
    FrameFinished,
    Yielded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LauncherFrameScenario {
    RenderedLatch,
    CompatibilityPresentation,
    Idle,
    InputConsumed,
    LaunchRecovery,
    FullScreenTransition,
    ConfirmationInterrupted,
    StartupCatalogReplay,
    ReadinessFromPostedFrame,
}

#[derive(Default)]
pub(super) struct LauncherFramePhaseObserver {
    phases: Vec<LauncherFramePhase>,
}

impl LauncherFramePhaseObserver {
    pub(super) fn record(&mut self, phase: LauncherFramePhase) {
        if phase == LauncherFramePhase::Begin {
            self.phases.clear();
        }
        self.phases.push(phase);
    }

    fn matches(&self, scenario: LauncherFrameScenario) -> bool {
        self.phases == expected_phases(scenario)
    }
}

fn expected_phases(scenario: LauncherFrameScenario) -> &'static [LauncherFramePhase] {
    use LauncherFramePhase as Phase;

    match scenario {
        LauncherFrameScenario::RenderedLatch => &[
            Phase::Begin,
            Phase::PreInputMaintenance,
            Phase::InputCaptured,
            Phase::InputRouted,
            Phase::FramePlanned,
            Phase::FrameSubmitted,
            Phase::PostSubmitAccounted,
            Phase::ActiveConfirmed,
            Phase::FrameAccounted,
            Phase::PresentationAcknowledged,
            Phase::FrameFinished,
        ],
        LauncherFrameScenario::CompatibilityPresentation => &[
            Phase::Begin,
            Phase::PreInputMaintenance,
            Phase::InputCaptured,
            Phase::InputRouted,
            Phase::FramePlanned,
            Phase::FrameSubmitted,
            Phase::CompatibilityResolved,
            Phase::FrameAccounted,
            Phase::PresentationAcknowledged,
            Phase::FrameFinished,
        ],
        LauncherFrameScenario::Idle => &[
            Phase::Begin,
            Phase::PreInputMaintenance,
            Phase::InputCaptured,
            Phase::InputRouted,
            Phase::IdleWait,
            Phase::Yielded,
        ],
        LauncherFrameScenario::InputConsumed => &[
            Phase::Begin,
            Phase::PreInputMaintenance,
            Phase::InputCaptured,
            Phase::InputConsumed,
            Phase::Yielded,
        ],
        LauncherFrameScenario::LaunchRecovery => &[
            Phase::Begin,
            Phase::LaunchRecoveryApplied,
            Phase::PreInputMaintenance,
            Phase::InputCaptured,
            Phase::InputRouted,
            Phase::FramePlanned,
            Phase::FrameSubmitted,
            Phase::PostSubmitAccounted,
            Phase::ActiveConfirmed,
            Phase::FrameAccounted,
            Phase::PresentationAcknowledged,
            Phase::FrameFinished,
        ],
        LauncherFrameScenario::FullScreenTransition => &[
            Phase::Begin,
            Phase::PreInputMaintenance,
            Phase::InputCaptured,
            Phase::InputRouted,
            Phase::FullScreenTransition,
            Phase::FramePlanned,
            Phase::FrameSubmitted,
            Phase::PostSubmitAccounted,
            Phase::ActiveConfirmed,
            Phase::FrameAccounted,
            Phase::PresentationAcknowledged,
            Phase::FrameFinished,
        ],
        LauncherFrameScenario::ConfirmationInterrupted => &[
            Phase::Begin,
            Phase::PreInputMaintenance,
            Phase::InputCaptured,
            Phase::InputRouted,
            Phase::FramePlanned,
            Phase::FrameSubmitted,
            Phase::PostSubmitAccounted,
            Phase::ConfirmationInterrupted,
            Phase::Yielded,
        ],
        LauncherFrameScenario::StartupCatalogReplay => &[
            Phase::Begin,
            Phase::StartupCatalogReplay,
            Phase::PreInputMaintenance,
            Phase::InputCaptured,
            Phase::InputRouted,
            Phase::FramePlanned,
            Phase::FrameSubmitted,
            Phase::PostSubmitAccounted,
            Phase::ActiveConfirmed,
            Phase::FrameAccounted,
            Phase::PresentationAcknowledged,
            Phase::FrameFinished,
        ],
        LauncherFrameScenario::ReadinessFromPostedFrame => &[
            Phase::Begin,
            Phase::PreInputMaintenance,
            Phase::InputCaptured,
            Phase::InputRouted,
            Phase::FramePlanned,
            Phase::FrameSubmitted,
            Phase::PostSubmitAccounted,
            Phase::ActiveConfirmed,
            Phase::ReadinessSourceAcknowledged,
            Phase::FrameAccounted,
            Phase::PresentationAcknowledged,
            Phase::FrameFinished,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(scenario: LauncherFrameScenario) -> LauncherFramePhaseObserver {
        let mut observer = LauncherFramePhaseObserver::default();
        for phase in expected_phases(scenario) {
            observer.record(*phase);
        }
        observer
    }

    #[test]
    fn characterized_paths_accept_their_exact_phase_contract() {
        for scenario in [
            LauncherFrameScenario::RenderedLatch,
            LauncherFrameScenario::CompatibilityPresentation,
            LauncherFrameScenario::Idle,
            LauncherFrameScenario::InputConsumed,
            LauncherFrameScenario::LaunchRecovery,
            LauncherFrameScenario::FullScreenTransition,
            LauncherFrameScenario::ConfirmationInterrupted,
            LauncherFrameScenario::StartupCatalogReplay,
            LauncherFrameScenario::ReadinessFromPostedFrame,
        ] {
            assert!(record(scenario).matches(scenario), "{scenario:?}");
        }
    }

    #[test]
    fn an_input_capture_and_route_swap_violates_the_contract() {
        let scenario = LauncherFrameScenario::RenderedLatch;
        let mut observer = record(scenario);
        let captured = observer
            .phases
            .iter()
            .position(|phase| *phase == LauncherFramePhase::InputCaptured)
            .unwrap();
        let routed = observer
            .phases
            .iter()
            .position(|phase| *phase == LauncherFramePhase::InputRouted)
            .unwrap();
        observer.phases.swap(captured, routed);

        assert!(!observer.matches(scenario));
    }

    #[test]
    fn production_hooks_keep_the_core_boundaries_ordered() {
        let source = include_str!("launcher_loop.rs")
            .split_whitespace()
            .collect::<String>();
        let loop_start = source.find("'launcher:while").expect("launcher loop");
        let source = &source[loop_start..];
        let phases = [
            LauncherFramePhase::Begin,
            LauncherFramePhase::PreInputMaintenance,
            LauncherFramePhase::FramePlanned,
            LauncherFramePhase::FrameSubmitted,
            LauncherFramePhase::FrameAccounted,
            LauncherFramePhase::PresentationAcknowledged,
            LauncherFramePhase::FrameFinished,
        ];
        let mut previous = 0;
        for phase in phases {
            let marker = format!("record_launcher_frame_phase!(LauncherFramePhase::{phase:?})");
            let offset = source[previous..]
                .find(&marker)
                .unwrap_or_else(|| panic!("missing production phase hook {phase:?}"));
            previous += offset + marker.len();
        }
    }

    #[test]
    fn launcher_input_phase_keeps_capture_route_and_yield_inside_one_boundary() {
        let source = include_str!("launcher_loop.rs")
            .split_whitespace()
            .collect::<String>();
        let phase_start = source
            .find("macro_rules!run_launcher_input_phase")
            .expect("launcher input phase start");
        let phase_end = source[phase_start..]
            .find("'launcher:while")
            .map(|offset| phase_start + offset)
            .expect("launcher input phase end");
        for marker in [
            "record_launcher_frame_phase!(LauncherFramePhase::InputCaptured)",
            "record_launcher_frame_phase!(LauncherFramePhase::InputConsumed)",
            "record_launcher_frame_phase!(LauncherFramePhase::InputRouted)",
        ] {
            let offset = source[phase_start..phase_end]
                .find(marker)
                .unwrap_or_else(|| panic!("input phase omitted {marker}"));
            assert!(phase_start + offset < phase_end);
        }
        assert!(source[phase_start..phase_end].contains("(false,input_batch_empty)"));
    }

    #[test]
    fn launcher_input_phase_preserves_router_and_state_parity_operations() {
        let source = include_str!("launcher_loop.rs")
            .split_whitespace()
            .collect::<String>();
        let phase_start = source
            .find("macro_rules!run_launcher_input_phase")
            .expect("launcher input phase start");
        let phase_end = source[phase_start..]
            .find("'launcher:while")
            .map(|offset| phase_start + offset)
            .expect("launcher input phase end");
        let phase = &source[phase_start..phase_end];
        for operation in [
            "pad.drain_input_batch()",
            "input_router.accept_batch(&input_batch)",
            "input_router.consume_remaining_batch(",
            "input_router.set_focus(",
            "input_router.route_event(",
            "input_router.tick_repeat(",
            "launcher_response_trace.observe_state(",
        ] {
            assert!(phase.contains(operation), "input phase omitted {operation}");
        }
    }

    #[test]
    fn early_route_is_selected_before_maintenance_and_current_fallback_remains_after_it() {
        let source = include_str!("launcher_loop.rs")
            .split_whitespace()
            .collect::<String>();
        let loop_start = source.find("'launcher:while").expect("launcher loop");
        let source = &source[loop_start..];
        let early_guard = source
            .find("ifroute_input_early&&input_pending_before_route{")
            .expect("early input selector guard");
        let early_call = source[early_guard..]
            .find("run_launcher_input_phase!(")
            .map(|offset| early_guard + offset)
            .expect("early input phase invocation");
        let maintenance = source
            .find("record_launcher_frame_phase!(LauncherFramePhase::PreInputMaintenance)")
            .expect("pre-input maintenance boundary");
        let fallback = source[maintenance..]
            .find("run_launcher_input_phase!(")
            .map(|offset| maintenance + offset)
            .expect("current input phase fallback");

        assert!(early_guard < early_call);
        assert!(early_call < maintenance);
        assert!(maintenance < fallback);
    }

    #[test]
    fn production_latch_hooks_preserve_account_confirm_and_readiness_order() {
        let source = include_str!("launcher_loop.rs")
            .split_whitespace()
            .collect::<String>();
        let phases = [
            LauncherFramePhase::FrameSubmitted,
            LauncherFramePhase::PostSubmitAccounted,
            LauncherFramePhase::ActiveConfirmed,
            LauncherFramePhase::ReadinessSourceAcknowledged,
            LauncherFramePhase::FrameAccounted,
        ];
        let mut previous = 0;
        for phase in phases {
            let marker = format!("record_launcher_frame_phase!(LauncherFramePhase::{phase:?})");
            let offset = source[previous..]
                .find(&marker)
                .unwrap_or_else(|| panic!("missing production phase hook {phase:?}"));
            previous += offset + marker.len();
        }
    }
}
