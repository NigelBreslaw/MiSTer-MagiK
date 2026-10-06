// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `PresentationDirector` owns the three things that decide what the
//! launcher shows during a full-screen transition: the navigation runtime (the
//! motion), the `FullScreenTransitionStateChart` (who may render) and the
//! `UiCompositionController` (what is on screen). The launcher loop calls its
//! methods instead of pairing the three by hand, so each pairing is written once
//! and the host walks in this module can drive exactly what the loop drives.
//!
//! The relations between the three are measured, not assumed; see the walks in
//! `tests` and `presentation_scenarios` and `docs/presentation-refactor-roadmap.md`.

use super::composition::UiCompositionController;
use super::full_screen_transition::{FullScreenTransitionOwner, FullScreenTransitionStateChart};
use super::navigation_transition::{
    NavigationTransitionCompletion, NavigationTransitionEndpoint, NavigationTransitionRuntime,
};
use slint::platform::software_renderer::Rgb565Pixel;

/// Give `owner` the full-screen transition. Returns whether it now holds it;
/// the refusal is logged.
pub fn begin_full_screen_transition(
    chart: &mut FullScreenTransitionStateChart,
    owner: FullScreenTransitionOwner,
) -> bool {
    match chart.begin(owner) {
        Ok(_) => true,
        Err(error) => {
            crate::ui_errln!("{owner:?} full-screen transition begin rejected: {error:?}");
            false
        }
    }
}

/// Ask the chart to force the live frame that ends `owner`'s transition.
pub fn release_full_screen_transition(
    chart: &mut FullScreenTransitionStateChart,
    owner: FullScreenTransitionOwner,
) {
    if let Some(generation) = chart.generation_for(owner)
        && let Err(error) = chart.release(generation)
    {
        crate::ui_errln!("{owner:?} full-screen transition release rejected: {error:?}");
    }
}

#[derive(Debug)]
pub struct PresentationDirector {
    pub navigation: NavigationTransitionRuntime,
    pub chart: FullScreenTransitionStateChart,
    pub composition: UiCompositionController,
}

impl PresentationDirector {
    pub fn new(navigation: NavigationTransitionRuntime) -> Self {
        Self {
            navigation,
            chart: FullScreenTransitionStateChart::default(),
            composition: UiCompositionController::new(),
        }
    }

    /// The runtime has started a navigation transition: have the chart hold the
    /// frame for it. When the chart refuses, the caller unwinds the transition.
    pub fn hold_frame_for_navigation(&mut self) -> bool {
        begin_full_screen_transition(&mut self.chart, FullScreenTransitionOwner::Navigation)
    }

    /// A navigation transition the runtime started but the chart refused to host:
    /// settle it where it is going and drop it, as if it had never played.
    pub fn unwind_navigation(&mut self) {
        self.navigation.settle_at_destination();
        let _ = self.navigation.complete();
    }

    /// Hand the runtime the captured destination and lock the chart's snapshot.
    /// If either refuses, the transition settles at its destination. Returns
    /// whether the capture took.
    pub fn capture_navigation_destination(
        &mut self,
        destination: &[Rgb565Pixel],
        now_us: u64,
    ) -> bool {
        let captured = self
            .navigation
            .capture_destination(destination, now_us)
            .is_ok()
            && self
                .chart
                .generation_for(FullScreenTransitionOwner::Navigation)
                .is_none_or(|generation| self.chart.capture_completed(generation).is_ok());
        if !captured {
            self.navigation.settle_at_destination();
        }
        captured
    }

    /// End a settled navigation transition: take its completion and, if there was
    /// one, release the chart.
    pub fn finish_navigation(&mut self) -> Option<NavigationTransitionCompletion> {
        let completion = self.navigation.complete();
        if completion.is_some() {
            release_full_screen_transition(&mut self.chart, FullScreenTransitionOwner::Navigation);
        }
        completion
    }

    /// A screensaver or confirmation now covers a playing navigation transition.
    /// It cannot keep playing under the cover: settle it at the destination if
    /// that was already committed, otherwise cancel it back to the source, then
    /// end it and release the chart, all before composition is asked. Returns the
    /// endpoint it ended at.
    pub fn cover_navigation(
        &mut self,
        destination_committed: bool,
    ) -> Option<NavigationTransitionEndpoint> {
        let endpoint = if destination_committed {
            self.navigation.settle_at_destination();
            Some(NavigationTransitionEndpoint::Destination)
        } else {
            self.navigation.cancel_for_exclusive_view()
        };
        let _ = self.finish_navigation();
        endpoint
    }
}

#[cfg(test)]
mod presentation_scenarios;

#[cfg(test)]
mod tests {
    use super::super::full_screen_transition::FullScreenTransitionState;
    use super::super::navigation_transition::{
        NavigationTransitionDirection, NavigationTransitionEdge, NavigationTransitionPhase,
    };
    use super::super::transition_scenarios::{DIRECTIONS, EDGES, HEIGHT, WIDTH, begin, frame};
    use super::super::walk_rng::WalkRng as Rng;
    use super::*;
    use std::collections::BTreeSet;

    const WALKS: usize = 300;
    const STEPS: usize = 80;

    pub(super) fn director() -> PresentationDirector {
        PresentationDirector::new(NavigationTransitionRuntime::new(WIDTH, HEIGHT, true))
    }

    /// Start the way the loop does: the runtime first, then the chart, and a
    /// transition the chart refuses is unwound.
    pub(super) fn start(
        director: &mut PresentationDirector,
        edge: NavigationTransitionEdge,
        direction: NavigationTransitionDirection,
        source: &[Rgb565Pixel],
        now_us: u64,
    ) -> bool {
        if !begin(&mut director.navigation, edge, direction, source, now_us) {
            return false;
        }
        if director.hold_frame_for_navigation() {
            true
        } else {
            director.unwind_navigation();
            false
        }
    }

    /// The runtime and the chart agree on who is playing and which stage it is in.
    fn check(director: &PresentationDirector, seen: &mut BTreeSet<String>, at: &str) {
        use FullScreenTransitionState::*;
        let (runtime, chart) = (&director.navigation, &director.chart);
        let phase = runtime.frame().phase;
        seen.insert(format!("{:?} + {phase:?}", chart.state()));
        if runtime.is_active() {
            assert_eq!(
                chart.owner(),
                Some(FullScreenTransitionOwner::Navigation),
                "{at}: an active transition has the chart"
            );
            assert!(
                matches!(chart.state(), CapturePending | SnapshotLocked),
                "{at}: {:?} while playing",
                chart.state()
            );
        } else {
            assert!(
                matches!(chart.state(), Live | Releasing),
                "{at}: {:?} with nothing playing",
                chart.state()
            );
        }
        // The timeline may run ahead of the snapshot lock (an immediate start
        // plays while the destination is prepared), but it never reveals one.
        if chart.state() == CapturePending {
            assert_ne!(
                phase,
                NavigationTransitionPhase::Reveal,
                "{at}: revealed a destination the chart has not locked"
            );
        }
        if chart.state() == SnapshotLocked {
            assert_ne!(
                phase,
                NavigationTransitionPhase::Capture,
                "{at}: a locked snapshot is past capture"
            );
        }
    }

    #[test]
    fn random_walks_keep_the_runtime_and_the_chart_in_step() {
        let source = frame(0x1111);
        let destination = frame(0x2222);
        let mut rng = Rng(0x7E57_0DD5_1CE5_BEEF);
        let mut seen = BTreeSet::new();
        let mut finished = 0;
        let mut unwound = 0;
        for walk in 0..WALKS {
            let mut d = director();
            let mut now_us = 0u64;
            for step in 0..STEPS {
                let at = format!("walk {walk} step {step}");
                now_us += [0, 5_000, 40_000, 400_000, 10_000_000][rng.below(5)];
                match rng.below(10) {
                    9 => {
                        // The loop authorizes the controlled capture before it renders it.
                        if let Some(generation) = d
                            .chart
                            .generation_for(FullScreenTransitionOwner::Navigation)
                            && d.chart.state() == FullScreenTransitionState::CapturePending
                        {
                            d.chart.take_controlled_capture(generation).unwrap();
                        }
                    }
                    0 | 1 => {
                        let edge = EDGES[rng.below(EDGES.len())];
                        let direction = DIRECTIONS[rng.below(DIRECTIONS.len())];
                        let chart_was_free = d.chart.is_live();
                        let started = start(&mut d, edge, direction, &source, now_us);
                        if !started && !d.navigation.is_active() && !chart_was_free {
                            unwound += 1;
                        }
                        assert!(
                            !started || chart_was_free,
                            "{at}: began over a releasing chart"
                        );
                    }
                    2 | 3 => {
                        d.capture_navigation_destination(&destination, now_us);
                    }
                    4 => {
                        d.navigation.tick(now_us);
                    }
                    5 => {
                        d.navigation.request_reverse(now_us);
                    }
                    6 => {
                        if rng.chance(50) {
                            d.navigation.settle_at_destination();
                        } else {
                            d.navigation.cancel_for_exclusive_view();
                        }
                    }
                    7 => {
                        let before = d.navigation.frame().phase;
                        if d.finish_navigation().is_some() {
                            finished += 1;
                            assert_eq!(before, NavigationTransitionPhase::Settled, "{at}");
                            assert_eq!(
                                d.chart.state(),
                                FullScreenTransitionState::Releasing,
                                "{at}"
                            );
                        }
                    }
                    _ => {
                        if let Some(generation) = d.chart.generation()
                            && d.chart.state() == FullScreenTransitionState::Releasing
                        {
                            d.chart.live_frame_presented(generation).unwrap();
                        }
                    }
                }
                check(&d, &mut seen, &at);
            }
        }
        assert!(finished > 100, "finished {finished}");
        assert!(unwound > 5, "unwound {unwound}");
        // The chart's state is not a function of the timeline's phase: an immediate
        // start plays Expand through Settled while the chart still awaits its
        // snapshot, and a locked snapshot spans every phase from Expand on.
        let expected: BTreeSet<String> = [
            "Live + Idle",
            "CapturePending + Capture",
            "CapturePending + Expand",
            "CapturePending + Covered",
            "CapturePending + Reversing",
            "CapturePending + Settled",
            "SnapshotLocked + Expand",
            "SnapshotLocked + Covered",
            "SnapshotLocked + Reveal",
            "SnapshotLocked + Reversing",
            "SnapshotLocked + Settled",
            "Releasing + Idle",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(seen, expected);
    }

    #[test]
    fn covering_a_playing_transition_ends_it_at_the_right_endpoint_and_releases_the_chart() {
        let source = frame(0x1111);
        let destination = frame(0x2222);
        for committed in [false, true] {
            let mut d = director();
            assert!(start(
                &mut d,
                NavigationTransitionEdge::HomeToConsoles,
                NavigationTransitionDirection::Forward,
                &source,
                0,
            ));
            let generation = d
                .chart
                .generation_for(FullScreenTransitionOwner::Navigation)
                .unwrap();
            d.chart.take_controlled_capture(generation).unwrap();
            assert!(d.capture_navigation_destination(&destination, 10_000));
            let endpoint = d.cover_navigation(committed);
            assert_eq!(
                endpoint,
                Some(if committed {
                    NavigationTransitionEndpoint::Destination
                } else {
                    NavigationTransitionEndpoint::Source
                })
            );
            assert!(!d.navigation.is_active(), "committed={committed}");
            assert_eq!(
                d.chart.state(),
                FullScreenTransitionState::Releasing,
                "committed={committed}"
            );
        }
    }
}
