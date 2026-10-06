// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral planning rules shared by every full-screen navigation
//! transition: where its pixels come from and where its geometry comes from.
//!
//! Both are derived from the committed navigation state at the moment the
//! transition begins. Neither may depend on what an earlier transition left in
//! process memory, so a transition behaves identically after a game return,
//! an orientation change or a cancelled predecessor.

use crate::launcher::Screen;
use mister_magik_framebuffer_scenes::navigation::{
    CrtNavigationLayout, NavigationTransitionEdge, NavigationTransitionGeometry,
    NavigationTransitionRect, crt_navigation_geometry, hdmi_navigation_geometry,
};

/// Whether card-home, not the composed RGB565 cache, drew the visible frame.
///
/// A transition must start from exactly the pixels on screen. Card-home
/// presents directly into scanout slots and leaves the cache at an older pose.
pub const fn card_home_owns_source(screen: Screen, card_home_active: bool) -> bool {
    matches!(screen, Screen::Home) && card_home_active
}

/// Committed navigation facts that position a transition's card and list.
#[derive(Clone, Copy, Debug)]
pub struct NavigationGeometryContext<'a> {
    pub frame_width: usize,
    pub frame_height: usize,
    /// `Some` selects the native CRT geometry.
    pub crt: Option<CrtNavigationLayout>,
    pub selected: usize,
    pub scroll_x: i32,
    pub item_count: usize,
    pub root_menu: bool,
    pub selected_label: &'a str,
    /// Settled rectangle of the selected launcher card when card-home owns the
    /// carousel and landscape geometry applies.
    pub card_home_rect: Option<NavigationTransitionRect>,
}

/// Geometry for either direction of `edge`.
///
/// Reverse transitions replay the forward edge, and the committed navigation
/// state still identifies the same tile (the menu level that was left keeps its
/// selection and scroll), so the forward derivation is also the reverse one.
pub fn derive_navigation_geometry(
    context: &NavigationGeometryContext<'_>,
    edge: NavigationTransitionEdge,
) -> NavigationTransitionGeometry {
    let mut geometry = match context.crt {
        Some(layout) => crt_navigation_geometry(
            context.frame_width,
            context.frame_height,
            layout,
            context.selected,
            context.item_count,
            context.root_menu,
            edge,
            context.selected_label,
        ),
        None => hdmi_navigation_geometry(
            context.frame_width,
            context.frame_height,
            context.selected,
            context.scroll_x,
            context.root_menu,
            edge,
            context.selected_label,
        ),
    };
    if matches!(
        edge,
        NavigationTransitionEdge::HomeToArcade | NavigationTransitionEdge::ConsolesToSystem
    ) && let Some(rect) = context.card_home_rect
    {
        geometry.source_card = rect;
    }
    geometry
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdmi<'a>(root_menu: bool, selected: usize, label: &'a str) -> NavigationGeometryContext<'a> {
        NavigationGeometryContext {
            frame_width: 960,
            frame_height: 540,
            crt: None,
            selected,
            scroll_x: 0,
            item_count: 6,
            root_menu,
            selected_label: label,
            card_home_rect: None,
        }
    }

    fn crt<'a>(selected: usize, label: &'a str) -> NavigationGeometryContext<'a> {
        NavigationGeometryContext {
            frame_width: 640,
            frame_height: 480,
            crt: Some(CrtNavigationLayout {
                content_x: 16,
                content_y: 12,
                content_width: 608,
                content_height: 456,
                grid_x: 8,
                grid_y: 8,
                header_height: 32,
                footer_height: 24,
                heading_font_height: 16,
                title_font_height: 12,
                detail_font_height: 8,
                game_row_height: 24,
            }),
            selected,
            scroll_x: 0,
            item_count: 4,
            root_menu: true,
            selected_label: label,
            card_home_rect: None,
        }
    }

    #[test]
    fn home_owns_the_source_only_while_card_home_is_active() {
        assert!(card_home_owns_source(Screen::Home, true));
        assert!(!card_home_owns_source(Screen::Home, false));
        for screen in [
            Screen::Arcade,
            Screen::Settings,
            Screen::About,
            Screen::Licenses,
            Screen::LicenseText,
            Screen::Controller,
        ] {
            assert!(!card_home_owns_source(screen, true));
        }
    }

    #[test]
    fn geometry_is_pure_in_the_committed_navigation_state() {
        // Reverse has no history to consult: the same state must always give
        // the same geometry, for every edge and both display families.
        for edge in [
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionEdge::ConsolesToSystem,
        ] {
            for context in [
                hdmi(true, 0, "Arcade"),
                hdmi(false, 3, "Sega"),
                crt(1, "Arcade"),
            ] {
                assert_eq!(
                    derive_navigation_geometry(&context, edge),
                    derive_navigation_geometry(&context, edge),
                    "{edge:?}"
                );
            }
        }
    }

    #[test]
    fn selected_tile_moves_the_source_card() {
        let edge = NavigationTransitionEdge::ConsolesToSystem;
        let first = derive_navigation_geometry(&hdmi(false, 0, "Atari"), edge);
        let third = derive_navigation_geometry(&hdmi(false, 2, "Atari"), edge);
        assert_ne!(first.source_card, third.source_card);
    }

    #[test]
    fn card_home_rect_replaces_the_source_card_on_card_edges_only() {
        let rect = NavigationTransitionRect {
            x: 321,
            y: 54,
            width: 200,
            height: 300,
        };
        let mut context = hdmi(true, 0, "Arcade");
        context.card_home_rect = Some(rect);
        for edge in [
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionEdge::ConsolesToSystem,
        ] {
            assert_eq!(derive_navigation_geometry(&context, edge).source_card, rect);
        }
        assert_ne!(
            derive_navigation_geometry(&context, NavigationTransitionEdge::HomeToConsoles)
                .source_card,
            rect
        );
    }
}
