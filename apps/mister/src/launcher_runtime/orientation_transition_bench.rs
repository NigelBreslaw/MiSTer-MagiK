// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fixed production-view orientation-transition benchmark sequencing for both effects.

use super::orientation_transition::OrientationTransitionEffect;
use crate::settings::ScreenOrientation;

pub const ORIENTATION_TRANSITION_BENCHMARK_ROUTE: [ScreenOrientation; 7] = [
    ScreenOrientation::Normal,
    ScreenOrientation::MonitorClockwise,
    ScreenOrientation::MonitorCounterclockwise,
    ScreenOrientation::Normal,
    ScreenOrientation::MonitorCounterclockwise,
    ScreenOrientation::MonitorClockwise,
    ScreenOrientation::Normal,
];
pub const ORIENTATION_TRANSITION_BENCHMARK_EFFECTS: [OrientationTransitionEffect; 2] = [
    OrientationTransitionEffect::BrightnessFade,
    OrientationTransitionEffect::CenterPixelZoom,
];
pub const ORIENTATION_TRANSITION_BENCHMARK_LEGS: usize =
    (ORIENTATION_TRANSITION_BENCHMARK_ROUTE.len() - 1)
        * ORIENTATION_TRANSITION_BENCHMARK_EFFECTS.len();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrientationTransitionBenchmarkLeg {
    pub index: usize,
    pub effect: OrientationTransitionEffect,
    pub from: ScreenOrientation,
    pub to: ScreenOrientation,
}

impl OrientationTransitionBenchmarkLeg {
    pub const fn label(self) -> &'static str {
        match (self.from, self.to) {
            (ScreenOrientation::Normal, ScreenOrientation::MonitorClockwise) => {
                "normal-to-clockwise"
            }
            (ScreenOrientation::MonitorClockwise, ScreenOrientation::MonitorCounterclockwise) => {
                "clockwise-to-counterclockwise"
            }
            (ScreenOrientation::MonitorCounterclockwise, ScreenOrientation::Normal) => {
                "counterclockwise-to-normal"
            }
            (ScreenOrientation::Normal, ScreenOrientation::MonitorCounterclockwise) => {
                "normal-to-counterclockwise"
            }
            (ScreenOrientation::MonitorCounterclockwise, ScreenOrientation::MonitorClockwise) => {
                "counterclockwise-to-clockwise"
            }
            (ScreenOrientation::MonitorClockwise, ScreenOrientation::Normal) => {
                "clockwise-to-normal"
            }
            _ => "invalid",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrientationTransitionBenchmarkRecord {
    pub leg: OrientationTransitionBenchmarkLeg,
    pub start_frame: u64,
    pub rendered_endpoint_frame: u64,
    pub presented_endpoint_frame: u64,
    pub presented_sequence: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BenchmarkPhase {
    Disabled,
    WaitingForInitialPresentation,
    Ready,
    Transitioning,
    WaitingForEndpointPresentation,
    Complete,
    Failed,
}

pub struct OrientationTransitionBenchmark {
    phase: BenchmarkPhase,
    next_leg: usize,
    start_frame: u64,
    rendered_endpoint_frame: u64,
    records: Vec<OrientationTransitionBenchmarkRecord>,
    failure: Option<&'static str>,
}

impl OrientationTransitionBenchmark {
    pub fn new(enabled: bool) -> Self {
        Self {
            phase: if enabled {
                BenchmarkPhase::WaitingForInitialPresentation
            } else {
                BenchmarkPhase::Disabled
            },
            next_leg: 0,
            start_frame: 0,
            rendered_endpoint_frame: 0,
            records: Vec::with_capacity(ORIENTATION_TRANSITION_BENCHMARK_LEGS),
            failure: None,
        }
    }

    pub const fn enabled(&self) -> bool {
        !matches!(self.phase, BenchmarkPhase::Disabled)
    }

    pub const fn complete(&self) -> bool {
        matches!(self.phase, BenchmarkPhase::Complete)
    }

    pub const fn failed(&self) -> bool {
        matches!(self.phase, BenchmarkPhase::Failed)
    }

    pub const fn failure(&self) -> Option<&'static str> {
        self.failure
    }

    pub fn records(&self) -> &[OrientationTransitionBenchmarkRecord] {
        &self.records
    }

    pub fn active_leg(&self) -> Option<OrientationTransitionBenchmarkLeg> {
        matches!(
            self.phase,
            BenchmarkPhase::Transitioning | BenchmarkPhase::WaitingForEndpointPresentation
        )
        .then(|| self.leg(self.next_leg))
        .flatten()
    }

    pub fn note_confirmed_presentation(
        &mut self,
        orientation: ScreenOrientation,
        frame: u64,
        sequence: u16,
    ) -> Option<OrientationTransitionBenchmarkRecord> {
        match self.phase {
            BenchmarkPhase::WaitingForInitialPresentation => {
                if orientation != ScreenOrientation::Normal {
                    self.fail("initial-orientation-is-not-normal");
                } else if sequence != 0 {
                    self.phase = BenchmarkPhase::Ready;
                }
                None
            }
            BenchmarkPhase::WaitingForEndpointPresentation => {
                let Some(leg) = self.leg(self.next_leg) else {
                    self.fail("endpoint-without-leg");
                    return None;
                };
                if orientation != leg.to {
                    self.fail("presented-orientation-does-not-match-leg");
                    return None;
                }
                let record = OrientationTransitionBenchmarkRecord {
                    leg,
                    start_frame: self.start_frame,
                    rendered_endpoint_frame: self.rendered_endpoint_frame,
                    presented_endpoint_frame: frame,
                    presented_sequence: sequence,
                };
                self.records.push(record);
                self.next_leg += 1;
                self.phase = if self.next_leg == ORIENTATION_TRANSITION_BENCHMARK_LEGS {
                    BenchmarkPhase::Complete
                } else {
                    BenchmarkPhase::Ready
                };
                Some(record)
            }
            _ => None,
        }
    }

    pub fn take_next_leg(
        &mut self,
        current: ScreenOrientation,
        frame: u64,
    ) -> Option<OrientationTransitionBenchmarkLeg> {
        if self.phase != BenchmarkPhase::Ready {
            return None;
        }
        let Some(leg) = self.leg(self.next_leg) else {
            self.fail("ready-without-leg");
            return None;
        };
        if current != leg.from {
            self.fail("transition-source-does-not-match-leg");
            return None;
        }
        self.start_frame = frame;
        self.rendered_endpoint_frame = 0;
        self.phase = BenchmarkPhase::Transitioning;
        Some(leg)
    }

    pub fn note_rendered_endpoint(&mut self, frame: u64) {
        if self.phase == BenchmarkPhase::Transitioning {
            self.rendered_endpoint_frame = frame;
            self.phase = BenchmarkPhase::WaitingForEndpointPresentation;
        } else {
            self.fail("rendered-endpoint-outside-transition");
        }
    }

    pub fn fail(&mut self, failure: &'static str) {
        if !matches!(
            self.phase,
            BenchmarkPhase::Complete | BenchmarkPhase::Disabled
        ) {
            self.phase = BenchmarkPhase::Failed;
            self.failure.get_or_insert(failure);
        }
    }

    fn leg(&self, index: usize) -> Option<OrientationTransitionBenchmarkLeg> {
        let route_legs = ORIENTATION_TRANSITION_BENCHMARK_ROUTE.len() - 1;
        let effect = *ORIENTATION_TRANSITION_BENCHMARK_EFFECTS.get(index / route_legs)?;
        let route_index = index % route_legs;
        let from = *ORIENTATION_TRANSITION_BENCHMARK_ROUTE.get(route_index)?;
        let to = *ORIENTATION_TRANSITION_BENCHMARK_ROUTE.get(route_index + 1)?;
        Some(OrientationTransitionBenchmarkLeg {
            index,
            effect,
            from,
            to,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_covers_every_directed_orientation_pair_once() {
        let pairs = ORIENTATION_TRANSITION_BENCHMARK_ROUTE
            .windows(2)
            .map(|pair| (pair[0], pair[1]))
            .collect::<Vec<_>>();

        assert_eq!(pairs.len(), 6);
        for (index, pair) in pairs.iter().enumerate() {
            assert!(!pairs[..index].contains(pair));
        }
        for from in ScreenOrientation::ALL {
            for to in ScreenOrientation::ALL {
                if from != to {
                    assert!(pairs.contains(&(from, to)));
                }
            }
        }
    }

    #[test]
    fn every_leg_waits_for_a_confirmed_endpoint() {
        let mut benchmark = OrientationTransitionBenchmark::new(true);
        assert!(
            benchmark
                .note_confirmed_presentation(ScreenOrientation::Normal, 10, 1)
                .is_none()
        );

        for index in 0..ORIENTATION_TRANSITION_BENCHMARK_LEGS {
            let route_index = index % (ORIENTATION_TRANSITION_BENCHMARK_ROUTE.len() - 1);
            let pair = &ORIENTATION_TRANSITION_BENCHMARK_ROUTE[route_index..=route_index + 1];
            let leg = benchmark
                .take_next_leg(pair[0], 11 + index as u64 * 3)
                .unwrap();
            assert_eq!(leg.index, index);
            assert_eq!(
                leg.effect,
                ORIENTATION_TRANSITION_BENCHMARK_EFFECTS
                    [index / (ORIENTATION_TRANSITION_BENCHMARK_ROUTE.len() - 1)]
            );
            assert!(benchmark.take_next_leg(pair[0], 12).is_none());
            benchmark.note_rendered_endpoint(12 + index as u64 * 3);
            let record = benchmark
                .note_confirmed_presentation(pair[1], 13 + index as u64 * 3, index as u16 + 2)
                .unwrap();
            assert_eq!(record.leg, leg);
        }

        assert!(benchmark.complete());
        assert_eq!(
            benchmark.records().len(),
            ORIENTATION_TRANSITION_BENCHMARK_LEGS
        );
        assert!(
            benchmark
                .take_next_leg(ScreenOrientation::Normal, 99)
                .is_none()
        );
    }

    #[test]
    fn mismatched_source_fails_without_starting_an_extra_leg() {
        let mut benchmark = OrientationTransitionBenchmark::new(true);
        benchmark.note_confirmed_presentation(ScreenOrientation::Normal, 1, 1);

        assert!(
            benchmark
                .take_next_leg(ScreenOrientation::MonitorClockwise, 2)
                .is_none()
        );
        assert!(benchmark.failed());
        assert_eq!(
            benchmark.failure(),
            Some("transition-source-does-not-match-leg")
        );
        assert!(benchmark.records().is_empty());
    }

    #[test]
    fn disabled_benchmark_is_inert() {
        let mut benchmark = OrientationTransitionBenchmark::new(false);
        benchmark.note_confirmed_presentation(ScreenOrientation::Normal, 1, 1);
        assert!(!benchmark.enabled());
        assert!(!benchmark.complete());
        assert!(
            benchmark
                .take_next_leg(ScreenOrientation::Normal, 2)
                .is_none()
        );
    }
}
