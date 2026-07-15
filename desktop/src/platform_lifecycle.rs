// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DisplayClockTick {
    pub(crate) timestamp_us: u64,
    pub(crate) target_timestamp_us: u64,
    pub(crate) duration_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplayClockSource {
    Native,
    SlintTimer,
}

pub(crate) trait DisplayClockAdapter {
    type Clock;

    fn start_native(&mut self) -> Option<Self::Clock>;
    fn start_slint_timer(&mut self) -> Self::Clock;
}

pub(crate) struct DisplayClockController<C> {
    active: Option<C>,
    source: DisplayClockSource,
}

impl<C> DisplayClockController<C> {
    pub(crate) fn start(adapter: &mut impl DisplayClockAdapter<Clock = C>) -> Self {
        let (active, source) = match adapter.start_native() {
            Some(clock) => (clock, DisplayClockSource::Native),
            None => (adapter.start_slint_timer(), DisplayClockSource::SlintTimer),
        };
        Self {
            active: Some(active),
            source,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn replace(&mut self, adapter: &mut impl DisplayClockAdapter<Clock = C>) {
        let replacement = Self::start(adapter);
        self.active = replacement.active;
        self.source = replacement.source;
    }

    pub(crate) fn source(&self) -> DisplayClockSource {
        self.source
    }

    pub(crate) fn into_clock(mut self) -> C {
        self.active
            .take()
            .expect("display clock controller is active")
    }
}

pub(crate) trait TitlebarAdapter {
    fn setup(&mut self) -> bool;
    fn activate_benchmark(&mut self) -> bool;
}

#[derive(Default)]
pub(crate) struct TitlebarController {
    setup_attempted: bool,
}

impl TitlebarController {
    pub(crate) fn setup_once(&mut self, adapter: &mut impl TitlebarAdapter) -> bool {
        if self.setup_attempted {
            return false;
        }
        self.setup_attempted = true;
        adapter.setup()
    }

    pub(crate) fn activate_benchmark(&mut self, adapter: &mut impl TitlebarAdapter) -> bool {
        adapter.activate_benchmark()
    }
}

pub(crate) fn seconds_to_microseconds(seconds: f64) -> u64 {
    if seconds.is_finite() && seconds > 0.0 {
        (seconds * 1_000_000.0).round() as u64
    } else {
        0
    }
}

pub(crate) fn timer_tick(elapsed: Duration, interval: Duration) -> DisplayClockTick {
    let timestamp_us = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
    let duration_us = interval.as_micros().min(u128::from(u64::MAX)) as u64;
    DisplayClockTick {
        timestamp_us,
        target_timestamp_us: timestamp_us.saturating_add(duration_us),
        duration_us,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct FakeClock(Rc<Cell<usize>>);

    impl Drop for FakeClock {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    struct FakeDisplayAdapter {
        native_available: bool,
        native_starts: usize,
        timer_starts: usize,
        stops: Rc<Cell<usize>>,
    }

    impl DisplayClockAdapter for FakeDisplayAdapter {
        type Clock = FakeClock;

        fn start_native(&mut self) -> Option<Self::Clock> {
            self.native_starts += 1;
            self.native_available
                .then(|| FakeClock(Rc::clone(&self.stops)))
        }

        fn start_slint_timer(&mut self) -> Self::Clock {
            self.timer_starts += 1;
            FakeClock(Rc::clone(&self.stops))
        }
    }

    #[test]
    fn native_display_clock_is_preferred_when_available() {
        let stops = Rc::new(Cell::new(0));
        let mut adapter = FakeDisplayAdapter {
            native_available: true,
            native_starts: 0,
            timer_starts: 0,
            stops,
        };
        let controller = DisplayClockController::start(&mut adapter);

        assert_eq!(controller.source(), DisplayClockSource::Native);
        assert_eq!(adapter.native_starts, 1);
        assert_eq!(adapter.timer_starts, 0);
    }

    #[test]
    fn slint_timer_is_used_when_native_clock_is_unavailable() {
        let stops = Rc::new(Cell::new(0));
        let mut adapter = FakeDisplayAdapter {
            native_available: false,
            native_starts: 0,
            timer_starts: 0,
            stops,
        };
        let controller = DisplayClockController::start(&mut adapter);

        assert_eq!(controller.source(), DisplayClockSource::SlintTimer);
        assert_eq!(adapter.native_starts, 1);
        assert_eq!(adapter.timer_starts, 1);
    }

    #[test]
    fn replacing_and_dropping_controller_stop_each_clock_once() {
        let stops = Rc::new(Cell::new(0));
        let mut adapter = FakeDisplayAdapter {
            native_available: true,
            native_starts: 0,
            timer_starts: 0,
            stops: Rc::clone(&stops),
        };
        let mut controller = DisplayClockController::start(&mut adapter);
        controller.replace(&mut adapter);
        assert_eq!(stops.get(), 1);

        drop(controller);
        assert_eq!(stops.get(), 2);
    }

    #[test]
    fn display_clock_timestamp_conversions_are_safe() {
        assert_eq!(seconds_to_microseconds(1.25), 1_250_000);
        assert_eq!(seconds_to_microseconds(1.0 / 120.0), 8_333);
        assert_eq!(seconds_to_microseconds(f64::NAN), 0);
        assert_eq!(seconds_to_microseconds(-1.0), 0);

        let tick = timer_tick(Duration::from_micros(u64::MAX), Duration::from_micros(2));
        assert_eq!(tick.timestamp_us, u64::MAX);
        assert_eq!(tick.target_timestamp_us, u64::MAX);
        assert_eq!(tick.duration_us, 2);
    }

    #[derive(Default)]
    struct FakeTitlebarAdapter {
        setup_calls: usize,
        benchmark_calls: usize,
        setup_succeeds: bool,
    }

    impl TitlebarAdapter for FakeTitlebarAdapter {
        fn setup(&mut self) -> bool {
            self.setup_calls += 1;
            self.setup_succeeds
        }

        fn activate_benchmark(&mut self) -> bool {
            self.benchmark_calls += 1;
            true
        }
    }

    #[test]
    fn titlebar_setup_is_attempted_once_and_failure_is_harmless() {
        let mut controller = TitlebarController::default();
        let mut adapter = FakeTitlebarAdapter::default();

        assert!(!controller.setup_once(&mut adapter));
        assert!(!controller.setup_once(&mut adapter));
        assert_eq!(adapter.setup_calls, 1);
    }

    #[test]
    fn benchmark_activation_is_separate_from_normal_titlebar_setup() {
        let mut controller = TitlebarController::default();
        let mut adapter = FakeTitlebarAdapter {
            setup_succeeds: true,
            ..FakeTitlebarAdapter::default()
        };

        assert!(controller.setup_once(&mut adapter));
        assert!(controller.activate_benchmark(&mut adapter));
        assert_eq!(adapter.setup_calls, 1);
        assert_eq!(adapter.benchmark_calls, 1);
    }
}
