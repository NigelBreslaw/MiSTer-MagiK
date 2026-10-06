// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Scenario matrix for the shared navigation-transition contract.
//!
//! Every card-capable edge is driven in both directions through the real
//! runtime from a cold start (no remembered forward transition, as after a game
//! return). The contract each run must meet:
//!
//! * source continuity: the first frame is exactly the supplied source;
//! * endpoint continuity: the settled frame is exactly the destination capture;
//! * reversibility: reversing mid-flight settles on the source;
//! * hygiene: nothing is left active, so the next transition can begin.

use super::navigation_transition::{
    NavigationTransitionDirection, NavigationTransitionEdge, NavigationTransitionEndpoint,
    NavigationTransitionPhase, NavigationTransitionRuntime,
};
use super::transition_plan::{NavigationGeometryContext, derive_navigation_geometry};
use crate::launcher_presentation::{device_reveal_spec, system_device_rgb565};
use slint::platform::software_renderer::Rgb565Pixel;

const WIDTH: usize = 960;
const HEIGHT: usize = 540;
const EDGES: [NavigationTransitionEdge; 3] = [
    NavigationTransitionEdge::HomeToConsoles,
    NavigationTransitionEdge::HomeToArcade,
    NavigationTransitionEdge::ConsolesToSystem,
];
const DIRECTIONS: [NavigationTransitionDirection; 2] = [
    NavigationTransitionDirection::Forward,
    NavigationTransitionDirection::Reverse,
];
/// Far beyond any transition's duration, so one tick reaches its endpoint.
const SETTLE_US: u64 = 10_000_000;

fn frame(value: u16) -> Vec<Rgb565Pixel> {
    vec![Rgb565Pixel(value); WIDTH * HEIGHT]
}

fn begin(
    runtime: &mut NavigationTransitionRuntime,
    edge: NavigationTransitionEdge,
    direction: NavigationTransitionDirection,
    source: &[Rgb565Pixel],
    now_us: u64,
) -> bool {
    let geometry = derive_navigation_geometry(
        &NavigationGeometryContext {
            frame_width: WIDTH,
            frame_height: HEIGHT,
            crt: None,
            selected: 1,
            scroll_x: 0,
            item_count: 4,
            root_menu: edge != NavigationTransitionEdge::ConsolesToSystem,
            selected_label: "Arcade",
            card_home_rect: None,
        },
        edge,
    );
    match edge {
        NavigationTransitionEdge::HomeToArcade | NavigationTransitionEdge::ConsolesToSystem => {
            runtime.begin_device_card(
                edge,
                direction,
                geometry,
                device_reveal_spec(None, false, true),
                source,
                system_device_rgb565(None),
                // The reveal composites over a frame-sized backdrop capture.
                source,
                now_us,
            )
        }
        _ => runtime.begin(edge, direction, geometry, source, now_us),
    }
    .unwrap()
}

#[test]
fn every_edge_and_direction_starts_cold_and_meets_the_continuity_contract() {
    let source = frame(0x1111);
    let destination = frame(0x2222);
    for edge in EDGES {
        for direction in DIRECTIONS {
            let label = format!("{edge:?} {direction:?}");
            let mut runtime = NavigationTransitionRuntime::new(WIDTH, HEIGHT, true);
            assert!(
                begin(&mut runtime, edge, direction, &source, 0),
                "{label}: begin"
            );
            assert!(
                runtime
                    .render()
                    .unwrap_or_else(|e| panic!("{label}: {e:?}"))
                    == source,
                "{label}: source continuity"
            );

            runtime.capture_destination(&destination, 20_000).unwrap();
            assert_eq!(
                runtime.tick(20_000 + SETTLE_US).phase,
                NavigationTransitionPhase::Settled,
                "{label}: settles"
            );
            assert!(
                runtime
                    .render()
                    .unwrap_or_else(|e| panic!("{label}: {e:?}"))
                    == destination,
                "{label}: endpoint continuity"
            );
            let completion = runtime.complete().expect("completion");
            assert_eq!(
                completion.endpoint,
                NavigationTransitionEndpoint::Destination
            );
            assert!(!runtime.is_active(), "{label}: released");

            assert!(
                begin(&mut runtime, edge, direction, &destination, SETTLE_US * 2),
                "{label}: runtime is reusable"
            );
        }
    }
}

#[test]
fn reversing_in_flight_settles_on_the_source_for_every_edge_and_direction() {
    let source = frame(0x1111);
    let destination = frame(0x2222);
    for edge in EDGES {
        for direction in DIRECTIONS {
            let label = format!("{edge:?} {direction:?}");
            let mut runtime = NavigationTransitionRuntime::new(WIDTH, HEIGHT, true);
            assert!(
                begin(&mut runtime, edge, direction, &source, 0),
                "{label}: begin"
            );
            runtime.capture_destination(&destination, 20_000).unwrap();
            runtime.tick(120_000);
            assert!(runtime.request_reverse(120_000), "{label}: reversible");
            runtime.tick(120_000 + SETTLE_US);
            assert_eq!(
                runtime.frame().endpoint,
                Some(NavigationTransitionEndpoint::Source),
                "{label}: reverses to source"
            );
            assert!(
                runtime
                    .render()
                    .unwrap_or_else(|e| panic!("{label}: {e:?}"))
                    == source,
                "{label}: source pixels"
            );
            let completion = runtime.complete().expect("completion");
            assert_eq!(completion.endpoint, NavigationTransitionEndpoint::Source);
            assert!(!runtime.is_active(), "{label}: released");
        }
    }
}
