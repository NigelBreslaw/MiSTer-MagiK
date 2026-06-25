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

    pub(super) fn setup_state(&self) -> &'a PadState {
        if self.setup.is_active() {
            self.pad.state_at(self.setup.target_pad_idx)
        } else {
            self.pad.state()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller_db::PadRegistryStatus;
    use crate::setup_nav::{SetupAction, SetupPhase};
    use std::time::Instant;

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
    }

    #[test]
    fn setup_focus_uses_target_pad_state_not_merged_state() {
        let fixture = PadPoolFixture::new(vec![PadState::with_a(true), PadState::with_a(false)]);
        let mut setup = SetupNav::new();
        setup.open_for(PadRegistryStatus::Unknown, 1);

        let session = ControllerSetupInputSession::new(&fixture.pool, &setup);

        assert!(session.launcher_state().btn_a);
        assert!(!session.setup_state().btn_a);
    }

    #[test]
    fn setup_focus_uses_new_target_after_advancing() {
        let fixture = PadPoolFixture::new(vec![PadState::with_a(true), PadState::with_a(false)]);
        let mut setup = SetupNav::new();
        setup.open_for(PadRegistryStatus::Unknown, 0);
        assert!(
            ControllerSetupInputSession::new(&fixture.pool, &setup)
                .setup_state()
                .btn_a
        );

        setup.open_for(PadRegistryStatus::Unknown, 1);
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
        setup.open_for(PadRegistryStatus::Unknown, 1);

        let setup_state = ControllerSetupInputSession::new(&fixture.pool, &setup)
            .setup_state()
            .clone();
        let action = setup.handle_input(
            &setup_state,
            Instant::now(),
            fixture.pool.info_at(1),
            fixture.pool.db(),
        );

        assert!(matches!(action, SetupAction::None));
        assert_eq!(setup.phase, SetupPhase::Detected);
    }

    #[test]
    fn setup_advances_from_target_pad_activity() {
        let fixture = PadPoolFixture::new(vec![PadState::with_a(false), PadState::with_a(true)]);
        let mut setup = SetupNav::new();
        setup.open_for(PadRegistryStatus::Unknown, 1);

        let setup_state = ControllerSetupInputSession::new(&fixture.pool, &setup)
            .setup_state()
            .clone();
        let action = setup.handle_input(
            &setup_state,
            Instant::now(),
            fixture.pool.info_at(1),
            fixture.pool.db(),
        );

        assert!(matches!(action, SetupAction::None));
        assert_eq!(setup.phase, SetupPhase::Configure);
    }
}
