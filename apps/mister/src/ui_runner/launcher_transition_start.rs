// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Starting a navigation transition: the pixels it begins from, its geometry, and
//! the renderer that plays it. The launcher loop supplies plain inputs, so the rules
//! here run on the host exactly as on the device.

use super::launcher_card_home::LauncherCardHomeSession;
use super::launcher_loop::card_pixels_as_slint;
use super::*;
use crate::ui_display::{CrtUiMetrics, UiLayoutGeometry};
use mister_magik_framebuffer_scenes::device_card::RevealImage;

/// Everything a reveal needs to begin, besides the runtime and the card session.
pub(super) struct TransitionInputs<'a> {
    pub edge: NavigationTransitionEdge,
    pub direction: NavigationTransitionDirection,
    pub nav: &'a LauncherNav,
    /// The collection an `OpenCollection` intent names, which picks the device art.
    pub collection_id: Option<&'a str>,
    pub layout: UiLayoutGeometry,
    pub crt_layout: bool,
    pub crt_metrics: &'a CrtUiMetrics,
    /// The composed RGB565 cache: the visible frame whenever card-home is not.
    pub composed: &'a [Rgb565Pixel],
    pub crt_backdrop: &'a [Rgb565Pixel],
    pub now_us: u64,
}

/// Begin the full-screen transition for `inputs.edge`. Returns whether it started.
///
/// The reveal begins from the pixels the user is looking at: the card-home frame
/// while it owns Home in a landscape raster, otherwise the composed cache.
/// Portrait transitions run in the physical composition raster, which card-home
/// does not draw, so they always begin from the composed cache.
pub(super) fn begin_navigation_transition(
    runtime: &mut NavigationTransitionRuntime,
    cards: Option<&mut LauncherCardHomeSession>,
    inputs: &TransitionInputs<'_>,
    reveal_image: impl FnOnce() -> Option<RevealImage>,
) -> bool {
    let TransitionInputs {
        edge,
        direction,
        nav,
        collection_id,
        layout,
        crt_layout,
        crt_metrics,
        composed,
        crt_backdrop,
        now_us,
    } = *inputs;
    let portrait = layout.is_portrait();
    if edge == NavigationTransitionEdge::SystemPanel {
        // The system hub/list slide has no portrait form.
        return !portrait
            && runtime
                .begin_system_panel(
                    crt_layout,
                    nav.is_system_hub(),
                    composed,
                    crt_backdrop,
                    now_us,
                )
                .unwrap_or(false);
    }
    let card_edge = is_card_edge(edge) && !portrait;
    let geometry = navigation_geometry(
        nav,
        &NavigationDisplay {
            frame_width: layout.logical_w(),
            frame_height: layout.logical_h(),
            crt: crt_layout.then(|| crt_navigation_layout(layout.content_rect(), crt_metrics)),
            card_home_rect: cards
                .as_deref()
                .filter(|_| card_edge)
                .map(LauncherCardHomeSession::selected_card_rect),
        },
        edge,
    );
    let use_card_reveal = card_edge && cards.is_some();
    let source: &[Rgb565Pixel] = match cards {
        Some(cards)
            if !portrait && card_home_owns_source(nav.screen, cards.owns_visible_frame()) =>
        {
            card_pixels_as_slint(cards.render())
        }
        _ => composed,
    };
    let started = if use_card_reveal {
        let kind = match collection_id {
            Some(id) => nav.device_kind_for_collection(id),
            None => nav.device_kind(),
        };
        let hub = direction == NavigationTransitionDirection::Forward || nav.is_system_hub();
        runtime.begin_device_card(
            edge,
            direction,
            geometry,
            crate::launcher_presentation::device_reveal_spec(kind, crt_layout, hub),
            source,
            crate::launcher_presentation::system_device_rgb565(kind),
            crt_backdrop,
            now_us,
        )
    } else if portrait {
        runtime.begin_physical(
            edge,
            direction,
            navigation_geometry_to_composition(layout, geometry),
            layout.composition_w(),
            layout.composition_h(),
            source,
            now_us,
        )
    } else {
        runtime.begin(edge, direction, geometry, source, now_us)
    };
    if started.as_ref().is_ok_and(|started| *started)
        && crt_layout
        && direction == NavigationTransitionDirection::Reverse
    {
        runtime.update_device_reveal_image(reveal_image());
    }
    started.unwrap_or(false)
}

pub(super) fn navigation_geometry_to_composition(
    layout: UiLayoutGeometry,
    mut geometry: NavigationTransitionGeometry,
) -> NavigationTransitionGeometry {
    fn map_rect(
        layout: UiLayoutGeometry,
        rect: NavigationTransitionRect,
    ) -> NavigationTransitionRect {
        if rect.width == 0 || rect.height == 0 {
            return rect;
        }
        let mapped = layout.logical_rect_to_composition(DirtyRect {
            x0: rect.x as usize,
            y0: rect.y as usize,
            x1: rect.right() as usize,
            y1: rect.bottom() as usize,
        });
        NavigationTransitionRect {
            x: mapped.x0.min(u16::MAX as usize) as u16,
            y: mapped.y0.min(u16::MAX as usize) as u16,
            width: mapped.width().min(u16::MAX as usize) as u16,
            height: mapped.rows().min(u32::from(u16::MAX)) as u16,
        }
    }

    geometry.source_card = map_rect(layout, geometry.source_card);
    geometry.source_label = map_rect(layout, geometry.source_label);
    geometry.source_detail = map_rect(layout, geometry.source_detail);
    geometry.destination_title = map_rect(layout, geometry.destination_title);
    geometry.destination_detail = map_rect(layout, geometry.destination_detail);
    geometry.destination_list = map_rect(layout, geometry.destination_list);
    geometry.destination_selected_row = map_rect(layout, geometry.destination_selected_row);
    geometry.destination_preview = map_rect(layout, geometry.destination_preview);
    geometry.destination_footer = map_rect(layout, geometry.destination_footer);
    geometry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arcade_catalog::MENU_ARCADE_SYSTEM_ID;
    use crate::launcher::{LauncherAction, LauncherEvent};
    use crate::launcher_home::{CardLevelSnapshot, LauncherHomeCounts, LauncherHomeSnapshot};
    use crate::test_support::{arcade_catalog, arcade_game, arcade_system};
    use crate::ui_display::{ScreenOrientation, UiDisplay};
    use mister_magik_framebuffer_scenes::launcher::LauncherScene;

    const COMPOSED: Rgb565Pixel = Rgb565Pixel(0x1111);

    fn root_level() -> CardLevelSnapshot {
        CardLevelSnapshot::root(&LauncherHomeSnapshot::from_counts(LauncherHomeCounts {
            arcade: 1,
            consoles: 2,
            computers: 3,
            handhelds: 4,
            favourites: 5,
            collections: 4,
        }))
    }

    /// A card-home session that owns the visible Home frame.
    fn active_cards(scene: LauncherScene) -> LauncherCardHomeSession {
        let level = root_level();
        let mut cards = LauncherCardHomeSession::new(scene, level.clone(), 0, "12:00").unwrap();
        assert!(cards.update_from_navigation(scene, &level, 0, 0.0, "12:00", 0, false, None, None));
        assert!(cards.owns_visible_frame());
        cards
    }

    fn catalog() -> crate::arcade_catalog::ArcadeCatalog {
        arcade_catalog(
            vec![arcade_game("Metal Slug").build()],
            vec![arcade_system("arcade", 1)],
        )
    }

    fn home_nav() -> LauncherNav {
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog());
        assert_eq!(nav.screen, Screen::Home);
        nav
    }

    struct Fixture {
        layout: UiLayoutGeometry,
        metrics: CrtUiMetrics,
        composed: Vec<Rgb565Pixel>,
    }

    impl Fixture {
        fn new(orientation: ScreenOrientation) -> Self {
            let display = UiDisplay::for_framebuffer(960, 540);
            let layout = UiLayoutGeometry::for_display(&display, orientation);
            Self {
                layout,
                metrics: CrtUiMetrics::for_framebuffer(960, 540),
                composed: vec![COMPOSED; layout.composition_w() * layout.composition_h()],
            }
        }

        fn inputs<'a>(
            &'a self,
            nav: &'a LauncherNav,
            edge: NavigationTransitionEdge,
            direction: NavigationTransitionDirection,
            collection_id: Option<&'a str>,
        ) -> TransitionInputs<'a> {
            TransitionInputs {
                edge,
                direction,
                nav,
                collection_id,
                layout: self.layout,
                crt_layout: false,
                crt_metrics: &self.metrics,
                composed: &self.composed,
                crt_backdrop: &self.composed,
                now_us: 0,
            }
        }
    }

    #[test]
    fn home_to_arcade_begins_from_the_card_home_frame_not_the_stale_cache() {
        let fixture = Fixture::new(ScreenOrientation::Normal);
        let nav = home_nav();
        let mut cards = active_cards(LauncherScene::new(960, 540));
        let visible = card_pixels_as_slint(cards.render()).to_vec();
        assert_ne!(visible, fixture.composed, "the cache must be stale");

        let mut runtime = NavigationTransitionRuntime::new(960, 540, true);
        let inputs = fixture.inputs(
            &nav,
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Forward,
            Some(MENU_ARCADE_SYSTEM_ID),
        );
        assert!(begin_navigation_transition(
            &mut runtime,
            Some(&mut cards),
            &inputs,
            || None
        ));
        assert!(runtime.render().unwrap() == visible);
    }

    #[test]
    fn a_reveal_without_card_home_begins_from_the_composed_cache() {
        let fixture = Fixture::new(ScreenOrientation::Normal);
        let nav = home_nav();
        let mut runtime = NavigationTransitionRuntime::new(960, 540, true);
        let inputs = fixture.inputs(
            &nav,
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionDirection::Forward,
            None,
        );
        assert!(begin_navigation_transition(
            &mut runtime,
            None,
            &inputs,
            || { None }
        ));
        assert!(runtime.render().unwrap() == fixture.composed);
    }

    #[test]
    fn portrait_never_begins_from_the_logical_card_home_frame() {
        for edge in [
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionEdge::HomeToArcade,
        ] {
            let fixture = Fixture::new(ScreenOrientation::MonitorClockwise);
            assert!(fixture.layout.is_portrait());
            let nav = home_nav();
            let mut cards = active_cards(LauncherScene::new(
                fixture.layout.logical_w(),
                fixture.layout.logical_h(),
            ));
            let mut runtime = NavigationTransitionRuntime::new(
                fixture.layout.logical_w(),
                fixture.layout.logical_h(),
                true,
            );
            let inputs = fixture.inputs(
                &nav,
                edge,
                NavigationTransitionDirection::Forward,
                Some(MENU_ARCADE_SYSTEM_ID),
            );
            assert!(
                begin_navigation_transition(&mut runtime, Some(&mut cards), &inputs, || None),
                "{edge:?}"
            );
            assert!(runtime.render().unwrap() == fixture.composed, "{edge:?}");
        }
    }

    #[test]
    fn a_crt_reverse_reveal_refreshes_the_device_reveal_image_once_it_starts() {
        let fixture = Fixture::new(ScreenOrientation::Normal);
        let catalog = catalog();
        let mut nav = home_nav();
        let open = LauncherEvent {
            action: LauncherAction::OpenCollection,
            path: Some(MENU_ARCADE_SYSTEM_ID.to_owned()),
            settings: None,
        };
        assert!(nav.commit_navigation_intent(&open, &catalog));
        for (crt_layout, direction, expected) in [
            (true, NavigationTransitionDirection::Reverse, 1),
            (true, NavigationTransitionDirection::Forward, 0),
            (false, NavigationTransitionDirection::Reverse, 0),
        ] {
            let mut runtime = NavigationTransitionRuntime::new(960, 540, true);
            let mut inputs = fixture.inputs(
                &nav,
                NavigationTransitionEdge::HomeToArcade,
                direction,
                None,
            );
            inputs.crt_layout = crt_layout;
            let calls = std::cell::Cell::new(0);
            assert!(begin_navigation_transition(
                &mut runtime,
                None,
                &inputs,
                || {
                    calls.set(calls.get() + 1);
                    None
                }
            ));
            assert_eq!(calls.get(), expected, "crt={crt_layout} {direction:?}");
        }
    }

    #[test]
    fn a_reverse_reveal_from_arcade_begins_from_the_composed_cache_even_with_card_home() {
        let fixture = Fixture::new(ScreenOrientation::Normal);
        let catalog = catalog();
        let mut nav = home_nav();
        let open = LauncherEvent {
            action: LauncherAction::OpenCollection,
            path: Some(MENU_ARCADE_SYSTEM_ID.to_owned()),
            settings: None,
        };
        assert!(nav.commit_navigation_intent(&open, &catalog));
        assert_eq!(nav.screen, Screen::Arcade);

        let mut cards = active_cards(LauncherScene::new(960, 540));
        let mut runtime = NavigationTransitionRuntime::new(960, 540, true);
        let inputs = fixture.inputs(
            &nav,
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Reverse,
            None,
        );
        assert!(begin_navigation_transition(
            &mut runtime,
            Some(&mut cards),
            &inputs,
            || None
        ));
        assert!(runtime.render().unwrap() == fixture.composed);
    }

    #[test]
    fn the_system_panel_slide_starts_in_landscape_and_never_in_portrait() {
        for (orientation, starts) in [
            (ScreenOrientation::Normal, true),
            (ScreenOrientation::MonitorClockwise, false),
        ] {
            let fixture = Fixture::new(orientation);
            let nav = home_nav();
            let mut runtime = NavigationTransitionRuntime::new(
                fixture.layout.logical_w(),
                fixture.layout.logical_h(),
                true,
            );
            let inputs = fixture.inputs(
                &nav,
                NavigationTransitionEdge::SystemPanel,
                NavigationTransitionDirection::Forward,
                None,
            );
            assert_eq!(
                begin_navigation_transition(&mut runtime, None, &inputs, || None),
                starts,
                "{orientation:?}"
            );
        }
    }

    #[test]
    fn portrait_navigation_geometry_uses_physical_rectangles() {
        let display = UiDisplay::for_framebuffer(4, 3);
        let layout = UiLayoutGeometry::for_display(&display, ScreenOrientation::MonitorClockwise);
        let logical_rect = NavigationTransitionRect {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };
        let geometry = NavigationTransitionGeometry {
            source_card: logical_rect,
            destination_preview: logical_rect,
            ..NavigationTransitionGeometry::default()
        };

        let mapped = navigation_geometry_to_composition(layout, geometry);

        assert_eq!(mapped.source_card, mapped.destination_preview);
        assert_eq!(mapped.source_card.x, 0);
        assert_eq!(mapped.source_card.y, 1);
        assert_eq!(mapped.source_card.width, 1);
        assert_eq!(mapped.source_card.height, 2);
    }
}
