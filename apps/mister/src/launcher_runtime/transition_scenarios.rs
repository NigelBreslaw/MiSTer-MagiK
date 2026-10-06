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
use super::transition_plan::{NavigationDisplay, TileView, derive_navigation_geometry};
use super::transition_spec::TransitionStart;
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
        &NavigationDisplay {
            frame_width: WIDTH,
            frame_height: HEIGHT,
            crt: None,
            card_home_rect: None,
        },
        &TileView {
            selected: 1,
            scroll_x: 0,
            item_count: 4,
            root_menu: edge != NavigationTransitionEdge::ConsolesToSystem,
            label: "Arcade",
        },
        edge,
    );
    match edge {
        NavigationTransitionEdge::HomeToArcade | NavigationTransitionEdge::ConsolesToSystem => {
            runtime.begin(TransitionStart::device_card(
                edge,
                direction,
                geometry,
                device_reveal_spec(None, false, true),
                source,
                system_device_rgb565(None),
                // The reveal composites over a frame-sized backdrop capture.
                source,
                now_us,
            ))
        }
        _ => runtime.begin(TransitionStart::super_scaler(
            edge, direction, geometry, source, now_us,
        )),
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

    /// A launchable game in `collection`, as the catalog fixture names them.
    fn game_in(collection: &str) -> &'static str {
        match collection {
            MENU_ARCADE_SYSTEM_ID => "/media/fat/_Arcade/Metal Slug.mra",
            "neogeopocket" => GAME,
            "gamegear" => "/media/fat/_Arcade/Sonic.mra",
            "nes" => "/media/fat/_Arcade/Super Mario Bros.mra",
            other => panic!("no fixture game for {other}"),
        }
    }

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

    /// With `card_home` the carousel supplies the source card's rectangle, as on
    /// device; without it the tile geometry alone must agree between directions.
    fn display(card_home: bool) -> NavigationDisplay {
        NavigationDisplay {
            frame_width: 960,
            frame_height: 540,
            crt: None,
            card_home_rect: card_home.then_some(NavigationTransitionRect {
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
        card_home: bool,
    ) -> (
        NavigationTransitionEdge,
        NavigationTransitionDirection,
        NavigationTransitionGeometry,
    ) {
        let (edge, direction) = navigation_transition_for_intent(nav, event, false)
            .unwrap_or_else(|| panic!("{:?} plays a transition", event.action));
        (
            edge,
            direction,
            navigation_geometry(nav, &display(card_home), edge),
        )
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
        for card_home in [false, true] {
            let catalog = catalog();
            let mut nav = LauncherNav::new();
            nav.sync_launcher_taxonomy(&catalog);
            let open = event(LauncherAction::OpenCollection, Some(MENU_ARCADE_SYSTEM_ID));
            let (edge, direction, forward) = plan(&nav, &open, card_home);
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
            let (edge, direction, reverse) = plan(&nav, &back, card_home);
            assert_eq!(
                (edge, direction),
                (
                    NavigationTransitionEdge::HomeToArcade,
                    NavigationTransitionDirection::Reverse
                )
            );
            assert_eq!(reverse, forward, "card_home={card_home}");
        }
    }

    #[test]
    fn nested_system_back_replays_the_forward_geometry() {
        for card_home in [false, true] {
            let catalog = catalog();
            let mut nav = handhelds_nav(&catalog);
            let tile = nav.selected;
            let open = event(LauncherAction::OpenCollection, Some("neogeopocket"));
            let (edge, _, forward) = plan(&nav, &open, card_home);
            assert_eq!(edge, NavigationTransitionEdge::ConsolesToSystem);
            commit(&mut nav, &open, &catalog);
            assert_eq!(nav.screen, Screen::Arcade);
            assert_ne!(
                nav.selected, tile,
                "inside a collection `selected` is a catalog-system index, not the tile"
            );

            let back = event(LauncherAction::NavigateBack, None);
            let (edge, direction, reverse) = plan(&nav, &back, card_home);
            assert_eq!(
                (edge, direction),
                (
                    NavigationTransitionEdge::ConsolesToSystem,
                    NavigationTransitionDirection::Reverse
                )
            );
            assert_eq!(reverse, forward, "card_home={card_home}");

            // Leaving the level afterwards plays the Home-level reverse.
            commit(&mut nav, &back, &catalog);
            assert_eq!(nav.screen, Screen::Home);
            let (edge, direction, _) =
                plan(&nav, &event(LauncherAction::NavigateBack, None), card_home);
            assert_eq!(
                (edge, direction),
                (
                    NavigationTransitionEdge::HomeToConsoles,
                    NavigationTransitionDirection::Reverse
                )
            );
        }
    }

    #[test]
    fn back_after_a_game_return_replays_the_forward_geometry() {
        for card_home in [false, true] {
            let catalog = catalog();
            let mut nav = handhelds_nav(&catalog);
            let open = event(LauncherAction::OpenCollection, Some("neogeopocket"));
            let (_, _, forward) = plan(&nav, &open, card_home);
            commit(&mut nav, &open, &catalog);
            let state = capture_launch_return_state(&nav, &catalog, GAME).expect("return state");

            // The launch restarts the process: no transition runtime, no history.
            let mut restored = LauncherNav::new();
            assert!(apply_launch_return_state(&mut restored, &catalog, state));
            assert_eq!(restored.screen, Screen::Arcade);

            let (edge, direction, reverse) = plan(
                &restored,
                &event(LauncherAction::NavigateBack, None),
                card_home,
            );
            assert_eq!(
                (edge, direction),
                (
                    NavigationTransitionEdge::ConsolesToSystem,
                    NavigationTransitionDirection::Reverse
                )
            );
            assert_eq!(reverse, forward, "card_home={card_home}");
        }
    }

    #[test]
    fn back_and_home_from_a_collection_play_the_same_reverse_reveal() {
        let catalog = catalog();
        let mut planned = Vec::new();
        for action in [LauncherAction::NavigateBack, LauncherAction::NavigateHome] {
            let mut nav = handhelds_nav(&catalog);
            let open = event(LauncherAction::OpenCollection, Some("neogeopocket"));
            let (_, _, forward) = plan(&nav, &open, false);
            commit(&mut nav, &open, &catalog);
            let (edge, direction, geometry) = plan(&nav, &event(action, None), false);
            assert_eq!(
                (edge, direction),
                (
                    NavigationTransitionEdge::ConsolesToSystem,
                    NavigationTransitionDirection::Reverse
                ),
                "{action:?}"
            );
            assert_eq!(geometry, forward, "{action:?}");
            planned.push(geometry);
        }
        assert_eq!(planned[0], planned[1]);
    }

    /// Seeded random walks over the real navigation state. Every reveal out of a
    /// collection, by Back or Home and whether or not the process restarted for a
    /// game in between, must replay the geometry its forward reveal planned.
    #[test]
    fn random_walks_keep_every_reverse_reveal_symmetric_with_its_forward() {
        use crate::launcher_taxonomy::LauncherMenuItemKind;

        let catalog = catalog();
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };
        let mut collections_entered = 0;
        let mut restarts = 0;
        for walk in 0..200 {
            let card_home = walk % 2 == 0;
            let mut nav = LauncherNav::new();
            nav.sync_launcher_taxonomy(&catalog);
            let mut entered = None;
            for _ in 0..40 {
                if nav.screen == Screen::Arcade {
                    let (edge, forward): (NavigationTransitionEdge, NavigationTransitionGeometry) =
                        entered
                            .take()
                            .expect("Arcade is only reached by a planned reveal");
                    if next(3) == 0 {
                        // A game launch restarts the process: only saved state survives.
                        let game = game_in(nav.active_collection_id().expect("a collection"));
                        let state = capture_launch_return_state(&nav, &catalog, game);
                        if let Some(state) = state {
                            let mut restored = LauncherNav::new();
                            assert!(apply_launch_return_state(&mut restored, &catalog, state));
                            nav = restored;
                            restarts += 1;
                        }
                    }
                    let action = if next(2) == 0 {
                        LauncherAction::NavigateBack
                    } else {
                        LauncherAction::NavigateHome
                    };
                    let leave = event(action, None);
                    let (reverse_edge, direction, reverse) = plan(&nav, &leave, card_home);
                    assert_eq!(direction, NavigationTransitionDirection::Reverse);
                    assert_eq!(reverse_edge, edge, "walk {walk}");
                    assert_eq!(reverse, forward, "walk {walk} {action:?}");
                    commit(&mut nav, &leave, &catalog);
                    continue;
                }
                let items = nav.current_menu_items().to_vec();
                match next(4) {
                    0 => {
                        let _ = nav.commit_navigation_intent(
                            &event(LauncherAction::NavigateBack, None),
                            &catalog,
                        );
                    }
                    1 => {
                        let _ = nav.commit_navigation_intent(
                            &event(LauncherAction::NavigateHome, None),
                            &catalog,
                        );
                    }
                    _ => {
                        let Some(item) = (!items.is_empty()).then(|| &items[next(items.len())])
                        else {
                            continue;
                        };
                        // The loop selects the tile before it activates it.
                        nav.selected = items.iter().position(|i| i.id == item.id).unwrap();
                        match item.kind {
                            LauncherMenuItemKind::Menu => {
                                let _ = nav.commit_navigation_intent(
                                    &event(LauncherAction::OpenMenu, Some(&item.id)),
                                    &catalog,
                                );
                            }
                            LauncherMenuItemKind::Collection => {
                                let open = event(LauncherAction::OpenCollection, Some(&item.id));
                                let (edge, _, forward) = plan(&nav, &open, card_home);
                                if nav.commit_navigation_intent(&open, &catalog)
                                    && nav.screen == Screen::Arcade
                                {
                                    entered = Some((edge, forward));
                                    collections_entered += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(collections_entered > 100, "{collections_entered}");
        assert!(restarts > 0, "the sweep never exercised a restart");
    }
}
