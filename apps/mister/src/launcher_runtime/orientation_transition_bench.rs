// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fixed production-view orientation-transition benchmark sequencing for one selected effect.

use super::orientation_transition::OrientationTransitionEffect;
use crate::settings::ScreenOrientation;
use mister_magik_latch_contract::PresentationTelemetry;
use std::time::Instant;

pub const ORIENTATION_TRANSITION_BENCHMARK_ROUTE: [ScreenOrientation; 7] = [
    ScreenOrientation::Normal,
    ScreenOrientation::MonitorClockwise,
    ScreenOrientation::MonitorCounterclockwise,
    ScreenOrientation::Normal,
    ScreenOrientation::MonitorCounterclockwise,
    ScreenOrientation::MonitorClockwise,
    ScreenOrientation::Normal,
];
pub const ORIENTATION_TRANSITION_BENCHMARK_LEGS: usize =
    ORIENTATION_TRANSITION_BENCHMARK_ROUTE.len() - 1;

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
pub struct OrientationTransitionPresentationCapture {
    pub telemetry: PresentationTelemetry,
    pub captured_at: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrientationTransitionBenchmarkRecord {
    pub leg: OrientationTransitionBenchmarkLeg,
    pub start_frame: u64,
    pub rendered_endpoint_frame: u64,
    pub presented_endpoint_frame: u64,
    pub presented_sequence: u16,
    pub presentation_start: Option<OrientationTransitionPresentationCapture>,
    pub presentation_end: Option<OrientationTransitionPresentationCapture>,
    pub presentation_elapsed_us: Option<u64>,
    pub presentation_error: Option<String>,
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
    effect: OrientationTransitionEffect,
    next_leg: usize,
    start_frame: u64,
    rendered_endpoint_frame: u64,
    presentation_start: Option<OrientationTransitionPresentationCapture>,
    presentation_error: Option<String>,
    records: Vec<OrientationTransitionBenchmarkRecord>,
    failure: Option<&'static str>,
}

impl OrientationTransitionBenchmark {
    pub fn new(enabled: bool, effect: OrientationTransitionEffect) -> Self {
        Self {
            phase: if enabled {
                BenchmarkPhase::WaitingForInitialPresentation
            } else {
                BenchmarkPhase::Disabled
            },
            effect,
            next_leg: 0,
            start_frame: 0,
            rendered_endpoint_frame: 0,
            presentation_start: None,
            presentation_error: None,
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

    pub const fn effect(&self) -> OrientationTransitionEffect {
        self.effect
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
        captured_at: Instant,
        telemetry: std::io::Result<PresentationTelemetry>,
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
                let (presentation_end, presentation_elapsed_us) = match telemetry {
                    Ok(telemetry) => {
                        let end = OrientationTransitionPresentationCapture {
                            telemetry,
                            captured_at,
                        };
                        let elapsed_us = self.presentation_start.map(|start| {
                            captured_at
                                .saturating_duration_since(start.captured_at)
                                .as_micros()
                                .min(u128::from(u64::MAX)) as u64
                        });
                        if elapsed_us.is_none() && self.presentation_error.is_none() {
                            self.presentation_error =
                                Some("presentation telemetry start was not captured".to_string());
                        }
                        (Some(end), elapsed_us)
                    }
                    Err(error) => {
                        self.presentation_error = Some(error.to_string());
                        (None, None)
                    }
                };
                let record = OrientationTransitionBenchmarkRecord {
                    leg,
                    start_frame: self.start_frame,
                    rendered_endpoint_frame: self.rendered_endpoint_frame,
                    presented_endpoint_frame: frame,
                    presented_sequence: sequence,
                    presentation_start: self.presentation_start.take(),
                    presentation_end,
                    presentation_elapsed_us,
                    presentation_error: self.presentation_error.take(),
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
        self.presentation_start = None;
        self.presentation_error = None;
        self.phase = BenchmarkPhase::Transitioning;
        Some(leg)
    }

    pub fn capture_presentation_start(
        &mut self,
        captured_at: Instant,
        telemetry: std::io::Result<PresentationTelemetry>,
    ) {
        if self.phase != BenchmarkPhase::Transitioning
            || self.presentation_start.is_some()
            || self.presentation_error.is_some()
        {
            return;
        }
        match telemetry {
            Ok(telemetry) => {
                self.presentation_start = Some(OrientationTransitionPresentationCapture {
                    telemetry,
                    captured_at,
                });
            }
            Err(error) => self.presentation_error = Some(error.to_string()),
        }
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
        let from = *ORIENTATION_TRANSITION_BENCHMARK_ROUTE.get(index)?;
        let to = *ORIENTATION_TRANSITION_BENCHMARK_ROUTE.get(index + 1)?;
        Some(OrientationTransitionBenchmarkLeg {
            index,
            effect: self.effect,
            from,
            to,
        })
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
        let effect = OrientationTransitionEffect::CenterPixelZoom;
        let mut benchmark = OrientationTransitionBenchmark::new(true, effect);
        assert!(
            benchmark
                .note_confirmed_presentation(
                    ScreenOrientation::Normal,
                    10,
                    1,
                    Instant::now(),
                    Ok(presentation_telemetry(1)),
                )
                .is_none()
        );

        for index in 0..ORIENTATION_TRANSITION_BENCHMARK_LEGS {
            let pair = &ORIENTATION_TRANSITION_BENCHMARK_ROUTE[index..=index + 1];
            let leg = benchmark
                .take_next_leg(pair[0], 11 + index as u64 * 3)
                .unwrap();
            assert_eq!(leg.index, index);
            assert_eq!(leg.effect, effect);
            assert!(benchmark.take_next_leg(pair[0], 12).is_none());
            let started_at = Instant::now();
            benchmark.capture_presentation_start(
                started_at,
                Ok(presentation_telemetry(index as u32 + 2)),
            );
            benchmark.note_rendered_endpoint(12 + index as u64 * 3);
            let record = benchmark
                .note_confirmed_presentation(
                    pair[1],
                    13 + index as u64 * 3,
                    index as u16 + 2,
                    started_at + std::time::Duration::from_millis(1_550),
                    Ok(presentation_telemetry(index as u32 + 95)),
                )
                .unwrap();
            assert_eq!(record.leg, leg);
            assert_eq!(record.presentation_elapsed_us, Some(1_550_000));
            assert!(record.presentation_error.is_none());
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
        let mut benchmark =
            OrientationTransitionBenchmark::new(true, OrientationTransitionEffect::BrightnessFade);
        benchmark.note_confirmed_presentation(
            ScreenOrientation::Normal,
            1,
            1,
            Instant::now(),
            Ok(presentation_telemetry(1)),
        );

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
        let mut benchmark =
            OrientationTransitionBenchmark::new(false, OrientationTransitionEffect::BrightnessFade);
        benchmark.note_confirmed_presentation(
            ScreenOrientation::Normal,
            1,
            1,
            Instant::now(),
            Ok(presentation_telemetry(1)),
        );
        assert!(!benchmark.enabled());
        assert!(!benchmark.complete());
        assert!(
            benchmark
                .take_next_leg(ScreenOrientation::Normal, 2)
                .is_none()
        );
    }
}
