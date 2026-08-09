// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fixed real-screen Settings navigation benchmark route.

use crate::input_event::{
    InputEvent, InputPhase, InputSourceId, InputSourceKind, LogicalAction, PressId, SourceEpoch,
};
use crate::launcher::Screen;
use crate::launcher_runtime::navigation_transition::{
    NavigationTransitionDirection, NavigationTransitionRoute,
};
use crate::settings::ScreenOrientation;
use mister_magik_latch_contract::PresentationTelemetry;
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

pub const SETTINGS_NAVIGATION_ORIENTATIONS: [ScreenOrientation; 2] = [
    ScreenOrientation::Normal,
    ScreenOrientation::MonitorCounterclockwise,
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
    const fn action(self) -> LogicalAction {
        match self {
            Self::Up => LogicalAction::Up,
            Self::Down => LogicalAction::Down,
            Self::A => LogicalAction::Activate,
            Self::B => LogicalAction::Back,
            Self::Home => LogicalAction::Home,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsNavigationPresentationCapture {
    pub telemetry: PresentationTelemetry,
    pub captured_at: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsNavigationRecord {
    pub orientation: ScreenOrientation,
    pub leg: SettingsNavigationLeg,
    pub start_frame: u64,
    pub rendered_endpoint_frame: u64,
    pub presented_endpoint_frame: u64,
    pub presented_sequence: u16,
    pub presentation_start: Option<SettingsNavigationPresentationCapture>,
    pub presentation_end: Option<SettingsNavigationPresentationCapture>,
    pub presentation_elapsed_us: Option<u64>,
    pub presentation_error: Option<String>,
}

#[derive(Debug)]
pub struct SettingsNavigationBenchmark {
    enabled: bool,
    orientation_index: usize,
    orientation_ready: bool,
    started: Instant,
    release_pending: bool,
    active_press: Option<(BenchmarkButton, PressId)>,
    next_sequence: u64,
    next_press_id: u64,
    active: Option<SettingsNavigationRecord>,
    records: Vec<SettingsNavigationRecord>,
    failure: Option<&'static str>,
}

impl SettingsNavigationBenchmark {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            orientation_index: 0,
            orientation_ready: true,
            started: Instant::now(),
            release_pending: false,
            active_press: None,
            next_sequence: 0,
            next_press_id: 0,
            active: None,
            records: Vec::new(),
            failure: None,
        }
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub const fn orientation(&self) -> ScreenOrientation {
        SETTINGS_NAVIGATION_ORIENTATIONS[self.orientation_index]
    }

    pub fn take_orientation_change(
        &mut self,
        screen: Screen,
        full_screen_live: bool,
    ) -> Option<ScreenOrientation> {
        if !self.enabled
            || self.failed()
            || self.active.is_some()
            || self.records.len() != SETTINGS_NAVIGATION_ROUTE.len()
            || self.orientation_index != 0
            || screen != Screen::Home
            || !full_screen_live
        {
            return None;
        }
        self.orientation_index = 1;
        self.orientation_ready = false;
        Some(self.orientation())
    }

    pub fn note_orientation_presented(&mut self, orientation: ScreenOrientation) {
        if self.enabled
            && !self.orientation_ready
            && self.orientation_index == 1
            && orientation == self.orientation()
        {
            self.orientation_ready = true;
        }
    }

    pub fn event_for(
        &mut self,
        screen: Screen,
        settings_focused: bool,
        settings_selected: usize,
        full_screen_live: bool,
        captured_at_us: u64,
    ) -> Option<InputEvent> {
        if !self.enabled || self.complete() || self.failed() {
            return None;
        }
        if self.started.elapsed() >= ROUTE_TIMEOUT {
            self.fail("route-timeout");
            return None;
        }
        if self.release_pending {
            self.release_pending = false;
            let (button, press_id) = self.active_press.take()?;
            return Some(self.input_event(button, press_id, InputPhase::Released, captured_at_us));
        }
        if !full_screen_live || self.active.is_some() || !self.orientation_ready {
            return None;
        }
        let leg_index = self.records.len() % SETTINGS_NAVIGATION_ROUTE.len();
        let button = match (leg_index, screen) {
            (0, Screen::Home) if self.orientation() == ScreenOrientation::Normal => {
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
            _ => return None,
        };
        self.release_pending = true;
        self.next_press_id = self.next_press_id.saturating_add(1).max(1);
        let press_id = PressId((1_u64 << 61) | self.next_press_id);
        self.active_press = Some((button, press_id));
        Some(self.input_event(button, press_id, InputPhase::Pressed, captured_at_us))
    }

    fn input_event(
        &mut self,
        button: BenchmarkButton,
        press_id: PressId,
        phase: InputPhase,
        captured_at_us: u64,
    ) -> InputEvent {
        self.next_sequence = self.next_sequence.saturating_add(1).max(1);
        InputEvent {
            source: InputSourceId {
                kind: InputSourceKind::Automation,
                instance: 3,
            },
            source_epoch: SourceEpoch(1),
            sequence: self.next_sequence,
            press_id,
            captured_at_us,
            action: button.action(),
            phase,
        }
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
        let Some(expected) = SETTINGS_NAVIGATION_ROUTE
            .get(self.records.len() % SETTINGS_NAVIGATION_ROUTE.len())
            .copied()
        else {
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
            orientation: self.orientation(),
            leg: expected,
            start_frame: frame,
            rendered_endpoint_frame: 0,
            presented_endpoint_frame: 0,
            presented_sequence: 0,
            presentation_start: None,
            presentation_end: None,
            presentation_elapsed_us: None,
            presentation_error: None,
        });
    }

    pub fn capture_presentation_start(
        &mut self,
        captured_at: Instant,
        telemetry: std::io::Result<PresentationTelemetry>,
    ) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        match telemetry {
            Ok(telemetry) => {
                active.presentation_start = Some(SettingsNavigationPresentationCapture {
                    telemetry,
                    captured_at,
                });
            }
            Err(error) => active.presentation_error = Some(error.to_string()),
        }
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
        captured_at: Instant,
        telemetry: std::io::Result<PresentationTelemetry>,
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
        match telemetry {
            Ok(telemetry) => {
                active.presentation_end = Some(SettingsNavigationPresentationCapture {
                    telemetry,
                    captured_at,
                });
                if let Some(start) = active.presentation_start {
                    active.presentation_elapsed_us = Some(
                        captured_at
                            .saturating_duration_since(start.captured_at)
                            .as_micros()
                            .min(u128::from(u64::MAX)) as u64,
                    );
                } else {
                    active.presentation_error =
                        Some("presentation telemetry start was not captured".to_string());
                }
            }
            Err(error) => {
                active.presentation_error = Some(error.to_string());
            }
        }
        self.records.push(active);
        self.records.last().cloned()
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
            && self.records.len()
                == SETTINGS_NAVIGATION_ROUTE.len() * SETTINGS_NAVIGATION_ORIENTATIONS.len()
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

    fn presentation_telemetry(count: u32) -> PresentationTelemetry {
        PresentationTelemetry {
            owned_vblank_count: count,
            presented_vblank_count: count,
            repeated_vblank_count: 0,
            ownership_loss_count: 0,
            active_sequence: u16::try_from(count).unwrap_or(u16::MAX),
            flags: 1 << 3,
            crc: 0,
        }
    }

    fn pressed(event: &InputEvent) -> &'static str {
        if event.phase == InputPhase::Released {
            return "released";
        }
        match event.action {
            LogicalAction::Up => "up",
            LogicalAction::Down => "down",
            LogicalAction::Activate => "a",
            LogicalAction::Back => "b",
            LogicalAction::Home => "home",
            _ => "other",
        }
    }

    #[test]
    fn landscape_enters_settings_through_focus_and_activate() {
        let mut benchmark = SettingsNavigationBenchmark::new(true);
        assert_eq!(
            pressed(
                &benchmark
                    .event_for(Screen::Home, false, 0, true, 1)
                    .unwrap()
            ),
            "up"
        );
        assert_eq!(
            pressed(&benchmark.event_for(Screen::Home, true, 0, true, 2).unwrap()),
            "released"
        );
        assert_eq!(
            pressed(&benchmark.event_for(Screen::Home, true, 0, true, 3).unwrap()),
            "a"
        );
    }

    #[test]
    fn portrait_enters_settings_with_home() {
        let mut benchmark = SettingsNavigationBenchmark::new(true);
        benchmark.orientation_index = 1;
        assert_eq!(
            pressed(
                &benchmark
                    .event_for(Screen::Home, false, 0, true, 1)
                    .unwrap()
            ),
            "home"
        );
    }

    #[test]
    fn route_requires_ordered_rendered_and_presented_endpoints() {
        let mut benchmark = SettingsNavigationBenchmark::new(true);
        let leg = SETTINGS_NAVIGATION_ROUTE[0];
        benchmark.note_started(leg.route, leg.direction, leg.source, leg.destination, 10);
        let started_at = Instant::now();
        benchmark.capture_presentation_start(started_at, Ok(presentation_telemetry(10)));
        assert_eq!(benchmark.active_leg(), 1);
        benchmark.note_rendered_endpoint(20);
        let record = benchmark
            .note_confirmed_presentation(
                Screen::Settings,
                21,
                7,
                started_at + Duration::from_millis(300),
                Ok(presentation_telemetry(28)),
            )
            .unwrap();
        assert_eq!(record.presented_sequence, 7);
        assert_eq!(record.presentation_elapsed_us, Some(300_000));
        assert!(record.presentation_error.is_none());
        assert_eq!(benchmark.records().len(), 1);
        assert!(!benchmark.failed());
    }

    #[test]
    fn wrong_route_fails_without_recording() {
        let mut benchmark = SettingsNavigationBenchmark::new(true);
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

    #[test]
    fn runs_landscape_then_waits_for_presented_portrait_before_second_route() {
        let mut benchmark = SettingsNavigationBenchmark::new(true);
        for (index, leg) in SETTINGS_NAVIGATION_ROUTE.into_iter().enumerate() {
            benchmark.note_started(
                leg.route,
                leg.direction,
                leg.source,
                leg.destination,
                index as u64 * 10,
            );
            let started_at = Instant::now();
            benchmark.capture_presentation_start(
                started_at,
                Ok(presentation_telemetry(index as u32 * 20)),
            );
            benchmark.note_rendered_endpoint(index as u64 * 10 + 1);
            benchmark.note_confirmed_presentation(
                leg.destination,
                index as u64 * 10 + 2,
                index as u16 + 1,
                started_at + Duration::from_millis(300),
                Ok(presentation_telemetry(index as u32 * 20 + 18)),
            );
        }
        assert!(!benchmark.complete());
        assert_eq!(
            benchmark.take_orientation_change(Screen::Home, true),
            Some(ScreenOrientation::MonitorCounterclockwise)
        );
        assert!(
            benchmark
                .event_for(Screen::Home, false, 60, true, 1)
                .is_none()
        );
        benchmark.note_orientation_presented(ScreenOrientation::MonitorCounterclockwise);
        assert_eq!(
            pressed(
                &benchmark
                    .event_for(Screen::Home, false, 61, true, 2)
                    .unwrap()
            ),
            "home"
        );
        for (offset, leg) in SETTINGS_NAVIGATION_ROUTE.into_iter().enumerate() {
            let index = offset + SETTINGS_NAVIGATION_ROUTE.len();
            benchmark.note_started(
                leg.route,
                leg.direction,
                leg.source,
                leg.destination,
                index as u64 * 10,
            );
            let started_at = Instant::now();
            benchmark.capture_presentation_start(
                started_at,
                Ok(presentation_telemetry(index as u32 * 20)),
            );
            benchmark.note_rendered_endpoint(index as u64 * 10 + 1);
            benchmark.note_confirmed_presentation(
                leg.destination,
                index as u64 * 10 + 2,
                index as u16 + 1,
                started_at + Duration::from_millis(300),
                Ok(presentation_telemetry(index as u32 * 20 + 18)),
            );
        }
        assert!(benchmark.complete());
        assert_eq!(benchmark.records().len(), 12);
    }
}
