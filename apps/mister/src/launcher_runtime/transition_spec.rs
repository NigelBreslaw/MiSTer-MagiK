// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! The one description of how a full-screen navigation transition starts.
//!
//! Every transition used to have its own `begin_*` entry point that chose the
//! raster space, when the clock starts, which assets it carries and which route
//! it reports. A [`TransitionStart`] names those choices as data, and
//! [`NavigationTransitionRuntime::begin`](super::navigation_transition::NavigationTransitionRuntime::begin)
//! applies them the same way for all kinds. A new transition adds a constructor
//! here, not another way to start.
//!
//! # Adding a navigation transition
//!
//! The chart (`FullScreenTransitionStateChart`) answers who may render; the timeline
//! (`NavigationTransitionController`) answers how far the motion has got. They are independent
//! axes (roadmap PR 10): add to the timeline side, never fold one into the other.
//!
//! 1. Name it: a `NavigationTransitionRoute` variant and `label()`. A card or list edge also needs
//!    a `NavigationTransitionEdge` and a row in `navigation_transition_for_intent`
//!    (`transition_plan.rs`). A Settings-family page needs a `settings_page_depth` and a
//!    `settings_page_transition` row (`navigation_transition.rs`).
//! 2. Geometry: a pure, tested function. It depends on the committed navigation state only.
//! 3. Start: one constructor here, choosing the raster space, [`StartPolicy`] and assets.
//! 4. Render: draw only, from the two snapshots and [`TransitionAssets`]; no allocation per frame.
//! 5. Prove it: add the edge to `EDGES` in `transition_scenarios.rs`; its source, endpoint, reverse
//!    and hygiene checks and the director walks then apply without new test code.
//!
//! An effect that is not navigation (orientation is the model) takes a chart owner and the
//! director's begin, capture and end operations instead; it never starts the chart by hand.

use super::navigation_transition::{
    NavigationTransitionDirection, NavigationTransitionEdge, NavigationTransitionGeometry,
    NavigationTransitionRequest, NavigationTransitionRoute, SettingsPageTransitionAxis,
};
use mister_magik_framebuffer_scenes::Rgb565Pixel as SharedRgb565Pixel;
use mister_magik_framebuffer_scenes::device_card::DeviceCardReveal;
use mister_magik_framebuffer_scenes::settings_cog::{CogArtwork, supports_dimensions};
use slint::platform::software_renderer::Rgb565Pixel;

const HDMI_ABOUT_CONTENT_X: u16 = 266;
const HDMI_SETTINGS_CONTENT_X: u16 = 400;
/// Settings page and cog durations on native CRT and portrait rasters.
const NATIVE_SETTINGS_PAGE_US: u64 = 520_000;
const NATIVE_SETTINGS_COG_US: u64 = 800_000;

/// The raster a transition is composed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RasterSpace {
    /// The logical launcher raster.
    Logical,
    /// The physical output raster (portrait, or native CRT Settings motion).
    Physical { width: usize, height: usize },
}

/// When the animation clock starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartPolicy {
    /// Now: the destination is prepared while the source already moves.
    Immediate,
    /// Only once the composed destination and its artwork are ready.
    AfterDestination,
}

/// Artwork a renderer needs that is not in the two snapshots.
pub enum TransitionAssets<'a> {
    None,
    /// Device artwork over a captured backdrop.
    DeviceCard {
        asset: &'static [SharedRgb565Pixel],
        backdrop: &'a [Rgb565Pixel],
    },
    /// The system hub/list slide's backdrop.
    SystemPanel {
        backdrop: &'a [Rgb565Pixel],
    },
    /// The Settings cog artwork.
    SettingsCog(&'static CogArtwork),
}

pub struct TransitionStart<'a> {
    pub request: NavigationTransitionRequest,
    pub route: NavigationTransitionRoute,
    pub space: RasterSpace,
    pub start: StartPolicy,
    pub assets: TransitionAssets<'a>,
    /// The pixels the user is looking at.
    pub source: &'a [Rgb565Pixel],
    pub now_us: u64,
}

impl<'a> TransitionStart<'a> {
    /// Super-scaler card/list scaling in the logical raster.
    pub fn super_scaler(
        edge: NavigationTransitionEdge,
        direction: NavigationTransitionDirection,
        geometry: NavigationTransitionGeometry,
        source: &'a [Rgb565Pixel],
        now_us: u64,
    ) -> Self {
        Self {
            request: NavigationTransitionRequest::new(edge, direction, geometry),
            route: NavigationTransitionRoute::from_super_scaler_edge(edge),
            space: RasterSpace::Logical,
            start: StartPolicy::Immediate,
            assets: TransitionAssets::None,
            source,
            now_us,
        }
    }

    /// Super-scaler in the physical composition raster (portrait).
    pub fn super_scaler_physical(
        edge: NavigationTransitionEdge,
        direction: NavigationTransitionDirection,
        geometry: NavigationTransitionGeometry,
        width: usize,
        height: usize,
        source: &'a [Rgb565Pixel],
        now_us: u64,
    ) -> Self {
        Self {
            space: RasterSpace::Physical { width, height },
            ..Self::super_scaler(edge, direction, geometry, source, now_us)
        }
    }

    /// Device-card reveal. The clock waits for the composed destination, as with
    /// the Settings card capture contract.
    #[allow(clippy::too_many_arguments)]
    pub fn device_card(
        edge: NavigationTransitionEdge,
        direction: NavigationTransitionDirection,
        geometry: NavigationTransitionGeometry,
        reveal: DeviceCardReveal,
        source: &'a [Rgb565Pixel],
        device: &'static [SharedRgb565Pixel],
        backdrop: &'a [Rgb565Pixel],
        now_us: u64,
    ) -> Self {
        Self {
            request: NavigationTransitionRequest::device_card(direction, edge, geometry, reveal),
            route: NavigationTransitionRoute::from_super_scaler_edge(edge),
            space: RasterSpace::Logical,
            start: StartPolicy::AfterDestination,
            assets: TransitionAssets::DeviceCard {
                asset: device,
                backdrop,
            },
            source,
            now_us,
        }
    }

    /// The system hub/list slide.
    pub fn system_panel(
        crt: bool,
        to_list: bool,
        source: &'a [Rgb565Pixel],
        backdrop: &'a [Rgb565Pixel],
        now_us: u64,
    ) -> Self {
        Self {
            request: NavigationTransitionRequest::system_panel(crt, to_list),
            route: NavigationTransitionRoute::SystemPanel,
            space: RasterSpace::Logical,
            start: StartPolicy::Immediate,
            assets: TransitionAssets::SystemPanel { backdrop },
            source,
            now_us,
        }
    }

    /// A Settings-family page slide in the logical raster. `route` must be a
    /// Settings page route, which `settings_page_transition` guarantees.
    pub fn settings_page(
        route: NavigationTransitionRoute,
        direction: NavigationTransitionDirection,
        source: &'a [Rgb565Pixel],
        now_us: u64,
    ) -> Self {
        debug_assert!(route.is_settings_page());
        let request = if route.uses_segmented_settings_motion() {
            NavigationTransitionRequest::settings_page_segmented_with_content_x(
                direction,
                segmented_destination_content_x(route, direction),
            )
        } else {
            NavigationTransitionRequest::settings_page(direction)
        };
        Self {
            request,
            route,
            space: RasterSpace::Logical,
            start: StartPolicy::AfterDestination,
            assets: TransitionAssets::None,
            source,
            now_us,
        }
    }

    /// A Settings-family page slide in the physical raster, along `axis`.
    #[allow(clippy::too_many_arguments)]
    pub fn settings_page_physical(
        route: NavigationTransitionRoute,
        direction: NavigationTransitionDirection,
        axis: SettingsPageTransitionAxis,
        width: usize,
        height: usize,
        source: &'a [Rgb565Pixel],
        now_us: u64,
    ) -> Self {
        debug_assert!(route.is_settings_page());
        let mut request = if route.uses_segmented_settings_motion() {
            NavigationTransitionRequest::settings_page_segmented_on_axis_with_content_x(
                direction,
                axis,
                segmented_destination_content_x(route, direction),
            )
        } else {
            NavigationTransitionRequest::settings_page_on_axis(direction, axis)
        };
        if is_native_raster(width, height) {
            request.duration_us = NATIVE_SETTINGS_PAGE_US;
        }
        Self {
            request,
            route,
            space: RasterSpace::Physical { width, height },
            start: StartPolicy::AfterDestination,
            assets: TransitionAssets::None,
            source,
            now_us,
        }
    }

    /// Home <-> Settings card zoom in physical HDMI or native CRT space.
    pub fn settings_cog(
        direction: NavigationTransitionDirection,
        width: usize,
        height: usize,
        source: &'a [Rgb565Pixel],
        cog: &'static CogArtwork,
        now_us: u64,
    ) -> Self {
        let mut request = NavigationTransitionRequest::settings_cog(direction);
        if is_native_raster(width, height) {
            request.duration_us = NATIVE_SETTINGS_COG_US;
        }
        Self {
            request,
            route: NavigationTransitionRoute::HomeToSettings,
            space: RasterSpace::Physical { width, height },
            start: StartPolicy::AfterDestination,
            assets: TransitionAssets::SettingsCog(cog),
            source,
            now_us,
        }
    }
}

/// Native CRT and portrait rasters play the Settings motion slower than 960x540.
fn is_native_raster(width: usize, height: usize) -> bool {
    supports_dimensions(width, height) && (width, height) != (960, 540)
}

const fn segmented_destination_content_x(
    route: NavigationTransitionRoute,
    direction: NavigationTransitionDirection,
) -> u16 {
    match (route, direction) {
        (NavigationTransitionRoute::SettingsToAbout, NavigationTransitionDirection::Reverse) => {
            HDMI_SETTINGS_CONTENT_X
        }
        _ => HDMI_ABOUT_CONTENT_X,
    }
}

#[cfg(test)]
mod tests {
    use super::super::navigation_transition::{
        NavigationTransitionRuntime, settings_page_transition,
    };
    use super::*;
    use crate::launcher::Screen;

    const SCREENS: [Screen; 7] = [
        Screen::Home,
        Screen::Controller,
        Screen::Arcade,
        Screen::Settings,
        Screen::About,
        Screen::Licenses,
        Screen::LicenseText,
    ];

    fn frame() -> Vec<Rgb565Pixel> {
        vec![Rgb565Pixel(0x1234); 16 * 12]
    }

    #[test]
    fn reverse_about_to_settings_marks_only_the_settings_list_as_moving_content() {
        use NavigationTransitionDirection::{Forward, Reverse};
        use NavigationTransitionRoute::{AboutToLicenses, SettingsToAbout};
        assert_eq!(
            segmented_destination_content_x(SettingsToAbout, Reverse),
            HDMI_SETTINGS_CONTENT_X
        );
        assert_eq!(
            segmented_destination_content_x(SettingsToAbout, Forward),
            HDMI_ABOUT_CONTENT_X
        );
        assert_eq!(
            segmented_destination_content_x(AboutToLicenses, Reverse),
            HDMI_ABOUT_CONTENT_X
        );
    }

    #[test]
    fn every_planned_settings_transition_is_a_settings_route() {
        // `settings_page` and `settings_page_physical` rely on this.
        let mut planned = 0;
        for source in SCREENS {
            for destination in SCREENS {
                if let Some((route, _)) = settings_page_transition(source, destination) {
                    assert!(route.is_settings_page(), "{source:?} -> {destination:?}");
                    planned += 1;
                }
            }
        }
        assert!(planned > 8, "{planned}");
    }

    #[test]
    fn each_kind_names_its_space_clock_route_and_assets() {
        let source = frame();
        let geometry = NavigationTransitionGeometry::default();
        let edge = NavigationTransitionEdge::HomeToArcade;
        let direction = NavigationTransitionDirection::Forward;

        let scaler = TransitionStart::super_scaler(edge, direction, geometry, &source, 7);
        assert_eq!(scaler.space, RasterSpace::Logical);
        assert_eq!(scaler.start, StartPolicy::Immediate);
        assert_eq!(scaler.route, NavigationTransitionRoute::HomeToArcade);
        assert!(matches!(scaler.assets, TransitionAssets::None));
        assert_eq!(scaler.now_us, 7);

        let portrait =
            TransitionStart::super_scaler_physical(edge, direction, geometry, 12, 16, &source, 0);
        assert_eq!(
            portrait.space,
            RasterSpace::Physical {
                width: 12,
                height: 16
            }
        );
        assert_eq!(portrait.start, StartPolicy::Immediate);

        let card = TransitionStart::device_card(
            edge,
            direction,
            geometry,
            DeviceCardReveal::cabinet(false),
            &source,
            &[],
            &source,
            0,
        );
        assert_eq!(card.start, StartPolicy::AfterDestination);
        assert!(card.request.is_device_card());
        assert!(matches!(card.assets, TransitionAssets::DeviceCard { .. }));

        let panel = TransitionStart::system_panel(false, true, &source, &source, 0);
        assert_eq!(panel.route, NavigationTransitionRoute::SystemPanel);
        assert_eq!(panel.start, StartPolicy::Immediate);
        assert!(matches!(panel.assets, TransitionAssets::SystemPanel { .. }));

        let page = TransitionStart::settings_page(
            NavigationTransitionRoute::SettingsToAbout,
            direction,
            &source,
            0,
        );
        assert_eq!(page.space, RasterSpace::Logical);
        assert_eq!(page.start, StartPolicy::AfterDestination);
        assert_eq!(page.route, NavigationTransitionRoute::SettingsToAbout);
    }

    #[test]
    fn native_rasters_play_settings_motion_slower_than_960_by_540() {
        let source = frame();
        let direction = NavigationTransitionDirection::Forward;
        let cog = crate::launcher_presentation::settings_cog_artwork();
        let hdmi = TransitionStart::settings_cog(direction, 960, 540, &source, cog, 0);
        let crt = TransitionStart::settings_cog(direction, 640, 240, &source, cog, 0);
        assert_ne!(hdmi.request.duration_us, NATIVE_SETTINGS_COG_US);
        assert_eq!(crt.request.duration_us, NATIVE_SETTINGS_COG_US);
        assert_eq!(
            crt.space,
            RasterSpace::Physical {
                width: 640,
                height: 240
            }
        );

        let route = NavigationTransitionRoute::HomeToSettings;
        let axis = SettingsPageTransitionAxis::Horizontal;
        let hdmi =
            TransitionStart::settings_page_physical(route, direction, axis, 960, 540, &source, 0);
        let crt =
            TransitionStart::settings_page_physical(route, direction, axis, 640, 240, &source, 0);
        assert_ne!(hdmi.request.duration_us, NATIVE_SETTINGS_PAGE_US);
        assert_eq!(crt.request.duration_us, NATIVE_SETTINGS_PAGE_US);
    }

    #[test]
    fn begin_starts_nothing_while_disabled_or_already_playing() {
        let source = frame();
        let geometry = NavigationTransitionGeometry::default();
        let edge = NavigationTransitionEdge::HomeToConsoles;
        let direction = NavigationTransitionDirection::Forward;

        let mut disabled = NavigationTransitionRuntime::new(16, 12, false);
        assert!(
            !disabled
                .begin(TransitionStart::super_scaler(
                    edge, direction, geometry, &source, 0
                ))
                .unwrap()
        );

        let mut runtime = NavigationTransitionRuntime::new(16, 12, true);
        assert!(
            runtime
                .begin(TransitionStart::super_scaler(
                    edge, direction, geometry, &source, 0
                ))
                .unwrap()
        );
        assert_eq!(
            runtime.route(),
            Some(NavigationTransitionRoute::HomeToConsoles)
        );
        // A second start while one plays changes neither the route nor the raster.
        assert!(
            !runtime
                .begin(TransitionStart::settings_cog(
                    direction,
                    12,
                    16,
                    &source,
                    crate::launcher_presentation::settings_cog_artwork(),
                    1,
                ))
                .unwrap()
        );
        assert_eq!(
            runtime.route(),
            Some(NavigationTransitionRoute::HomeToConsoles)
        );
        assert!(!runtime.settings_physical_space());
    }

    #[test]
    fn the_duration_override_applies_to_every_kind() {
        let source = frame();
        let mut runtime = NavigationTransitionRuntime::new(16, 12, true);
        runtime.configure_preview(Some(2_000));
        assert!(
            runtime
                .begin(TransitionStart::system_panel(
                    false, true, &source, &source, 0
                ))
                .unwrap()
        );
        assert_eq!(runtime.request().unwrap().duration_us, 2_000_000);
    }
}
