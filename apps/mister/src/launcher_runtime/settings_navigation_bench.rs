// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fixed real-screen Settings navigation benchmark route.

use crate::input_state::PadState;
use crate::launcher::Screen;
use crate::launcher_runtime::navigation_transition::{
    NavigationTransitionDirection, NavigationTransitionRoute,
};
use crate::settings::ScreenOrientation;
use std::time::{Duration, Instant};

const ROUTE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsNavigationLeg {
    pub route: NavigationTransitionRoute,
    pub direction: NavigationTransitionDirection,
    pub source: Screen,
    pub destination: Screen,
}

pub const SETTINGS_NAVIGATION_ROUTE: [SettingsNavigationLeg; 6] = [
    SettingsNavigationLeg {
        route: NavigationTransitionRoute::HomeToSettings,
        direction: NavigationTransitionDirection::Forward,
        source: Screen::Home,
        destination: Screen::Settings,
    },
    SettingsNavigationLeg {
        route: NavigationTransitionRoute::SettingsToAbout,
        direction: NavigationTransitionDirection::Forward,
        source: Screen::Settings,
        destination: Screen::About,
    },
    SettingsNavigationLeg {
        route: NavigationTransitionRoute::AboutToInfo,
        direction: NavigationTransitionDirection::Forward,
        source: Screen::About,
        destination: Screen::Info,
    },
    SettingsNavigationLeg {
        route: NavigationTransitionRoute::AboutToInfo,
        direction: NavigationTransitionDirection::Reverse,
        source: Screen::Info,
        destination: Screen::About,
    },
    SettingsNavigationLeg {
        route: NavigationTransitionRoute::SettingsToAbout,
        direction: NavigationTransitionDirection::Reverse,
        source: Screen::About,
        destination: Screen::Settings,
    },
    SettingsNavigationLeg {
        route: NavigationTransitionRoute::HomeToSettings,
        direction: NavigationTransitionDirection::Reverse,
        source: Screen::Settings,
        destination: Screen::Home,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BenchmarkButton {
    Up,
    Down,
    A,
    B,
    Home,
}

impl BenchmarkButton {
    fn pad_state(self) -> PadState {
        let mut state = PadState::default();
        match self {
            Self::Up => state.dpad_up = true,
            Self::Down => state.dpad_down = true,
            Self::A => state.btn_a = true,
            Self::B => state.btn_b = true,
            Self::Home => state.btn_home = true,
        }
        state
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsNavigationRecord {
    pub leg: SettingsNavigationLeg,
    pub start_frame: u64,
    pub rendered_endpoint_frame: u64,
    pub presented_endpoint_frame: u64,
    pub presented_sequence: u16,
}

#[derive(Debug)]
pub struct SettingsNavigationBenchmark {
    enabled: bool,
    orientation: ScreenOrientation,
    started: Instant,
    release_pending: bool,
    active: Option<SettingsNavigationRecord>,
    records: Vec<SettingsNavigationRecord>,
    failure: Option<&'static str>,
}

impl SettingsNavigationBenchmark {
    pub fn new(enabled: bool, orientation: ScreenOrientation) -> Self {
        Self {
            enabled,
            orientation,
            started: Instant::now(),
            release_pending: false,
            active: None,
            records: Vec::new(),
            failure: None,
        }
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub const fn orientation(&self) -> ScreenOrientation {
        self.orientation
    }

    pub fn input_for(
        &mut self,
        screen: Screen,
        settings_focused: bool,
        settings_selected: usize,
        full_screen_live: bool,
    ) -> Option<PadState> {
        if !self.enabled || self.complete() || self.failed() {
            return None;
        }
        if self.started.elapsed() >= ROUTE_TIMEOUT {
            self.fail("route-timeout");
            return None;
        }
        if self.release_pending {
            self.release_pending = false;
            return Some(PadState::default());
        }
        if !full_screen_live || self.active.is_some() {
            return Some(PadState::default());
        }
        let button = match (self.records.len(), screen) {
            (0, Screen::Home) if self.orientation == ScreenOrientation::Normal => {
                if settings_focused {
                    BenchmarkButton::A
                } else {
                    BenchmarkButton::Up
                }
            }
            (0, Screen::Home) => BenchmarkButton::Home,
            (1, Screen::Settings) => {
                if settings_selected < 6 {
                    BenchmarkButton::Down
                } else {
                    BenchmarkButton::A
                }
            }
            (2, Screen::About) => BenchmarkButton::A,
            (3, Screen::Info) | (4, Screen::About) | (5, Screen::Settings) => BenchmarkButton::B,
            _ => return Some(PadState::default()),
        };
        self.release_pending = true;
        Some(button.pad_state())
    }

    pub fn note_started(
        &mut self,
        route: NavigationTransitionRoute,
        direction: NavigationTransitionDirection,
        source: Screen,
        destination: Screen,
        frame: u64,
    ) {
        if !self.enabled || self.failed() {
            return;
        }
        let Some(expected) = SETTINGS_NAVIGATION_ROUTE.get(self.records.len()).copied() else {
            self.fail("unexpected-extra-leg");
            return;
        };
        if self.active.is_some()
            || expected.route != route
            || expected.direction != direction
            || expected.source != source
            || expected.destination != destination
        {
            self.fail("unexpected-leg");
            return;
        }
        self.active = Some(SettingsNavigationRecord {
            leg: expected,
            start_frame: frame,
            rendered_endpoint_frame: 0,
            presented_endpoint_frame: 0,
            presented_sequence: 0,
        });
    }

    pub fn note_rendered_endpoint(&mut self, frame: u64) {
        if let Some(active) = self.active.as_mut() {
            active.rendered_endpoint_frame = frame;
        }
    }

    pub fn note_confirmed_presentation(
        &mut self,
        screen: Screen,
        frame: u64,
        sequence: u16,
    ) -> Option<SettingsNavigationRecord> {
        if !self.enabled || self.failed() {
            return None;
        }
        let Some(mut active) = self.active.take() else {
            return None;
        };
        if screen != active.leg.destination
            || active.rendered_endpoint_frame == 0
            || frame < active.rendered_endpoint_frame
            || sequence == 0
        {
            self.fail("endpoint-confirmation-mismatch");
            return None;
        }
        active.presented_endpoint_frame = frame;
        active.presented_sequence = sequence;
        self.records.push(active);
        Some(active)
    }

    pub const fn active_leg(&self) -> u8 {
        if self.active.is_some() {
            self.records.len() as u8 + 1
        } else {
            0
        }
    }

    pub fn complete(&self) -> bool {
        self.enabled
            && self.failure.is_none()
            && self.active.is_none()
            && self.records.len() == SETTINGS_NAVIGATION_ROUTE.len()
    }

    pub const fn failed(&self) -> bool {
        self.failure.is_some()
    }

    pub const fn failure(&self) -> Option<&'static str> {
        self.failure
    }

    pub fn records(&self) -> &[SettingsNavigationRecord] {
        &self.records
    }

    pub fn fail(&mut self, reason: &'static str) {
        if self.failure.is_none() {
            self.failure = Some(reason);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pressed(state: &PadState) -> &'static str {
        if state.dpad_up {
            "up"
        } else if state.dpad_down {
            "down"
        } else if state.btn_a {
            "a"
        } else if state.btn_b {
            "b"
        } else if state.btn_home {
            "home"
        } else {
            "released"
        }
    }

    #[test]
    fn landscape_enters_settings_through_focus_and_activate() {
        let mut benchmark = SettingsNavigationBenchmark::new(true, ScreenOrientation::Normal);
        assert_eq!(
            pressed(&benchmark.input_for(Screen::Home, false, 0, true).unwrap()),
            "up"
        );
        assert_eq!(
            pressed(&benchmark.input_for(Screen::Home, true, 0, true).unwrap()),
            "released"
        );
        assert_eq!(
            pressed(&benchmark.input_for(Screen::Home, true, 0, true).unwrap()),
            "a"
        );
    }

    #[test]
    fn portrait_enters_settings_with_home() {
        let mut benchmark =
            SettingsNavigationBenchmark::new(true, ScreenOrientation::MonitorCounterclockwise);
        assert_eq!(
            pressed(&benchmark.input_for(Screen::Home, false, 0, true).unwrap()),
            "home"
        );
    }

    #[test]
    fn route_requires_ordered_rendered_and_presented_endpoints() {
        let mut benchmark = SettingsNavigationBenchmark::new(true, ScreenOrientation::Normal);
        let leg = SETTINGS_NAVIGATION_ROUTE[0];
        benchmark.note_started(leg.route, leg.direction, leg.source, leg.destination, 10);
        assert_eq!(benchmark.active_leg(), 1);
        benchmark.note_rendered_endpoint(20);
        let record = benchmark
            .note_confirmed_presentation(Screen::Settings, 21, 7)
            .unwrap();
        assert_eq!(record.presented_sequence, 7);
        assert_eq!(benchmark.records().len(), 1);
        assert!(!benchmark.failed());
    }

    #[test]
    fn wrong_route_fails_without_recording() {
        let mut benchmark = SettingsNavigationBenchmark::new(true, ScreenOrientation::Normal);
        benchmark.note_started(
            NavigationTransitionRoute::SettingsToAbout,
            NavigationTransitionDirection::Forward,
            Screen::Home,
            Screen::Settings,
            10,
        );
        assert!(benchmark.failed());
        assert!(benchmark.records().is_empty());
    }
}
