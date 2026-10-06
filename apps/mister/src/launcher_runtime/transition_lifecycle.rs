// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! The operations that move a full-screen transition's own timeline and the
//! `FullScreenTransitionStateChart` together. The launcher loop calls these
//! instead of pairing the two by hand, so the pairing is written once and a
//! host test can walk it.

use super::full_screen_transition::{FullScreenTransitionOwner, FullScreenTransitionStateChart};
use super::navigation_transition::{NavigationTransitionCompletion, NavigationTransitionRuntime};
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

/// A navigation transition the runtime started but the chart refused to host:
/// settle it where it is going and drop it, as if it had never played.
pub fn unwind_navigation_transition(runtime: &mut NavigationTransitionRuntime) {
    runtime.settle_at_destination();
    let _ = runtime.complete();
}

/// Hand the runtime the captured destination and lock the chart's snapshot.
/// If either refuses, the transition settles at its destination. Returns whether
/// the capture took.
pub fn capture_navigation_destination(
    runtime: &mut NavigationTransitionRuntime,
    chart: &mut FullScreenTransitionStateChart,
    destination: &[Rgb565Pixel],
    now_us: u64,
) -> bool {
    let captured = runtime.capture_destination(destination, now_us).is_ok()
        && chart
            .generation_for(FullScreenTransitionOwner::Navigation)
            .is_none_or(|generation| chart.capture_completed(generation).is_ok());
    if !captured {
        runtime.settle_at_destination();
    }
    captured
}

/// End a settled navigation transition: take its completion and, if there was
/// one, release the chart.
pub fn finish_navigation_transition(
    runtime: &mut NavigationTransitionRuntime,
    chart: &mut FullScreenTransitionStateChart,
) -> Option<NavigationTransitionCompletion> {
    let completion = runtime.complete();
    if completion.is_some() {
        release_full_screen_transition(chart, FullScreenTransitionOwner::Navigation);
    }
    completion
}

#[cfg(test)]
mod tests {
    use super::super::full_screen_transition::FullScreenTransitionState;
    use super::super::navigation_transition::{
        NavigationTransitionDirection, NavigationTransitionEdge, NavigationTransitionEndpoint,
        NavigationTransitionPhase,
    };
    use super::super::transition_scenarios::{DIRECTIONS, EDGES, HEIGHT, WIDTH, begin, frame};
    use super::super::walk_rng::WalkRng as Rng;
    use super::*;
    use std::collections::BTreeSet;

    const WALKS: usize = 300;
    const STEPS: usize = 80;

    /// Start the way the loop does: the runtime first, then the chart, and a
    /// transition the chart refuses is unwound.
    fn start(
        runtime: &mut NavigationTransitionRuntime,
        chart: &mut FullScreenTransitionStateChart,
        edge: NavigationTransitionEdge,
        direction: NavigationTransitionDirection,
        source: &[Rgb565Pixel],
        now_us: u64,
    ) -> bool {
        if !begin(runtime, edge, direction, source, now_us) {
            return false;
        }
        if begin_full_screen_transition(chart, FullScreenTransitionOwner::Navigation) {
            true
        } else {
            unwind_navigation_transition(runtime);
            false
        }
    }

    /// The runtime and the chart agree on who is playing and which stage it is in.
    fn check(
        runtime: &NavigationTransitionRuntime,
        chart: &FullScreenTransitionStateChart,
        seen: &mut BTreeSet<String>,
        at: &str,
    ) {
        use FullScreenTransitionState::*;
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
            let mut runtime = NavigationTransitionRuntime::new(WIDTH, HEIGHT, true);
            let mut chart = FullScreenTransitionStateChart::default();
            let mut now_us = 0u64;
            for step in 0..STEPS {
                let at = format!("walk {walk} step {step}");
                now_us += [0, 5_000, 40_000, 400_000, 10_000_000][rng.below(5)];
                match rng.below(10) {
                    9 => {
                        // The loop authorizes the controlled capture before it renders it.
                        if let Some(generation) =
                            chart.generation_for(FullScreenTransitionOwner::Navigation)
                            && chart.state() == FullScreenTransitionState::CapturePending
                        {
                            chart.take_controlled_capture(generation).unwrap();
                        }
                    }
                    0 | 1 => {
                        let edge = EDGES[rng.below(EDGES.len())];
                        let direction = DIRECTIONS[rng.below(DIRECTIONS.len())];
                        let chart_was_free = chart.is_live();
                        let started =
                            start(&mut runtime, &mut chart, edge, direction, &source, now_us);
                        if !started && !runtime.is_active() && !chart_was_free {
                            unwound += 1;
                        }
                        assert!(
                            !started || chart_was_free,
                            "{at}: began over a releasing chart"
                        );
                    }
                    2 | 3 => {
                        capture_navigation_destination(
                            &mut runtime,
                            &mut chart,
                            &destination,
                            now_us,
                        );
                    }
                    4 => {
                        runtime.tick(now_us);
                    }
                    5 => {
                        runtime.request_reverse(now_us);
                    }
                    6 => {
                        if rng.chance(50) {
                            runtime.settle_at_destination();
                        } else {
                            runtime.cancel_for_exclusive_view();
                        }
                    }
                    7 => {
                        let before = runtime.frame().phase;
                        let completion = finish_navigation_transition(&mut runtime, &mut chart);
                        if let Some(completion) = completion {
                            finished += 1;
                            assert_eq!(before, NavigationTransitionPhase::Settled, "{at}");
                            assert!(
                                matches!(
                                    completion.endpoint,
                                    NavigationTransitionEndpoint::Source
                                        | NavigationTransitionEndpoint::Destination
                                ),
                                "{at}"
                            );
                            assert_eq!(chart.state(), FullScreenTransitionState::Releasing, "{at}");
                        }
                    }
                    _ => {
                        if let Some(generation) = chart.generation()
                            && chart.state() == FullScreenTransitionState::Releasing
                        {
                            chart.live_frame_presented(generation).unwrap();
                        }
                    }
                }
                check(&runtime, &chart, &mut seen, &at);
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
}
