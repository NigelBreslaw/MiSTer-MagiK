// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Timed confirmation state for display-resolution and screen-orientation
//! changes. Persistence runs on short-lived worker threads; the launcher loop
//! drains their results once per frame and owns any orientation transition.

use crate::launcher::{self, LauncherNav, Screen};
use crate::ui_display::ScreenOrientation;
use mister_magik_fb::launcher_runtime::settings::ConfirmedOrientationStore;
use std::sync::mpsc;
use std::time::{Duration, Instant};

type DisplayConfirmResult = Result<launcher::DisplayCommandState, String>;
type OrientationConfirmResult = Result<(), String>;

pub(super) fn display_confirmation_ui_enabled(value: Option<&std::ffi::OsStr>) -> bool {
    value != Some(std::ffi::OsStr::new("0"))
}

fn confirm_deadline_after(now: Instant, seconds: u8) -> Instant {
    now + Duration::from_secs(u64::from(seconds))
}

fn remaining_confirm_seconds(deadline: Instant, now: Instant) -> u8 {
    if now >= deadline {
        0
    } else {
        ((deadline - now).as_millis().div_ceil(1000) as u8).min(launcher::DISPLAY_CONFIRM_SECONDS)
    }
}

pub(super) struct DisplayConfirmation {
    deadline: Option<Instant>,
    result_tx: mpsc::Sender<DisplayConfirmResult>,
    result_rx: mpsc::Receiver<DisplayConfirmResult>,
}

impl DisplayConfirmation {
    pub(super) fn new() -> Self {
        let (result_tx, result_rx) = mpsc::channel();
        Self {
            deadline: None,
            result_tx,
            result_rx,
        }
    }

    /// Adopts a pending resolution reported by Main at launcher start.
    pub(super) fn adopt_startup_pending(
        &mut self,
        nav: &mut LauncherNav,
        state: &launcher::DisplayCommandState,
        confirmation_ui_enabled: bool,
        now: Instant,
    ) {
        if state.pending.is_none() || !confirmation_ui_enabled {
            return;
        }
        nav.screen = Screen::Settings;
        nav.settings_selected = 0;
        self.arm(nav, state.remaining, now);
    }

    fn arm(&mut self, nav: &mut LauncherNav, remaining: u8, now: Instant) {
        let remaining = remaining.max(1);
        nav.confirm_action = Some(launcher::ConfirmAction::DisplayResolution);
        nav.confirm_selected = 0;
        nav.display_confirm_remaining = remaining;
        self.deadline = Some(confirm_deadline_after(now, remaining));
    }

    pub(super) fn update_remaining(&self, nav: &mut LauncherNav, now: Instant) {
        if let Some(deadline) = self.deadline {
            nav.display_confirm_remaining = remaining_confirm_seconds(deadline, now);
        }
    }

    pub(super) fn begin_confirm(&self, nav: &mut LauncherNav) {
        nav.display_confirm_busy = true;
        nav.display_error = None;
        nav.confirm_action = Some(launcher::ConfirmAction::DisplayResolution);
        let result_tx = self.result_tx.clone();
        std::thread::spawn(move || {
            let result = launcher::confirm_display_resolution_and_wait(Duration::from_secs(12));
            let _ = result_tx.send(result);
        });
    }

    pub(super) fn try_recv(&self) -> Option<DisplayConfirmResult> {
        self.result_rx.try_recv().ok()
    }

    pub(super) fn apply_result(
        &mut self,
        nav: &mut LauncherNav,
        result: DisplayConfirmResult,
        now: Instant,
    ) {
        nav.display_confirm_busy = false;
        match result {
            Ok(state) => {
                if state.phase == launcher::DisplayTransactionPhase::Failed {
                    nav.display_error = Some(
                        state
                            .error
                            .unwrap_or_else(|| "display persistence failed".to_string()),
                    );
                    self.arm(nav, state.remaining, now);
                } else {
                    nav.confirm_action = None;
                    nav.display_error = None;
                    self.deadline = None;
                    if let Some(index) =
                        mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS
                            .iter()
                            .position(|mode| mode.id == state.active)
                    {
                        nav.display_selected = index;
                        nav.display_highlighted =
                            launcher::settings_display_selection_index(index).unwrap_or(0);
                    }
                }
            }
            Err(error) => {
                nav.confirm_action = Some(launcher::ConfirmAction::DisplayResolution);
                nav.confirm_selected = 0;
                nav.display_error = Some(error);
            }
        }
    }
}

pub(super) struct OrientationConfirmation {
    deadline: Option<Instant>,
    previous: Option<ScreenOrientation>,
    store: ConfirmedOrientationStore,
    result_tx: mpsc::Sender<OrientationConfirmResult>,
    result_rx: mpsc::Receiver<OrientationConfirmResult>,
}

impl OrientationConfirmation {
    pub(super) fn new(store: ConfirmedOrientationStore) -> Self {
        let (result_tx, result_rx) = mpsc::channel();
        Self {
            deadline: None,
            previous: None,
            store,
            result_tx,
            result_rx,
        }
    }

    /// Updates the countdown. Returns true once the countdown has expired on
    /// the orientation dialog; the caller then calls `finish_expired`.
    pub(super) fn update_remaining(&self, nav: &mut LauncherNav, now: Instant) -> bool {
        let Some(deadline) = self.deadline else {
            return false;
        };
        nav.orientation_confirm_remaining = remaining_confirm_seconds(deadline, now);
        now >= deadline && nav.confirm_action == Some(launcher::ConfirmAction::ScreenOrientation)
    }

    /// Closes the expired dialog and returns the orientation to roll back to.
    pub(super) fn finish_expired(&mut self, nav: &mut LauncherNav) -> Option<ScreenOrientation> {
        self.deadline = None;
        nav.confirm_action = None;
        nav.confirm_selected = 0;
        nav.orientation_confirm_remaining = 0;
        self.previous.take()
    }

    /// Records the orientation to restore and shows the confirmation dialog.
    /// The countdown starts once the transition to the new orientation ends.
    pub(super) fn begin_apply(&mut self, nav: &mut LauncherNav, previous: ScreenOrientation) {
        self.previous = Some(previous);
        nav.orientation_confirm_busy = false;
        nav.orientation_error = None;
        nav.confirm_action = Some(launcher::ConfirmAction::ScreenOrientation);
        nav.confirm_selected = 0;
        nav.orientation_confirm_remaining = launcher::DISPLAY_CONFIRM_SECONDS;
        self.deadline = None;
    }

    pub(super) fn start_countdown(&mut self, now: Instant) {
        self.deadline = Some(confirm_deadline_after(
            now,
            launcher::DISPLAY_CONFIRM_SECONDS,
        ));
    }

    pub(super) fn begin_confirm(&mut self, nav: &mut LauncherNav) {
        self.deadline = None;
        nav.orientation_confirm_remaining = 0;
        nav.orientation_confirm_busy = true;
        nav.orientation_error = None;
        nav.confirm_action = Some(launcher::ConfirmAction::ScreenOrientation);
        nav.confirm_selected = 1;
        let confirmed = nav.settings.clone();
        let mut previous = confirmed.clone();
        previous.screen_orientation = self.previous.unwrap_or(confirmed.screen_orientation);
        let result_tx = self.result_tx.clone();
        let store = self.store.clone();
        std::thread::spawn(move || {
            let result = store
                .save_confirmed(&previous, &confirmed)
                .map_err(|error| error.to_string());
            let _ = result_tx.send(result);
        });
    }

    /// Clears the cancelled dialog and returns the orientation to roll back to.
    pub(super) fn finish_cancel(&mut self, nav: &mut LauncherNav) -> Option<ScreenOrientation> {
        self.deadline = None;
        nav.orientation_confirm_remaining = 0;
        nav.orientation_confirm_busy = false;
        nav.orientation_error = None;
        self.previous.take()
    }

    pub(super) fn try_recv(&self) -> Option<OrientationConfirmResult> {
        self.result_rx.try_recv().ok()
    }

    pub(super) fn apply_result(&mut self, nav: &mut LauncherNav, result: OrientationConfirmResult) {
        nav.orientation_confirm_busy = false;
        match result {
            Ok(()) => {
                self.previous = None;
                nav.confirm_action = None;
                nav.confirm_selected = 0;
                nav.orientation_error = None;
                nav.orientation_confirm_remaining = 0;
            }
            Err(error) => {
                nav.confirm_action = Some(launcher::ConfirmAction::ScreenOrientation);
                nav.confirm_selected = 1;
                nav.orientation_error = Some(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> ConfirmedOrientationStore {
        ConfirmedOrientationStore::with_mister_ini_path(
            mister_magik_fb::launcher_runtime::settings::FileSettingsStore::new(
                std::env::temp_dir().join("launcher-confirmation-unused-settings.json"),
            ),
            None,
        )
    }

    #[test]
    fn remaining_seconds_round_up_and_clamp_to_the_dialog_length() {
        let now = Instant::now();
        assert_eq!(remaining_confirm_seconds(now, now), 0);
        assert_eq!(
            remaining_confirm_seconds(now + Duration::from_millis(1), now),
            1
        );
        assert_eq!(
            remaining_confirm_seconds(now + Duration::from_millis(4_001), now),
            5
        );
        assert_eq!(
            remaining_confirm_seconds(now + Duration::from_secs(600), now),
            launcher::DISPLAY_CONFIRM_SECONDS
        );
    }

    #[test]
    fn failed_display_confirmation_rearms_the_countdown() {
        let now = Instant::now();
        let mut nav = LauncherNav::default();
        let mut confirmation = DisplayConfirmation::new();
        confirmation.apply_result(
            &mut nav,
            Ok(launcher::DisplayCommandState {
                active: "hdmi-720p".to_owned(),
                pending: None,
                remaining: 0,
                phase: launcher::DisplayTransactionPhase::Failed,
                error: None,
                return_to_settings: false,
            }),
            now,
        );
        assert_eq!(
            nav.confirm_action,
            Some(launcher::ConfirmAction::DisplayResolution)
        );
        assert_eq!(
            nav.display_error.as_deref(),
            Some("display persistence failed")
        );
        assert_eq!(nav.display_confirm_remaining, 1);
        confirmation.update_remaining(&mut nav, now + Duration::from_secs(2));
        assert_eq!(nav.display_confirm_remaining, 0);
    }

    #[test]
    fn orientation_countdown_expires_only_on_the_orientation_dialog() {
        let now = Instant::now();
        let mut nav = LauncherNav::default();
        let mut confirmation = OrientationConfirmation::new(test_store());
        confirmation.begin_apply(&mut nav, ScreenOrientation::MonitorClockwise);
        assert_eq!(
            nav.confirm_action,
            Some(launcher::ConfirmAction::ScreenOrientation)
        );
        assert_eq!(nav.confirm_selected, 0);
        assert!(!confirmation.update_remaining(&mut nav, now + Duration::from_secs(60)));

        confirmation.start_countdown(now);
        assert!(!confirmation.update_remaining(&mut nav, now));
        assert_eq!(
            nav.orientation_confirm_remaining,
            launcher::DISPLAY_CONFIRM_SECONDS
        );

        let expired = now + Duration::from_secs(u64::from(launcher::DISPLAY_CONFIRM_SECONDS));
        nav.confirm_action = None;
        assert!(!confirmation.update_remaining(&mut nav, expired));
        nav.confirm_action = Some(launcher::ConfirmAction::ScreenOrientation);
        assert!(confirmation.update_remaining(&mut nav, expired));
        assert_eq!(
            confirmation.finish_expired(&mut nav),
            Some(ScreenOrientation::MonitorClockwise)
        );
        assert_eq!(nav.confirm_action, None);
        assert!(!confirmation.update_remaining(&mut nav, expired));
    }

    #[test]
    fn successful_orientation_save_forgets_the_rollback_target() {
        let mut nav = LauncherNav::default();
        let mut confirmation = OrientationConfirmation::new(test_store());
        confirmation.begin_apply(&mut nav, ScreenOrientation::MonitorClockwise);
        confirmation.apply_result(&mut nav, Err("disk full".to_owned()));
        assert_eq!(nav.orientation_error.as_deref(), Some("disk full"));
        assert_eq!(nav.confirm_selected, 1);

        confirmation.apply_result(&mut nav, Ok(()));
        assert_eq!(nav.confirm_action, None);
        assert_eq!(nav.orientation_error, None);
        assert_eq!(confirmation.finish_cancel(&mut nav), None);
    }

    #[test]
    fn startup_pending_display_only_enters_confirmation_for_the_ui_route() {
        let state = launcher::DisplayCommandState {
            active: "hdmi-1920x1080p60".to_string(),
            pending: Some("hdmi-1280x720p60".to_string()),
            remaining: launcher::DISPLAY_CONFIRM_SECONDS,
            phase: launcher::DisplayTransactionPhase::Provisional,
            error: None,
            return_to_settings: false,
        };
        let now = Instant::now();
        let mut ui_nav = LauncherNav::new();
        let mut confirmation = DisplayConfirmation::new();
        confirmation.adopt_startup_pending(&mut ui_nav, &state, true, now);
        assert_eq!(ui_nav.screen, Screen::Settings);
        assert_eq!(
            ui_nav.confirm_action,
            Some(launcher::ConfirmAction::DisplayResolution)
        );
        assert_eq!(
            ui_nav.display_confirm_remaining,
            launcher::DISPLAY_CONFIRM_SECONDS
        );
        assert_eq!(
            confirmation.deadline,
            Some(now + Duration::from_secs(u64::from(launcher::DISPLAY_CONFIRM_SECONDS)))
        );

        let mut headless_nav = LauncherNav::new();
        let mut headless = DisplayConfirmation::new();
        headless.adopt_startup_pending(&mut headless_nav, &state, false, now);
        assert_eq!(headless.deadline, None);
        assert_eq!(headless_nav.screen, Screen::Home);
        assert_eq!(headless_nav.confirm_action, None);
        assert!(!display_confirmation_ui_enabled(Some(
            std::ffi::OsStr::new("0")
        )));
        assert!(display_confirmation_ui_enabled(None));
    }
}
