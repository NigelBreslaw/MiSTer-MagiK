// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Seeded random walks over the three things that decide what the launcher
//! shows during a full-screen transition: the navigation runtime, the
//! `FullScreenTransitionStateChart` and the `UiCompositionController`. The loop
//! derives the composition input from the runtime and never shows the chart to
//! composition, so this is where the contract between the two is measured
//! rather than assumed. The walk drives them as the loop does: a screensaver or
//! confirmation that appears over a playing navigation transition cancels it
//! (settling it at the destination if that was already committed) and releases
//! the chart before composition is asked.

use super::super::composition::{UiCompositionController, UiCompositionInput, UiCompositionState};
use super::super::full_screen_transition::{
    FullScreenTransitionOwner, FullScreenTransitionState, FullScreenTransitionStateChart,
};
use super::super::navigation_transition::NavigationTransitionRuntime;
use super::super::transition_scenarios::{DIRECTIONS, EDGES, HEIGHT, WIDTH, frame};
use super::super::walk_rng::WalkRng as Rng;
use super::tests::start;
use super::{capture_navigation_destination, finish_navigation_transition};
use crate::launcher::Screen;
use std::collections::BTreeSet;

const WALKS: usize = 300;
const STEPS: usize = 100;

#[test]
fn random_walks_measure_what_composition_shows_for_every_chart_state() {
    let source = frame(0x1111);
    let destination = frame(0x2222);
    let mut rng = Rng(0xC0DE_5EED_F00D_1234);
    let mut seen = BTreeSet::new();
    for walk in 0..WALKS {
        let mut runtime = NavigationTransitionRuntime::new(WIDTH, HEIGHT, true);
        let mut chart = FullScreenTransitionStateChart::default();
        let mut composition = UiCompositionController::new();
        let mut now_us = 0u64;
        let (mut screensaver, mut confirm) = (false, false);
        let mut committed = false;
        for step in 0..STEPS {
            let at = format!("walk {walk} step {step}");
            now_us += [0, 5_000, 40_000, 400_000, 10_000_000][rng.below(5)];
            match rng.below(14) {
                0 | 1 => {
                    let edge = EDGES[rng.below(EDGES.len())];
                    let direction = DIRECTIONS[rng.below(DIRECTIONS.len())];
                    if start(&mut runtime, &mut chart, edge, direction, &source, now_us) {
                        committed = rng.chance(30);
                    }
                }
                2 => {
                    if let Some(generation) =
                        chart.generation_for(FullScreenTransitionOwner::Navigation)
                        && chart.state() == FullScreenTransitionState::CapturePending
                    {
                        chart.take_controlled_capture(generation).unwrap();
                    }
                }
                3 | 4 => {
                    capture_navigation_destination(&mut runtime, &mut chart, &destination, now_us);
                }
                5 => {
                    runtime.tick(now_us);
                }
                6 => {
                    runtime.request_reverse(now_us);
                }
                7 => {
                    if runtime.frame().phase
                        == super::super::navigation_transition::NavigationTransitionPhase::Settled
                    {
                        finish_navigation_transition(&mut runtime, &mut chart);
                    }
                }
                8 => {
                    if let Some(generation) = chart.generation()
                        && chart.state() == FullScreenTransitionState::Releasing
                    {
                        chart.live_frame_presented(generation).unwrap();
                    }
                }
                9 => screensaver = !screensaver && rng.chance(60),
                10 => confirm = !confirm && rng.chance(60),
                11 => {
                    // Orientation takes the chart when nothing else holds it.
                    if let Ok(generation) = chart.begin(FullScreenTransitionOwner::Orientation) {
                        if rng.chance(50) {
                            chart.take_controlled_capture(generation).unwrap();
                            chart.capture_completed(generation).unwrap();
                        }
                        if rng.chance(50) {
                            chart.release(generation).unwrap();
                        }
                    }
                }
                12 => {
                    if chart.owner() == Some(FullScreenTransitionOwner::Orientation)
                        && let Some(generation) = chart.generation()
                    {
                        chart.release(generation).unwrap();
                    }
                }
                _ => {}
            }

            // The loop's exclusive-view rule, applied before composition is asked.
            if runtime.is_active() && (screensaver || confirm) {
                if committed {
                    runtime.settle_at_destination();
                } else {
                    runtime.cancel_for_exclusive_view();
                }
                finish_navigation_transition(&mut runtime, &mut chart);
            }

            let decision = composition.tick(UiCompositionInput {
                screensaver_active: screensaver,
                navigation_transition_active: runtime.is_active(),
                navigation_destination_committed: committed,
                navigation_destination_ready: runtime.destination_ready(),
                navigation_destination_layers_ready: rng.chance(60),
                return_screen: Some(if rng.chance(50) {
                    Screen::Home
                } else {
                    Screen::Arcade
                }),
                confirm_visible: confirm,
                fullscreen_overlay_visible: false,
                arcade_ready: true,
                route_ok: true,
                wants_arcade_list: false,
                wants_preview: false,
                preview_cache_exact: false,
                preview_frame_ready: false,
            });

            let nav_state = matches!(
                decision.state,
                UiCompositionState::NavigationTransition
                    | UiCompositionState::NavigationDestination
            );
            // Composition shows a navigation transition exactly while the
            // runtime plays one and nothing exclusive covers it, and then the
            // chart holds the frame for navigation.
            assert_eq!(nav_state, runtime.is_active(), "{at}");
            if nav_state {
                assert_eq!(
                    chart.owner(),
                    Some(FullScreenTransitionOwner::Navigation),
                    "{at}"
                );
                assert!(
                    matches!(
                        chart.state(),
                        FullScreenTransitionState::CapturePending
                            | FullScreenTransitionState::SnapshotLocked
                    ),
                    "{at}: {:?}",
                    chart.state()
                );
            }
            // And the converse: a chart that is not releasing holds the frame for
            // navigation only while the runtime really plays.
            if chart.owner() == Some(FullScreenTransitionOwner::Navigation)
                && chart.state() != FullScreenTransitionState::Releasing
            {
                assert!(runtime.is_active(), "{at}: the chart waits on nothing");
            }
            // The destination is only awaited while the snapshot is unlocked.
            if decision.state == UiCompositionState::NavigationDestination {
                assert_eq!(
                    chart.state(),
                    FullScreenTransitionState::CapturePending,
                    "{at}"
                );
            }
            seen.insert(format!(
                "{:?}/{:?} + {}",
                chart.owner(),
                chart.state(),
                decision.state.label()
            ));
        }
    }
    // Reaching these is what makes the assertions above mean something. The last
    // is a finding, not a goal: the chart can be releasing (it asks for a live
    // Slint raster) while composition shows the screensaver, which the loop's
    // render ladder serves first, so the release waits for the screensaver.
    for pair in [
        "Some(Navigation)/CapturePending + navigation-transition",
        "Some(Navigation)/CapturePending + navigation-destination",
        "Some(Navigation)/SnapshotLocked + navigation-transition",
        "Some(Navigation)/Releasing + full-slint",
        "Some(Navigation)/Releasing + screensaver",
        "Some(Orientation)/SnapshotLocked + full-slint",
    ] {
        assert!(seen.contains(pair), "never reached {pair}: {seen:#?}");
    }
    assert!(
        !seen.contains("Some(Navigation)/SnapshotLocked + navigation-destination"),
        "a locked snapshot is not awaiting its destination"
    );
}
