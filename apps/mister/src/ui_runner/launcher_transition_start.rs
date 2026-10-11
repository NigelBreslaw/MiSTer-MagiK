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
use mister_magik_framebuffer_scenes::settings_cog;
use mister_magik_framebuffer_scenes::{OutputRotation, Rgb565OutputLayout};

/// Everything a reveal needs to begin, besides the runtime and the card session.
pub struct TransitionInputs<'a> {
    pub edge: NavigationTransitionEdge,
    pub direction: NavigationTransitionDirection,
    pub nav: &'a LauncherNav,
    /// The collection an `OpenCollection` intent names, which picks the device art.
    pub collection_id: Option<&'a str>,
    pub layout: UiLayoutGeometry,
    pub crt_layout: bool,
    pub crt_metrics: &'a CrtUiMetrics,
    pub crt_backdrop: &'a [Rgb565Pixel],
    pub now_us: u64,
}

/// What `start_navigation_transition` needs beyond the shared inputs: the pixels
/// the reveal begins from and where the selected Home card sits.
pub struct TransitionSource<'a> {
    /// The pixels the user is looking at.
    pub pixels: &'a [Rgb565Pixel],
    /// The selected card's rectangle when a card-home frame is shown; it selects
    /// the device-card reveal.
    pub card_home_rect: Option<NavigationTransitionRect>,
    /// Whether `pixels` are in the physical composition raster. Portrait
    /// transitions run in that raster only when this holds.
    pub physical: bool,
}

/// Begin the full-screen transition for `inputs.edge`. Returns whether it started.
///
/// HDMI landscape card motion can bypass the cache, so capture its native
/// frame. CRT and portrait sources use the completed physical frame, including
/// all chrome, rather than asking the card producer for another scratch frame.
pub(super) fn begin_navigation_transition(
    runtime: &mut NavigationTransitionRuntime,
    cards: Option<&mut LauncherCardHomeSession>,
    composed: &[Rgb565Pixel],
    inputs: &TransitionInputs<'_>,
    reveal_image: impl FnOnce() -> Option<RevealImage>,
) -> bool {
    let portrait = inputs.layout.is_portrait();
    let card_edge = is_card_edge(inputs.edge) && !portrait;
    let card_home_rect = cards
        .as_deref()
        .filter(|_| card_edge)
        .map(LauncherCardHomeSession::selected_card_rect);
    let pixels: &[Rgb565Pixel] = match cards {
        Some(cards)
            if !portrait
                && !inputs.crt_layout
                && card_home_owns_source(inputs.nav.screen, cards.owns_visible_frame()) =>
        {
            card_pixels_as_slint(cards.render())
        }
        _ => composed,
    };
    start_navigation_transition(
        runtime,
        inputs,
        &TransitionSource {
            pixels,
            card_home_rect,
            physical: true,
        },
        reveal_image,
    )
}

/// The start rules shared by the device loop and the macOS preview, which differ
/// only in where the card-home rectangle and the source pixels come from.
pub fn start_navigation_transition(
    runtime: &mut NavigationTransitionRuntime,
    inputs: &TransitionInputs<'_>,
    source: &TransitionSource<'_>,
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
        crt_backdrop,
        now_us,
    } = *inputs;
    let TransitionSource {
        pixels,
        card_home_rect,
        physical,
    } = *source;
    let portrait = layout.is_portrait();
    if portrait {
        let route = NavigationTransitionRoute::from_super_scaler_edge(edge);
        let start = if physical {
            TransitionStart::portrait_page(route, direction, layout.output_layout(), pixels, now_us)
        } else {
            TransitionStart::portrait_page_logical(route, direction, pixels, now_us)
        };
        return runtime.begin(start).unwrap_or(false);
    }
    if edge == NavigationTransitionEdge::SystemPanel {
        return runtime
            .begin(TransitionStart::system_panel(
                crt_layout,
                nav.is_system_hub(),
                pixels,
                crt_backdrop,
                now_us,
            ))
            .unwrap_or(false);
    }
    let card_home_rect = card_home_rect.filter(|_| is_card_edge(edge) && !portrait);
    let geometry = navigation_geometry(
        nav,
        &NavigationDisplay {
            frame_width: layout.logical_w(),
            frame_height: layout.logical_h(),
            crt: crt_layout.then(|| crt_navigation_layout(layout.content_rect(), crt_metrics)),
            card_home_rect,
        },
        edge,
    );
    let started = if card_home_rect.is_some() {
        let kind = match collection_id {
            Some(id) => nav.device_kind_for_collection(id),
            None => nav.device_kind(),
        };
        let hub = direction == NavigationTransitionDirection::Forward || nav.is_system_hub();
        runtime.begin(TransitionStart::device_card(
            edge,
            direction,
            geometry,
            crate::launcher_presentation::device_reveal_spec(kind, crt_layout, hub),
            pixels,
            crate::launcher_presentation::system_device_rgb565(kind),
            crt_backdrop,
            now_us,
        ))
    } else {
        runtime.begin(TransitionStart::super_scaler(
            edge, direction, geometry, pixels, now_us,
        ))
    };
    if started.as_ref().is_ok_and(|started| *started)
        && crt_layout
        && direction == NavigationTransitionDirection::Reverse
    {
        runtime.update_device_reveal_image(reveal_image());
    }
    started.unwrap_or(false)
}

/// Everything a Settings-family transition needs to begin.
pub(super) struct SettingsInputs<'a> {
    pub route: NavigationTransitionRoute,
    pub direction: NavigationTransitionDirection,
    /// The output layout shared by the compositor and transition snapshots.
    pub output: Rgb565OutputLayout,
    pub reduce_motion: bool,
    pub crt_layout: bool,
    /// The composed cache: the visible frame whenever card-home is not.
    pub composed: &'a [Rgb565Pixel],
    pub now_us: u64,
}

/// Portrait slides complete rotated snapshots. Landscape uses the Home <->
/// Settings cog when available, or the page slide for other Settings routes.
pub(super) fn begin_settings_transition(
    runtime: &mut NavigationTransitionRuntime,
    cards: Option<&mut LauncherCardHomeSession>,
    inputs: &SettingsInputs<'_>,
) -> bool {
    let SettingsInputs {
        route,
        direction,
        output,
        reduce_motion,
        crt_layout,
        composed,
        now_us,
    } = *inputs;
    if output.rotation() != OutputRotation::None {
        return runtime
            .begin(TransitionStart::portrait_page(
                route, direction, output, composed, now_us,
            ))
            .unwrap_or(false);
    }
    let (render_w, render_h) = (output.physical_width(), output.physical_height());
    let settled_cards = cards.filter(|cards| !cards.is_animating());
    let zoom = route == NavigationTransitionRoute::HomeToSettings
        && settled_cards.is_some()
        && settings_cog::supports_dimensions(render_w, render_h)
        && !reduce_motion;
    let start = if zoom {
        let source = match (direction, settled_cards) {
            // HDMI card motion bypasses the cache. CRT already keeps the
            // complete displayed frame, including chrome, in the cache.
            (NavigationTransitionDirection::Forward, Some(cards)) if !crt_layout => {
                card_pixels_as_slint(cards.render())
            }
            // Reverse starts from the live Settings page. Forward CRT starts
            // from the complete Home frame already maintained in this cache.
            _ => composed,
        };
        TransitionStart::settings_cog(
            direction,
            render_w,
            render_h,
            source,
            crate::launcher_presentation::settings_cog_artwork(),
            now_us,
        )
    } else {
        TransitionStart::settings_page_physical(
            route,
            direction,
            SettingsPageTransitionAxis::Horizontal,
            render_w,
            render_h,
            composed,
            now_us,
        )
    };
    runtime.begin(start).unwrap_or(false)
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

    fn native_crt_display(height: u16) -> UiDisplay {
        UiDisplay::for_plan(
            crate::ui_display::UiDisplayPlan::from_geometry_with_route_and_composition(
                mister_magik_core::display::DisplayGeometry::new(640, height),
                match height {
                    240 => crate::ui_display::ResolvedOutputRoute::Crt240p60,
                    288 => crate::ui_display::ResolvedOutputRoute::Crt288p50,
                    _ => panic!("unsupported CRT test raster"),
                },
                "native-crt-transition",
                crate::ui_display::UiFramebufferSizePolicy::Auto,
                mister_magik_core::display::Crt240Composition::Native240,
            ),
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
            &fixture.composed,
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
            &fixture.composed,
            &inputs,
            || { None }
        ));
        assert!(runtime.render().unwrap() == fixture.composed);
    }

    #[test]
    fn the_shared_start_begins_from_the_pixels_it_is_given_without_a_card_session() {
        let fixture = Fixture::new(ScreenOrientation::Normal);
        let nav = home_nav();
        let shown = vec![Rgb565Pixel(0x2222); fixture.composed.len()];
        let rect = LauncherScene::new(960, 540).slot_zero(false).rect();
        let mut runtime = NavigationTransitionRuntime::new(960, 540, true);
        let inputs = fixture.inputs(
            &nav,
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionDirection::Forward,
            None,
        );
        assert!(start_navigation_transition(
            &mut runtime,
            &inputs,
            &TransitionSource {
                pixels: &shown,
                card_home_rect: Some(rect),
                physical: false,
            },
            || None
        ));
        assert!(runtime.render().unwrap() == shown);
    }

    #[test]
    fn a_portrait_start_runs_in_the_raster_its_source_is_in() {
        let fixture = Fixture::new(ScreenOrientation::MonitorClockwise);
        assert!(fixture.layout.is_portrait());
        let nav = home_nav();
        let logical = vec![COMPOSED; fixture.layout.logical_w() * fixture.layout.logical_h()];
        let inputs = fixture.inputs(
            &nav,
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Forward,
            Some(MENU_ARCADE_SYSTEM_ID),
        );
        let physical_space = |pixels: &[Rgb565Pixel], physical| {
            let mut runtime = NavigationTransitionRuntime::new(960, 540, true);
            assert!(start_navigation_transition(
                &mut runtime,
                &inputs,
                &TransitionSource {
                    pixels,
                    card_home_rect: None,
                    physical,
                },
                || None,
            ));
            runtime.settings_physical_space()
        };
        assert!(
            !physical_space(&logical, false),
            "a logical source plays logically"
        );
        assert!(physical_space(&fixture.composed, true));
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
                begin_navigation_transition(
                    &mut runtime,
                    Some(&mut cards),
                    &fixture.composed,
                    &inputs,
                    || None
                ),
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
                &fixture.composed,
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
            &fixture.composed,
            &inputs,
            || None
        ));
        assert!(runtime.render().unwrap() == fixture.composed);
    }

    #[test]
    fn the_system_panel_slide_starts_in_landscape_and_portrait() {
        for (orientation, starts) in [
            (ScreenOrientation::Normal, true),
            (ScreenOrientation::MonitorClockwise, true),
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
                begin_navigation_transition(&mut runtime, None, &fixture.composed, &inputs, || {
                    None
                }),
                starts,
                "{orientation:?}"
            );
        }
    }

    fn settings_inputs<'a>(
        fixture: &'a Fixture,
        route: NavigationTransitionRoute,
        direction: NavigationTransitionDirection,
        reduce_motion: bool,
    ) -> SettingsInputs<'a> {
        SettingsInputs {
            route,
            direction,
            output: fixture.layout.output_layout(),
            reduce_motion,
            crt_layout: false,
            composed: &fixture.composed,
            now_us: 0,
        }
    }

    #[test]
    fn the_settings_zoom_begins_from_the_settled_card_home_frame() {
        let fixture = Fixture::new(ScreenOrientation::Normal);
        let mut cards = active_cards(LauncherScene::new(960, 540));
        let visible = card_pixels_as_slint(cards.render()).to_vec();
        assert_ne!(visible, fixture.composed, "the cache must be stale");

        let mut runtime = NavigationTransitionRuntime::new(960, 540, true);
        let inputs = settings_inputs(
            &fixture,
            NavigationTransitionRoute::HomeToSettings,
            NavigationTransitionDirection::Forward,
            false,
        );
        assert!(begin_settings_transition(
            &mut runtime,
            Some(&mut cards),
            &inputs
        ));
        assert_eq!(runtime.request().unwrap().renderer_label(), "settings-cog");
        assert!(runtime.render().unwrap() == visible);
    }

    #[test]
    fn crt_transitions_capture_composed_chrome_instead_of_the_bare_cards() {
        let display = native_crt_display(240);
        let layout = UiLayoutGeometry::for_display(&display, ScreenOrientation::Normal);
        let metrics = CrtUiMetrics::for_display(&display);
        let mut cards = active_cards(LauncherScene::crt(640, 240));
        let mut composed = card_pixels_as_slint(cards.render()).to_vec();
        // Completed chrome must survive source-carrier frames during capture,
        // even when it differs from the card producer's current scratch frame.
        composed[20 * 640 + 40] = Rgb565Pixel(0xef5a);
        composed[220 * 640 + 40] = Rgb565Pixel(0xef5a);
        let nav = home_nav();
        for edge in [
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionEdge::HomeToConsoles,
        ] {
            let mut runtime = NavigationTransitionRuntime::new(640, 240, true);
            let inputs = TransitionInputs {
                edge,
                direction: NavigationTransitionDirection::Forward,
                nav: &nav,
                collection_id: None,
                layout,
                crt_layout: true,
                crt_metrics: &metrics,
                crt_backdrop: &[],
                now_us: 0,
            };
            assert!(begin_navigation_transition(
                &mut runtime,
                Some(&mut cards),
                &composed,
                &inputs,
                || None
            ));
            assert!(
                runtime.render().unwrap() == composed,
                "{edge:?} lost the composed CRT chrome"
            );
        }
        let mut runtime = NavigationTransitionRuntime::new(640, 240, true);
        assert!(begin_settings_transition(
            &mut runtime,
            Some(&mut cards),
            &SettingsInputs {
                route: NavigationTransitionRoute::HomeToSettings,
                direction: NavigationTransitionDirection::Forward,
                output: layout.output_layout(),
                reduce_motion: false,
                crt_layout: true,
                composed: &composed,
                now_us: 0,
            }
        ));
        assert!(
            runtime.render().unwrap() == composed,
            "Settings lost the composed CRT chrome"
        );
    }

    #[test]
    fn the_settings_zoom_reverses_from_the_live_settings_page() {
        let fixture = Fixture::new(ScreenOrientation::Normal);
        let mut cards = active_cards(LauncherScene::new(960, 540));
        let mut runtime = NavigationTransitionRuntime::new(960, 540, true);
        let inputs = settings_inputs(
            &fixture,
            NavigationTransitionRoute::HomeToSettings,
            NavigationTransitionDirection::Reverse,
            false,
        );
        assert!(begin_settings_transition(
            &mut runtime,
            Some(&mut cards),
            &inputs
        ));
        assert_eq!(runtime.request().unwrap().renderer_label(), "settings-cog");
        assert!(runtime.render().unwrap() == fixture.composed);
    }

    #[test]
    fn the_settings_slide_replaces_the_zoom_when_it_cannot_run() {
        let fixture = Fixture::new(ScreenOrientation::Normal);
        let route = NavigationTransitionRoute::HomeToSettings;
        let direction = NavigationTransitionDirection::Forward;
        let mut cards = active_cards(LauncherScene::new(960, 540));

        // Reduce motion, no card session, an unsupported raster and another
        // route all keep the page slide, from the composed cache.
        let slide = |label: &str, inputs: SettingsInputs<'_>, cards: Option<&mut _>| {
            let mut runtime = NavigationTransitionRuntime::new(960, 540, true);
            assert!(
                begin_settings_transition(&mut runtime, cards, &inputs),
                "{label}"
            );
            assert_eq!(
                runtime.request().unwrap().renderer_label(),
                "settings-page",
                "{label}"
            );
        };
        slide(
            "reduce motion",
            settings_inputs(&fixture, route, direction, true),
            Some(&mut cards),
        );
        slide(
            "no cards",
            settings_inputs(&fixture, route, direction, false),
            None,
        );
        slide(
            "about page",
            settings_inputs(
                &fixture,
                NavigationTransitionRoute::SettingsToAbout,
                direction,
                false,
            ),
            Some(&mut cards),
        );
        let unsupported_pixels = vec![COMPOSED; 800 * 600];
        let unsupported = SettingsInputs {
            output: Rgb565OutputLayout::new(800, 600, 800, OutputRotation::None).unwrap(),
            composed: &unsupported_pixels,
            ..settings_inputs(&fixture, route, direction, false)
        };
        let mut runtime = NavigationTransitionRuntime::new(800, 600, true);
        assert!(begin_settings_transition(
            &mut runtime,
            Some(&mut cards),
            &unsupported
        ));
        assert_eq!(
            runtime.request().unwrap().renderer_label(),
            "settings-page",
            "unsupported raster must still start its page slide"
        );
    }

    #[test]
    fn the_settings_zoom_waits_for_a_settled_card_and_supports_native_crt_rasters() {
        let fixture = Fixture::new(ScreenOrientation::Normal);
        let scene = LauncherScene::new(960, 540);
        let route = NavigationTransitionRoute::HomeToSettings;
        let direction = NavigationTransitionDirection::Forward;
        let renderer = |cards: &mut LauncherCardHomeSession, inputs: &SettingsInputs<'_>| {
            let mut runtime = NavigationTransitionRuntime::new(960, 540, true);
            assert!(begin_settings_transition(&mut runtime, Some(cards), inputs));
            runtime.request().unwrap().renderer_label()
        };

        // A carousel still moving keeps the slide.
        let mut moving = active_cards(scene);
        let level = root_level();
        assert!(
            moving.update_from_navigation(scene, &level, 1, 0.5, "12:00", 16, true, None, None)
        );
        assert!(moving.is_animating());
        assert_eq!(
            renderer(
                &mut moving,
                &settings_inputs(&fixture, route, direction, false)
            ),
            "settings-page"
        );

        // A native landscape CRT raster supports the zoom; rotated outputs
        // use the page slide covered by the portrait motion matrix below.
        let layout =
            UiLayoutGeometry::for_display(&native_crt_display(240), ScreenOrientation::Normal);
        let mut settled = active_cards(LauncherScene::crt(640, 240));
        let composed = vec![COMPOSED; layout.output_layout().len()];
        let native = SettingsInputs {
            output: layout.output_layout(),
            crt_layout: true,
            composed: &composed,
            ..settings_inputs(&fixture, route, direction, false)
        };
        let mut runtime = NavigationTransitionRuntime::new(640, 240, true);
        assert!(begin_settings_transition(
            &mut runtime,
            Some(&mut settled),
            &native
        ));
        assert_eq!(runtime.request().unwrap().renderer_label(), "settings-cog");
    }

    #[test]
    fn portrait_navigation_frames_equal_rotated_complete_page_motion() {
        use mister_magik_framebuffer_scenes::Rgb565Pixel as SharedPixel;
        use mister_magik_framebuffer_scenes::navigation::{
            NavigationTransitionBuffers, render_settings_page_transition_into,
        };
        for display in [
            UiDisplay::for_framebuffer(960, 540),
            native_crt_display(240),
            native_crt_display(288),
        ] {
            for orientation in [
                ScreenOrientation::MonitorClockwise,
                ScreenOrientation::MonitorCounterclockwise,
            ] {
                let layout = UiLayoutGeometry::for_display(&display, orientation);
                let output = layout.output_layout();
                let (w, h) = (layout.logical_w(), layout.logical_h());
                let source = (0..w * h)
                    .map(|i| SharedPixel((i as u16).wrapping_mul(13)))
                    .collect::<Vec<_>>();
                let destination = (0..w * h)
                    .map(|i| SharedPixel((i as u16).wrapping_mul(29)))
                    .collect::<Vec<_>>();
                let rotate = |logical: &[SharedPixel]| {
                    let mut physical = vec![Rgb565Pixel(0); output.len()];
                    for y in 0..h {
                        for x in 0..w {
                            physical[output.physical_offset(x, y)] =
                                Rgb565Pixel(logical[y * w + x].0);
                        }
                    }
                    physical
                };
                let physical_source = rotate(&source);
                let physical_destination = rotate(&destination);
                let nav = home_nav();
                let metrics = CrtUiMetrics::for_display(&display);
                let crt = display.output_route().is_crt();
                let mut cards = active_cards(if crt {
                    LauncherScene::crt(w, h)
                } else {
                    LauncherScene::new(w, h)
                });
                for direction in [
                    NavigationTransitionDirection::Forward,
                    NavigationTransitionDirection::Reverse,
                ] {
                    for route in [
                        NavigationTransitionRoute::HomeToArcade,
                        NavigationTransitionRoute::HomeToConsoles,
                        NavigationTransitionRoute::ConsolesToSystem,
                        NavigationTransitionRoute::SystemPanel,
                        NavigationTransitionRoute::HomeToSettings,
                        NavigationTransitionRoute::SettingsToAbout,
                        NavigationTransitionRoute::AboutToLicenses,
                        NavigationTransitionRoute::LicensesToLicenseText,
                        NavigationTransitionRoute::NestedToHome,
                    ] {
                        let mut runtime = NavigationTransitionRuntime::new(w, h, true);
                        let edge = match route {
                            NavigationTransitionRoute::HomeToArcade => {
                                Some(NavigationTransitionEdge::HomeToArcade)
                            }
                            NavigationTransitionRoute::HomeToConsoles => {
                                Some(NavigationTransitionEdge::HomeToConsoles)
                            }
                            NavigationTransitionRoute::ConsolesToSystem => {
                                Some(NavigationTransitionEdge::ConsolesToSystem)
                            }
                            NavigationTransitionRoute::SystemPanel => {
                                Some(NavigationTransitionEdge::SystemPanel)
                            }
                            _ => None,
                        };
                        let started = if let Some(edge) = edge {
                            begin_navigation_transition(
                                &mut runtime,
                                Some(&mut cards),
                                &physical_source,
                                &TransitionInputs {
                                    edge,
                                    direction,
                                    nav: &nav,
                                    collection_id: None,
                                    layout,
                                    crt_layout: crt,
                                    crt_metrics: &metrics,
                                    crt_backdrop: &[],
                                    now_us: 0,
                                },
                                || None,
                            )
                        } else {
                            begin_settings_transition(
                                &mut runtime,
                                Some(&mut cards),
                                &SettingsInputs {
                                    route,
                                    direction,
                                    output: layout.output_layout(),
                                    reduce_motion: false,
                                    crt_layout: crt,
                                    composed: &physical_source,
                                    now_us: 0,
                                },
                            )
                        };
                        let logical_source =
                            source.iter().map(|p| Rgb565Pixel(p.0)).collect::<Vec<_>>();
                        let logical_destination = destination
                            .iter()
                            .map(|p| Rgb565Pixel(p.0))
                            .collect::<Vec<_>>();
                        let mut preview = NavigationTransitionRuntime::new(w, h, true);
                        let preview_started = if let Some(edge) = edge {
                            start_navigation_transition(
                                &mut preview,
                                &TransitionInputs {
                                    edge,
                                    direction,
                                    nav: &nav,
                                    collection_id: None,
                                    layout,
                                    crt_layout: crt,
                                    crt_metrics: &metrics,
                                    crt_backdrop: &[],
                                    now_us: 0,
                                },
                                &TransitionSource {
                                    pixels: &logical_source,
                                    card_home_rect: None,
                                    physical: false,
                                },
                                || None,
                            )
                        } else {
                            preview
                                .begin(TransitionStart::portrait_page_logical(
                                    route,
                                    direction,
                                    &logical_source,
                                    0,
                                ))
                                .unwrap()
                        };
                        assert!(preview_started, "preview {route:?} did not start");
                        assert_eq!(
                            preview.request().unwrap().renderer_label(),
                            runtime.request().unwrap().renderer_label()
                        );
                        preview
                            .capture_destination(&logical_destination, 1)
                            .unwrap();
                        assert!(started, "{route:?} {orientation:?} did not start");
                        assert!(
                            runtime.render().unwrap() == physical_source,
                            "{route:?} {orientation:?} used an unrotated source"
                        );
                        runtime
                            .capture_destination(&physical_destination, 1)
                            .unwrap();
                        let duration = runtime.request().unwrap().duration_us;
                        let mut reference = NavigationTransitionBuffers::new(w, h);
                        reference.capture_source(&source).unwrap();
                        reference.capture_destination(&destination).unwrap();
                        for part in 0..=4 {
                            runtime.tick(1 + duration * part / 4);
                            preview.tick(1 + duration * part / 4);
                            let logical_preview = preview
                                .render()
                                .unwrap()
                                .iter()
                                .map(|p| SharedPixel(p.0))
                                .collect::<Vec<_>>();
                            assert_eq!(
                                rotate(&logical_preview),
                                runtime.render().unwrap(),
                                "preview differs from device: {route:?} {direction:?} {orientation:?} phase {part}"
                            );
                            let frame = runtime.frame();
                            let mut logical_frame = vec![SharedPixel(0); w * h];
                            render_settings_page_transition_into(
                                &reference,
                                NavigationTransitionRequest::settings_page(direction),
                                frame,
                                &mut logical_frame,
                            )
                            .unwrap();
                            let expected = rotate(&logical_frame);
                            let actual = runtime.render().unwrap();
                            assert!(
                                actual == expected,
                                "{route:?} {direction:?} {orientation:?} {w}x{h} phase {part}: mixed portrait/landscape pixels; first mismatch {:?}",
                                actual.iter().zip(&expected).position(|(a, b)| a != b)
                            );
                        }
                    }
                }
            }
        }
    }
}
