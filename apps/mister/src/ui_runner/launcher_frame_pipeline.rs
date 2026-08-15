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
        let phases = [
            LauncherFramePhase::Begin,
            LauncherFramePhase::PreInputMaintenance,
            LauncherFramePhase::InputCaptured,
            LauncherFramePhase::InputRouted,
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
