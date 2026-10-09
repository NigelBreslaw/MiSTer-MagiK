// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Always-on per-phase timing for the launcher frame loop.
//!
//! The loop marks the end of each phase with `record_launcher_frame_phase!`. The time
//! since the previous mark in the same frame belongs to the phase named by the mark, so
//! a phase list that follows execution order attributes every microsecond of a frame.
//! A frame ends at `FrameFinished` (produced), `IdleWait` (nothing to draw) or `Yielded`
//! (input or a superseded post ended it early). Only those three marks commit a frame;
//! a frame that is abandoned by an early return is counted as such.
//!
//! The cost is one clock read per mark, an array add, and no allocation. Samples go into
//! log-linear histograms (four buckets per octave, under 19 % error) and a short list of
//! the slowest frames. Once a window has run for `WINDOW` it is summarised, kept as the
//! latest window for the runtime status (`phase_profile` in `status.json`), and cleared.
//! A measurement (`begin_measurement` .. `end_measurement`) records the same data over an
//! explicit span instead, so a `check` window carries its own phase distribution.

use mister_magik_fb::runtime_status::{
    PhaseProfileStatus, PhaseStatStatus, PhaseTimeStatus, WorstFrameStatus,
};
use std::cell::RefCell;
use std::time::{Duration, Instant};

/// Length of one reporting window.
const WINDOW: Duration = Duration::from_secs(5);
/// Octaves of microseconds the histograms cover (1 µs to about 17 minutes).
const OCTAVES: usize = 30;
const SUB_BUCKETS: usize = 4;
const BUCKETS: usize = OCTAVES * SUB_BUCKETS;
/// Slowest frames kept per window for the breakdown lines.
const WORST_FRAMES: usize = 3;

macro_rules! phases {
    ($($name:ident => $label:literal),+ $(,)?) => {
        /// The marks the launcher loop records, in execution order.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(super) enum LauncherFramePhase {
            $($name),+
        }

        const PHASE_COUNT: usize = [$(stringify!($name)),+].len();
        const PHASES: [LauncherFramePhase; PHASE_COUNT] = [$(LauncherFramePhase::$name),+];

        impl LauncherFramePhase {
            fn index(self) -> usize {
                self as usize
            }

            fn label(self) -> &'static str {
                match self {
                    $(Self::$name => $label),+
                }
            }
        }
    };
}

phases! {
    Begin => "begin",
    StartupCatalogReplay => "startup_catalog_replay",
    LaunchRecoveryApplied => "launch_recovery_applied",
    PreInputMaintenance => "pre_input",
    InputCaptured => "input_captured",
    InputConsumed => "input_consumed",
    InputRouted => "input_routed",
    Yielded => "yielded",
    IdleWait => "idle_wait",
    FullScreenTransition => "full_screen_transition",
    FramePlanned => "frame_planned",
    FrameSubmitted => "frame_submitted",
    CompatibilityResolved => "compatibility_resolved",
    PostSubmitAccounted => "post_submit_accounted",
    ConfirmationInterrupted => "confirmation_interrupted",
    ActiveConfirmed => "active_confirmed",
    ReadinessSourceAcknowledged => "readiness_acknowledged",
    FrameAccounted => "frame_accounted",
    PresentationAcknowledged => "presentation_acknowledged",
    FrameFinished => "frame_finished",
}

impl LauncherFramePhase {
    /// How a frame ends at this mark, if it does.
    fn ending(self) -> Option<FrameEnd> {
        match self {
            Self::FrameFinished => Some(FrameEnd::Produced),
            Self::IdleWait => Some(FrameEnd::Idle),
            Self::Yielded => Some(FrameEnd::Yielded),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameEnd {
    Produced,
    Idle,
    Yielded,
}

/// Log-linear histogram of microsecond durations.
#[derive(Clone)]
struct Histogram {
    counts: [u32; BUCKETS],
    total: u64,
    sum_us: u64,
    max_us: u32,
}

impl Histogram {
    const fn new() -> Self {
        Self {
            counts: [0; BUCKETS],
            total: 0,
            sum_us: 0,
            max_us: 0,
        }
    }

    fn bucket(us: u32) -> usize {
        let value = us.max(1);
        let octave = (31 - value.leading_zeros()) as usize;
        let sub = if octave >= 2 {
            ((value >> (octave - 2)) & 0b11) as usize
        } else {
            ((value << (2 - octave)) & 0b11) as usize
        };
        (octave * SUB_BUCKETS + sub).min(BUCKETS - 1)
    }

    /// Lower bound in microseconds of a bucket.
    fn lower_bound(bucket: usize) -> u64 {
        let octave = bucket / SUB_BUCKETS;
        let sub = (bucket % SUB_BUCKETS) as u64;
        if octave >= 2 {
            (1u64 << octave) + (sub << (octave - 2))
        } else {
            (1u64 << octave) + (sub >> (2 - octave))
        }
    }

    fn record(&mut self, us: u32) {
        self.counts[Self::bucket(us)] = self.counts[Self::bucket(us)].saturating_add(1);
        self.total += 1;
        self.sum_us += u64::from(us);
        self.max_us = self.max_us.max(us);
    }

    /// The value at or below which `fraction` of the samples fall.
    fn percentile_us(&self, fraction: f64) -> u64 {
        if self.total == 0 {
            return 0;
        }
        let target = ((self.total as f64) * fraction).ceil().max(1.0) as u64;
        let mut seen = 0u64;
        for (bucket, count) in self.counts.iter().enumerate() {
            seen += u64::from(*count);
            if seen >= target {
                return Self::lower_bound(bucket).min(u64::from(self.max_us));
            }
        }
        u64::from(self.max_us)
    }
}

/// One finished frame's time per phase.
#[derive(Clone, Copy)]
struct FrameSample {
    total_us: u32,
    phase_us: [u32; PHASE_COUNT],
}

struct FrameProfile {
    histograms: Vec<Histogram>,
    total: Histogram,
    window_start: Instant,
    last_mark: Instant,
    open: bool,
    current: [u32; PHASE_COUNT],
    produced: u64,
    idle: u64,
    yielded: u64,
    abandoned: u64,
    budget_us: u32,
    over_budget: u64,
    worst: Vec<FrameSample>,
    latest: PhaseProfileStatus,
    window_len: Duration,
}

impl FrameProfile {
    fn new(now: Instant) -> Self {
        Self {
            histograms: vec![Histogram::new(); PHASE_COUNT],
            total: Histogram::new(),
            window_start: now,
            last_mark: now,
            open: false,
            current: [0; PHASE_COUNT],
            produced: 0,
            idle: 0,
            yielded: 0,
            abandoned: 0,
            budget_us: 0,
            over_budget: 0,
            worst: Vec::with_capacity(WORST_FRAMES + 1),
            latest: PhaseProfileStatus::default(),
            window_len: WINDOW,
        }
    }

    fn mark(&mut self, phase: LauncherFramePhase, now: Instant) -> Option<Window> {
        if phase == LauncherFramePhase::Begin {
            if self.open {
                self.abandoned += 1;
            }
            self.open = true;
            self.current = [0; PHASE_COUNT];
            self.last_mark = now;
            return None;
        }
        if !self.open {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.last_mark).as_micros();
        let elapsed = u32::try_from(elapsed).unwrap_or(u32::MAX);
        self.last_mark = now;
        let slot = &mut self.current[phase.index()];
        *slot = slot.saturating_add(elapsed);
        let end = phase.ending()?;
        self.open = false;
        let total_us = self
            .current
            .iter()
            .fold(0u32, |sum, us| sum.saturating_add(*us));
        match end {
            FrameEnd::Produced => {
                self.produced += 1;
                for (histogram, us) in self.histograms.iter_mut().zip(self.current) {
                    if us > 0 {
                        histogram.record(us);
                    }
                }
                self.total.record(total_us);
                if self.budget_us > 0 && total_us > self.budget_us + self.budget_us / 2 {
                    self.over_budget += 1;
                }
                self.keep_if_slow(FrameSample {
                    total_us,
                    phase_us: self.current,
                });
            }
            FrameEnd::Idle => self.idle += 1,
            FrameEnd::Yielded => self.yielded += 1,
        }
        if now.saturating_duration_since(self.window_start) >= self.window_len {
            let window = self.summarise(now);
            self.reset(now);
            return Some(window);
        }
        None
    }

    fn keep_if_slow(&mut self, sample: FrameSample) {
        if self.worst.len() < WORST_FRAMES {
            self.worst.push(sample);
        } else if let Some(fastest) = self.worst.iter_mut().min_by_key(|kept| kept.total_us)
            && fastest.total_us < sample.total_us
        {
            *fastest = sample;
        }
    }

    fn summarise(&self, now: Instant) -> Window {
        let phases = PHASES
            .iter()
            .zip(&self.histograms)
            .filter(|(_, histogram)| histogram.total > 0)
            .map(|(phase, histogram)| PhaseSummary::of(phase.label(), histogram))
            .collect();
        let mut worst = self.worst.clone();
        worst.sort_by_key(|sample| std::cmp::Reverse(sample.total_us));
        Window {
            elapsed_ms: now.saturating_duration_since(self.window_start).as_millis() as u64,
            produced: self.produced,
            idle: self.idle,
            yielded: self.yielded,
            abandoned: self.abandoned,
            over_budget: self.over_budget,
            budget_us: self.budget_us,
            total: PhaseSummary::of("frame", &self.total),
            phases,
            worst,
        }
    }

    fn reset(&mut self, now: Instant) {
        for histogram in &mut self.histograms {
            *histogram = Histogram::new();
        }
        self.total = Histogram::new();
        self.window_start = now;
        self.produced = 0;
        self.idle = 0;
        self.yielded = 0;
        self.abandoned = 0;
        self.over_budget = 0;
        self.worst.clear();
    }
}

struct PhaseSummary {
    label: &'static str,
    count: u64,
    mean_us: u64,
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    max_us: u64,
}

impl PhaseSummary {
    fn of(label: &'static str, histogram: &Histogram) -> Self {
        Self {
            label,
            count: histogram.total,
            mean_us: histogram.sum_us.checked_div(histogram.total).unwrap_or(0),
            p50_us: histogram.percentile_us(0.50),
            p95_us: histogram.percentile_us(0.95),
            p99_us: histogram.percentile_us(0.99),
            max_us: u64::from(histogram.max_us),
        }
    }
}

/// One closed reporting window.
struct Window {
    elapsed_ms: u64,
    produced: u64,
    idle: u64,
    yielded: u64,
    abandoned: u64,
    over_budget: u64,
    budget_us: u32,
    total: PhaseSummary,
    phases: Vec<PhaseSummary>,
    worst: Vec<FrameSample>,
}

impl Window {
    fn status(&self) -> PhaseProfileStatus {
        let stat = |summary: &PhaseSummary| PhaseStatStatus {
            phase: summary.label,
            count: summary.count,
            mean_us: summary.mean_us,
            p50_us: summary.p50_us,
            p95_us: summary.p95_us,
            p99_us: summary.p99_us,
            max_us: summary.max_us,
        };
        PhaseProfileStatus {
            window_ms: self.elapsed_ms,
            produced: self.produced,
            idle: self.idle,
            yielded: self.yielded,
            abandoned: self.abandoned,
            budget_us: self.budget_us,
            over_budget: self.over_budget,
            frame: stat(&self.total),
            phases: self.phases.iter().map(stat).collect(),
            worst: self
                .worst
                .iter()
                .map(|sample| WorstFrameStatus {
                    total_us: sample.total_us,
                    phases: PHASES
                        .iter()
                        .zip(sample.phase_us)
                        .filter(|(_, us)| *us > 0)
                        .map(|(phase, us)| PhaseTimeStatus {
                            phase: phase.label(),
                            us,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

struct Profiles {
    rolling: FrameProfile,
    measurement: Option<FrameProfile>,
}

thread_local! {
    static PROFILE: RefCell<Option<Profiles>> = const { RefCell::new(None) };
}

fn with_profiles<T>(now: Instant, body: impl FnOnce(&mut Profiles) -> T) -> T {
    PROFILE.with(|profiles| {
        let mut profiles = profiles.borrow_mut();
        let profiles = profiles.get_or_insert_with(|| Profiles {
            rolling: FrameProfile::new(now),
            measurement: None,
        });
        body(profiles)
    })
}

/// Sets the per-frame time budget (one display period) the windows count overruns against.
pub(super) fn set_budget_us(budget_us: u32) {
    with_profiles(Instant::now(), |profiles| {
        profiles.rolling.budget_us = budget_us
    });
}

/// Records the end of `phase` on the calling thread's frame.
pub(super) fn mark(phase: LauncherFramePhase) {
    let now = Instant::now();
    with_profiles(now, |profiles| {
        if let Some(window) = profiles.rolling.mark(phase, now) {
            profiles.rolling.latest = window.status();
        }
        if let Some(measurement) = profiles.measurement.as_mut() {
            measurement.mark(phase, now);
        }
    });
}

/// The latest closed rolling window, for the runtime status. Empty until one closes.
pub(super) fn latest() -> PhaseProfileStatus {
    with_profiles(Instant::now(), |profiles| profiles.rolling.latest.clone())
}

/// Starts recording a measurement span that ends only at `end_measurement`.
#[cfg(feature = "tooling")]
pub(super) fn begin_measurement() {
    let now = Instant::now();
    with_profiles(now, |profiles| {
        let mut measurement = FrameProfile::new(now);
        measurement.budget_us = profiles.rolling.budget_us;
        measurement.window_len = Duration::MAX;
        profiles.measurement = Some(measurement);
    });
}

/// Ends the measurement span and returns its phase distribution (empty when none began).
#[cfg(feature = "tooling")]
pub(super) fn end_measurement() -> PhaseProfileStatus {
    let now = Instant::now();
    with_profiles(now, |profiles| {
        profiles
            .measurement
            .take()
            .map(|measurement| measurement.summarise(now).status())
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, us: u64) -> Instant {
        start + Duration::from_micros(us)
    }

    #[test]
    fn buckets_are_monotonic_and_bracket_their_values() {
        let mut previous = 0;
        for value in [1u32, 2, 3, 4, 5, 7, 8, 100, 999, 1_000, 16_666, 1_000_000] {
            let bucket = Histogram::bucket(value);
            assert!(bucket >= previous, "{value}");
            assert!(
                Histogram::lower_bound(bucket) <= u64::from(value),
                "{value}"
            );
            previous = bucket;
        }
        // Four buckets per octave keep the lower bound within a quarter of the value.
        let bucket = Histogram::bucket(16_666);
        assert!(Histogram::lower_bound(bucket) * 100 >= 16_666 * 80);
    }

    #[test]
    fn a_produced_frame_attributes_each_phase_and_its_total() {
        let start = Instant::now();
        let mut profile = FrameProfile::new(start);
        profile.mark(LauncherFramePhase::Begin, at(start, 0));
        profile.mark(LauncherFramePhase::PreInputMaintenance, at(start, 400));
        profile.mark(LauncherFramePhase::InputRouted, at(start, 900));
        profile.mark(LauncherFramePhase::FramePlanned, at(start, 5_900));
        profile.mark(LauncherFramePhase::FrameFinished, at(start, 6_900));
        let window = profile.summarise(at(start, 6_900));
        assert_eq!(window.produced, 1);
        assert_eq!(window.total.max_us, 6_900);
        let render = window
            .phases
            .iter()
            .find(|phase| phase.label == "frame_planned")
            .expect("planned phase");
        assert_eq!(render.max_us, 5_000);
        assert_eq!(window.worst.len(), 1);
        assert_eq!(window.worst[0].total_us, 6_900);
    }

    #[test]
    fn idle_and_yielded_frames_are_counted_but_not_timed() {
        let start = Instant::now();
        let mut profile = FrameProfile::new(start);
        profile.mark(LauncherFramePhase::Begin, at(start, 0));
        profile.mark(LauncherFramePhase::IdleWait, at(start, 300));
        profile.mark(LauncherFramePhase::Begin, at(start, 400));
        profile.mark(LauncherFramePhase::Yielded, at(start, 500));
        let window = profile.summarise(at(start, 500));
        assert_eq!((window.idle, window.yielded, window.produced), (1, 1, 0));
        assert_eq!(window.total.count, 0);
    }

    #[test]
    fn a_frame_abandoned_by_an_early_return_is_counted() {
        let start = Instant::now();
        let mut profile = FrameProfile::new(start);
        profile.mark(LauncherFramePhase::Begin, at(start, 0));
        profile.mark(LauncherFramePhase::PreInputMaintenance, at(start, 100));
        profile.mark(LauncherFramePhase::Begin, at(start, 200));
        profile.mark(LauncherFramePhase::FrameFinished, at(start, 300));
        let window = profile.summarise(at(start, 300));
        assert_eq!((window.abandoned, window.produced), (1, 1));
    }

    #[test]
    fn marks_outside_a_frame_are_ignored() {
        let start = Instant::now();
        let mut profile = FrameProfile::new(start);
        assert!(
            profile
                .mark(LauncherFramePhase::FrameFinished, at(start, 50))
                .is_none()
        );
        assert_eq!(profile.produced, 0);
    }

    #[test]
    fn percentiles_follow_the_distribution() {
        let mut histogram = Histogram::new();
        for _ in 0..98 {
            histogram.record(1_000);
        }
        histogram.record(9_000);
        histogram.record(40_000);
        assert!(histogram.percentile_us(0.50) <= 1_000);
        assert!(histogram.percentile_us(0.50) >= 800);
        assert!(histogram.percentile_us(0.99) >= 7_000);
        assert_eq!(histogram.percentile_us(1.0), 32_768);
        assert_eq!(histogram.max_us, 40_000);
    }

    #[test]
    fn the_slowest_frames_are_kept_and_the_window_rolls_over() {
        let start = Instant::now();
        let mut profile = FrameProfile::new(start);
        profile.budget_us = 8_000;
        let mut clock = 0u64;
        let mut closed = None;
        for total in [5_000u64, 12_000, 6_000, 20_000, 9_000] {
            profile.mark(LauncherFramePhase::Begin, at(start, clock));
            clock += total;
            profile.mark(LauncherFramePhase::FrameFinished, at(start, clock));
        }
        let window = profile.summarise(at(start, clock));
        assert_eq!(window.produced, 5);
        assert_eq!(window.over_budget, 1);
        let kept: Vec<u32> = window.worst.iter().map(|sample| sample.total_us).collect();
        assert_eq!(kept, vec![20_000, 12_000, 9_000]);
        // A mark past the window length closes it and starts a fresh one.
        profile.mark(LauncherFramePhase::Begin, at(start, clock));
        if let Some(rolled) = profile.mark(
            LauncherFramePhase::FrameFinished,
            at(start, clock + WINDOW.as_micros() as u64),
        ) {
            closed = Some(rolled);
        }
        let rolled = closed.expect("window closes after its length");
        assert_eq!(rolled.produced, 6);
        assert_eq!(profile.produced, 0);
        assert!(profile.worst.is_empty());
    }

    #[test]
    fn the_status_names_every_timed_phase_and_the_worst_frames() {
        let start = Instant::now();
        let mut profile = FrameProfile::new(start);
        profile.mark(LauncherFramePhase::Begin, at(start, 0));
        profile.mark(LauncherFramePhase::InputRouted, at(start, 200));
        profile.mark(LauncherFramePhase::FrameFinished, at(start, 1_200));
        let status = profile.summarise(at(start, 1_200)).status();
        assert_eq!(status.produced, 1);
        assert_eq!(status.frame.phase, "frame");
        assert_eq!(status.frame.max_us, 1_200);
        assert!(
            status
                .phases
                .iter()
                .any(|phase| phase.phase == "input_routed")
        );
        let worst = &status.worst[0];
        assert_eq!(worst.total_us, 1_200);
        assert!(
            worst
                .phases
                .iter()
                .any(|phase| phase.phase == "frame_finished" && phase.us == 1_000)
        );
        let json = serde_json::to_value(&status).expect("status serialises");
        assert_eq!(json["worst"][0]["total_us"], 1_200);
    }

    #[cfg(feature = "tooling")]
    #[test]
    fn a_measurement_covers_only_its_own_span_and_never_rolls_over() {
        // The thread-local is per test thread, so this exercises the public functions.
        mark(LauncherFramePhase::Begin);
        mark(LauncherFramePhase::FrameFinished);
        begin_measurement();
        for _ in 0..3 {
            mark(LauncherFramePhase::Begin);
            mark(LauncherFramePhase::FrameFinished);
        }
        let report = end_measurement();
        assert_eq!(report.produced, 3);
        assert_eq!(end_measurement(), PhaseProfileStatus::default());
        mark(LauncherFramePhase::Begin);
        mark(LauncherFramePhase::FrameFinished);
        assert_eq!(end_measurement(), PhaseProfileStatus::default());
    }
}
