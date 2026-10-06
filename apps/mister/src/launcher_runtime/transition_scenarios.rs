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

mod navigation {
    //! Navigation-level scenarios: the real `LauncherNav` plans every reveal, and
    //! the reverse of an edge must replay the geometry of its forward.

    use super::super::transition_plan::{
        NavigationDisplay, navigation_geometry, navigation_transition_for_intent,
    };
    use super::*;
    use crate::arcade_catalog::{ArcadeCatalog, MENU_ARCADE_SYSTEM_ID};
    use crate::launcher::{
        LauncherAction, LauncherEvent, LauncherNav, Screen, apply_launch_return_state,
        capture_launch_return_state,
    };
    use crate::test_support::{arcade_catalog, arcade_game, arcade_system};
    use mister_magik_framebuffer_scenes::navigation::{
        NavigationTransitionGeometry, NavigationTransitionRect,
    };

    const GAME: &str = "/media/fat/_Arcade/Pocket Tennis.mra";

    fn catalog() -> ArcadeCatalog {
        arcade_catalog(
            vec![
                arcade_game("Metal Slug").build(),
                arcade_game("Pocket Tennis")
                    .system_id("neogeopocket")
                    .build(),
                arcade_game("Sonic").system_id("gamegear").build(),
                arcade_game("Super Mario Bros").system_id("nes").build(),
            ],
            vec![
                arcade_system("arcade", 1),
                arcade_system("neogeopocket", 1),
                arcade_system("gamegear", 1),
                arcade_system("nes", 1),
            ],
        )
    }

    fn event(action: LauncherAction, path: Option<&str>) -> LauncherEvent {
        LauncherEvent {
            action,
            path: path.map(str::to_owned),
            settings: None,
        }
    }

    fn display() -> NavigationDisplay {
        NavigationDisplay {
            frame_width: 960,
            frame_height: 540,
            crt: None,
            card_home_rect: Some(NavigationTransitionRect {
                x: 360,
                y: 60,
                width: 240,
                height: 336,
            }),
        }
    }

    /// Plan `event` against the current state, as the loop does before committing it.
    fn plan(
        nav: &LauncherNav,
        event: &LauncherEvent,
    ) -> (
        NavigationTransitionEdge,
        NavigationTransitionDirection,
        NavigationTransitionGeometry,
    ) {
        let (edge, direction) = navigation_transition_for_intent(nav, event, false)
            .unwrap_or_else(|| panic!("{:?} plays a transition", event.action));
        (edge, direction, navigation_geometry(nav, &display(), edge))
    }

    fn commit(nav: &mut LauncherNav, event: &LauncherEvent, catalog: &ArcadeCatalog) {
        assert!(
            nav.commit_navigation_intent(event, catalog),
            "{:?}",
            event.action
        );
    }

    fn handhelds_nav(catalog: &ArcadeCatalog) -> LauncherNav {
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(catalog);
        assert!(nav.open_menu("handhelds"));
        nav.selected = nav
            .current_menu_items()
            .iter()
            .position(|item| item.id == "neogeopocket")
            .expect("NeoGeo Pocket tile");
        nav
    }

    #[test]
    fn root_arcade_back_replays_the_forward_geometry() {
        let catalog = catalog();
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        let open = event(LauncherAction::OpenCollection, Some(MENU_ARCADE_SYSTEM_ID));
        let (edge, direction, forward) = plan(&nav, &open);
        assert_eq!(
            (edge, direction),
            (
                NavigationTransitionEdge::HomeToArcade,
                NavigationTransitionDirection::Forward
            )
        );
        commit(&mut nav, &open, &catalog);
        assert_eq!(nav.screen, Screen::Arcade);

        let back = event(LauncherAction::NavigateBack, None);
        let (edge, direction, reverse) = plan(&nav, &back);
        assert_eq!(
            (edge, direction),
            (
                NavigationTransitionEdge::HomeToArcade,
                NavigationTransitionDirection::Reverse
            )
        );
        assert_eq!(reverse, forward);
    }

    #[test]
    fn nested_system_back_replays_the_forward_geometry() {
        let catalog = catalog();
        let mut nav = handhelds_nav(&catalog);
        let tile = nav.selected;
        let open = event(LauncherAction::OpenCollection, Some("neogeopocket"));
        let (edge, _, forward) = plan(&nav, &open);
        assert_eq!(edge, NavigationTransitionEdge::ConsolesToSystem);
        commit(&mut nav, &open, &catalog);
        assert_eq!(nav.screen, Screen::Arcade);
        assert_ne!(
            nav.selected, tile,
            "inside a collection `selected` is a catalog-system index, not the tile"
        );

        let back = event(LauncherAction::NavigateBack, None);
        let (edge, direction, reverse) = plan(&nav, &back);
        assert_eq!(
            (edge, direction),
            (
                NavigationTransitionEdge::ConsolesToSystem,
                NavigationTransitionDirection::Reverse
            )
        );
        assert_eq!(reverse, forward);

        // Leaving the level afterwards plays the Home-level reverse.
        commit(&mut nav, &back, &catalog);
        assert_eq!(nav.screen, Screen::Home);
        let (edge, direction, _) = plan(&nav, &event(LauncherAction::NavigateBack, None));
        assert_eq!(
            (edge, direction),
            (
                NavigationTransitionEdge::HomeToConsoles,
                NavigationTransitionDirection::Reverse
            )
        );
    }

    #[test]
    fn back_after_a_game_return_replays_the_forward_geometry() {
        let catalog = catalog();
        let mut nav = handhelds_nav(&catalog);
        let open = event(LauncherAction::OpenCollection, Some("neogeopocket"));
        let (_, _, forward) = plan(&nav, &open);
        commit(&mut nav, &open, &catalog);
        let state = capture_launch_return_state(&nav, &catalog, GAME).expect("return state");

        // The launch restarts the process: no transition runtime, no history.
        let mut restored = LauncherNav::new();
        assert!(apply_launch_return_state(&mut restored, &catalog, state));
        assert_eq!(restored.screen, Screen::Arcade);

        let (edge, direction, reverse) =
            plan(&restored, &event(LauncherAction::NavigateBack, None));
        assert_eq!(
            (edge, direction),
            (
                NavigationTransitionEdge::ConsolesToSystem,
                NavigationTransitionDirection::Reverse
            )
        );
        assert_eq!(reverse, forward);
    }

    #[test]
    fn every_back_and_home_intent_from_a_collection_plays_a_reveal() {
        let catalog = catalog();
        for action in [LauncherAction::NavigateBack, LauncherAction::NavigateHome] {
            let mut nav = handhelds_nav(&catalog);
            commit(
                &mut nav,
                &event(LauncherAction::OpenCollection, Some("neogeopocket")),
                &catalog,
            );
            let (_, direction, geometry) = plan(&nav, &event(action, None));
            assert_eq!(
                direction,
                NavigationTransitionDirection::Reverse,
                "{action:?}"
            );
            assert!(geometry.source_card.width > 0, "{action:?}");
        }
    }
}
