// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Seeded random walks over the three things that decide what the launcher
//! shows during a full-screen transition: the navigation runtime, the
//! `FullScreenTransitionStateChart` and the `UiCompositionController`, all owned
//! by the `PresentationDirector`. The loop derives the composition input from the
//! runtime and does not show the chart to composition, so this is where the
//! contract between the two is measured rather than assumed. The walk drives the
//! director as the loop does: a screensaver or confirmation that appears over a
//! playing navigation transition covers it (`cover_navigation`) before
//! composition is asked.

use super::super::composition::UiCompositionState;
use super::super::full_screen_transition::{FullScreenTransitionOwner, FullScreenTransitionState};
use super::super::navigation_transition::NavigationTransitionPhase;
use super::super::transition_scenarios::{DIRECTIONS, EDGES, frame};
use super::super::walk_rng::WalkRng as Rng;
use super::CompositionRequest;
use super::tests::{director, start};
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
        let mut d = director();
        let mut now_us = 0u64;
        let (mut screensaver, mut confirm) = (false, false);
        for step in 0..STEPS {
            let at = format!("walk {walk} step {step}");
            now_us += [0, 5_000, 40_000, 400_000, 10_000_000][rng.below(5)];
            match rng.below(14) {
                0 | 1 => {
                    let edge = EDGES[rng.below(EDGES.len())];
                    let direction = DIRECTIONS[rng.below(DIRECTIONS.len())];
                    if start(&mut d, edge, direction, &source, now_us) {
                        d.pending.as_mut().unwrap().committed = rng.chance(30);
                    }
                }
                2 => {
                    if let Some(generation) = d
                        .chart
                        .generation_for(FullScreenTransitionOwner::Navigation)
                        && d.chart.state() == FullScreenTransitionState::CapturePending
                    {
                        d.chart.take_controlled_capture(generation).unwrap();
                    }
                }
                3 | 4 => {
                    d.capture_navigation_destination(&destination, now_us);
                }
                5 => {
                    d.navigation.tick(now_us);
                }
                6 => {
                    d.navigation.request_reverse(now_us);
                }
                7 => {
                    if d.navigation.frame().phase == NavigationTransitionPhase::Settled {
                        d.finish_navigation();
                    }
                }
                8 => {
                    if let Some(generation) = d.chart.generation()
                        && d.chart.state() == FullScreenTransitionState::Releasing
                    {
                        d.chart.live_frame_presented(generation).unwrap();
                    }
                }
                9 => screensaver = !screensaver && rng.chance(60),
                10 => confirm = !confirm && rng.chance(60),
                11 => {
                    // Orientation takes the chart when nothing else holds it.
                    if let Ok(generation) = d.chart.begin(FullScreenTransitionOwner::Orientation) {
                        if rng.chance(50) {
                            d.chart.take_controlled_capture(generation).unwrap();
                            d.chart.capture_completed(generation).unwrap();
                        }
                        if rng.chance(50) {
                            d.chart.release(generation).unwrap();
                        }
                    }
                }
                12 => {
                    if d.chart.owner() == Some(FullScreenTransitionOwner::Orientation)
                        && let Some(generation) = d.chart.generation()
                    {
                        d.chart.release(generation).unwrap();
                    }
                }
                _ => {}
            }

            // The loop's exclusive-view rule, applied before composition is asked.
            if d.navigation.is_active() && (screensaver || confirm) {
                d.cover_navigation();
            }

            let decision = d.compose(CompositionRequest {
                screensaver_active: screensaver,
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
            assert_eq!(nav_state, d.navigation.is_active(), "{at}");
            if nav_state {
                assert_eq!(
                    d.chart.owner(),
                    Some(FullScreenTransitionOwner::Navigation),
                    "{at}"
                );
                assert!(
                    matches!(
                        d.chart.state(),
                        FullScreenTransitionState::CapturePending
                            | FullScreenTransitionState::SnapshotLocked
                    ),
                    "{at}: {:?}",
                    d.chart.state()
                );
            }
            // And the converse: a chart that is not releasing holds the frame for
            // navigation only while the runtime really plays.
            if d.chart.owner() == Some(FullScreenTransitionOwner::Navigation)
                && d.chart.state() != FullScreenTransitionState::Releasing
            {
                assert!(d.navigation.is_active(), "{at}: the chart waits on nothing");
            }
            // The destination is only awaited while the snapshot is unlocked.
            if decision.state == UiCompositionState::NavigationDestination {
                assert_eq!(
                    d.chart.state(),
                    FullScreenTransitionState::CapturePending,
                    "{at}"
                );
            }
            seen.insert(format!(
                "{:?}/{:?} + {}",
                d.chart.owner(),
                d.chart.state(),
                decision.state.label()
            ));
        }
    }
    // Reaching these is what makes the assertions above mean something. The
    // screensaver pair is the documented rule (docs/architecture.md): the chart
    // can be releasing while composition shows the screensaver, which the render
    // ladder serves first, so the release waits for it and blocks nothing.
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
