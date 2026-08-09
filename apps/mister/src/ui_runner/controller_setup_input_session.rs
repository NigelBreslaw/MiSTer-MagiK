// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Input routing while the controller setup overlay has focus.

use crate::input::PadPool;
use crate::input_state::PadState;
use crate::setup_nav::SetupNav;

pub(super) struct ControllerSetupInputSession<'a> {
    pad: &'a PadPool,
    setup: &'a SetupNav,
}

impl<'a> ControllerSetupInputSession<'a> {
    pub(super) fn new(pad: &'a PadPool, setup: &'a SetupNav) -> Self {
        Self { pad, setup }
    }

    pub(super) fn launcher_state(&self) -> &'a PadState {
        self.pad.state()
    }

    pub(super) fn setup_state(&self) -> PadState {
        self.setup
            .target_device
            .as_ref()
            .and_then(|device| self.pad.navigation_state_for_device(device))
            .unwrap_or_else(|| self.pad.state().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller_db::PadRegistryStatus;
    use crate::input_event::{
        InputEvent, InputPhase, InputSourceId, InputSourceKind, LogicalAction, PressId, SourceEpoch,
    };
    use crate::setup_nav::{SetupAction, SetupPhase};
    use std::time::Instant;

    fn activate_press() -> InputEvent {
        InputEvent {
            source: InputSourceId {
                kind: InputSourceKind::MainProxy,
                instance: 1,
            },
            source_epoch: SourceEpoch(1),
            sequence: 1,
            press_id: PressId(1),
            captured_at_us: 1,
            action: LogicalAction::Activate,
            phase: InputPhase::Pressed,
        }
    }

    trait PadStateTestExt {
        fn with_a(pressed: bool) -> Self;
    }

    impl PadStateTestExt for PadState {
        fn with_a(pressed: bool) -> Self {
            let mut state = PadState {
                btn_a: pressed,
                ..PadState::default()
            };
            state.rebuild_pressed_now();
            state
        }
    }

    struct PadPoolFixture {
        pool: PadPool,
    }

    impl PadPoolFixture {
        fn new(states: Vec<PadState>) -> Self {
            let pool = PadPool::from_test_states(states);
            Self { pool }
        }

        fn device(&self, index: usize) -> crate::input_event::DeviceInstanceId {
            self.pool.device_at(index).expect("test pad identity")
        }
    }

    #[test]
    fn setup_focus_uses_target_pad_state_not_merged_state() {
        let fixture = PadPoolFixture::new(vec![PadState::with_a(true), PadState::with_a(false)]);
        let mut setup = SetupNav::new();
        setup.open_for(PadRegistryStatus::Unknown, fixture.device(1));

        let session = ControllerSetupInputSession::new(&fixture.pool, &setup);

        assert!(session.launcher_state().btn_a);
        assert!(!session.setup_state().btn_a);
    }

    #[test]
    fn setup_focus_uses_new_target_after_advancing() {
        let fixture = PadPoolFixture::new(vec![PadState::with_a(true), PadState::with_a(false)]);
        let mut setup = SetupNav::new();
        setup.open_for(PadRegistryStatus::Unknown, fixture.device(0));
        assert!(
            ControllerSetupInputSession::new(&fixture.pool, &setup)
                .setup_state()
                .btn_a
        );

        setup.open_for(PadRegistryStatus::Unknown, fixture.device(1));
        assert!(
            !ControllerSetupInputSession::new(&fixture.pool, &setup)
                .setup_state()
                .btn_a
        );
    }

    #[test]
    fn inactive_setup_routes_setup_state_to_launcher_state() {
        let fixture = PadPoolFixture::new(vec![PadState::with_a(false), PadState::with_a(true)]);
        let setup = SetupNav::new();

        let session = ControllerSetupInputSession::new(&fixture.pool, &setup);

        assert!(session.launcher_state().btn_a);
        assert!(session.setup_state().btn_a);
    }

    #[test]
    fn setup_does_not_advance_from_non_target_pad_activity() {
        let fixture = PadPoolFixture::new(vec![PadState::with_a(true), PadState::with_a(false)]);
        let mut setup = SetupNav::new();
        setup.open_for(PadRegistryStatus::Unknown, fixture.device(1));

        let setup_state = ControllerSetupInputSession::new(&fixture.pool, &setup)
            .setup_state()
            .clone();
        let action = if setup_state.btn_a {
            setup.handle_action(
                &activate_press(),
                Instant::now(),
                fixture.pool.info_at(1),
                fixture.pool.db(),
            )
        } else {
            SetupAction::None
        };

        assert!(matches!(action, SetupAction::None));
        assert_eq!(setup.phase, SetupPhase::Detected);
    }

    #[test]
    fn setup_advances_from_target_pad_activity() {
        let fixture = PadPoolFixture::new(vec![PadState::with_a(false), PadState::with_a(true)]);
        let mut setup = SetupNav::new();
        setup.open_for(PadRegistryStatus::Unknown, fixture.device(1));

        let setup_state = ControllerSetupInputSession::new(&fixture.pool, &setup)
            .setup_state()
            .clone();
        assert!(setup_state.btn_a);
        let action = setup.handle_action(
            &activate_press(),
            Instant::now(),
            fixture.pool.info_at(1),
            fixture.pool.db(),
        );

        assert!(matches!(action, SetupAction::None));
        assert_eq!(setup.phase, SetupPhase::Configure);
    }

    #[test]
    fn setup_navigation_accepts_keyboard_without_merging_other_pads() {
        let mut fixture = PadPoolFixture::new(vec![PadState::with_a(false)]);
        fixture.pool.set_test_keyboard_state(PadState::with_a(true));
        let mut setup = SetupNav::new();
        setup.open_for(PadRegistryStatus::MovedPort, fixture.device(0));

        assert!(
            ControllerSetupInputSession::new(&fixture.pool, &setup)
                .setup_state()
                .btn_a
        );
    }
}
