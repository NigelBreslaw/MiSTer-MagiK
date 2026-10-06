// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral planning rules shared by every full-screen navigation
//! transition: where its pixels come from and where its geometry comes from.
//!
//! Both are derived from the committed navigation state at the moment the
//! transition begins. Neither may depend on what an earlier transition left in
//! process memory, so a transition behaves identically after a game return,
//! an orientation change or a cancelled predecessor.

use crate::launcher::{LauncherAction, LauncherEvent, LauncherNav, Screen};
use crate::launcher_taxonomy::ROOT_MENU_ID;
use crate::ui_display::{CrtContentRect, CrtUiMetrics};
use mister_magik_framebuffer_scenes::navigation::{
    CrtNavigationLayout, NavigationTransitionDirection, NavigationTransitionEdge,
    NavigationTransitionGeometry, NavigationTransitionRect, crt_navigation_geometry,
    hdmi_navigation_geometry,
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

/// The edge and direction a navigation intent plays, if it plays one.
///
/// `card_levels` is true when card-home owns Home level changes; it plays its own
/// level trick for those, so no full-screen transition runs.
pub fn navigation_transition_for_intent(
    nav: &LauncherNav,
    event: &LauncherEvent,
    card_levels: bool,
) -> Option<(NavigationTransitionEdge, NavigationTransitionDirection)> {
    use NavigationTransitionDirection::{Forward, Reverse};
    use NavigationTransitionEdge::{ConsolesToSystem, HomeToArcade, HomeToConsoles, SystemPanel};

    let home_level_change = nav.screen == Screen::Home
        && matches!(
            event.action,
            LauncherAction::OpenMenu | LauncherAction::NavigateBack | LauncherAction::NavigateHome
        );
    if card_levels && home_level_change {
        return None;
    }
    let root = nav.current_menu_id() == ROOT_MENU_ID;
    match (event.action, nav.screen) {
        (LauncherAction::ToggleSystemPage | LauncherAction::OpenSystemSection, _) => {
            Some((SystemPanel, Forward))
        }
        (LauncherAction::OpenMenu, _) => Some((HomeToConsoles, Forward)),
        (LauncherAction::OpenCollection, _) if root => Some((HomeToArcade, Forward)),
        (LauncherAction::OpenCollection, _) => Some((ConsolesToSystem, Forward)),
        (LauncherAction::NavigateBack | LauncherAction::NavigateHome, Screen::Home) => {
            Some((HomeToConsoles, Reverse))
        }
        (LauncherAction::NavigateBack | LauncherAction::NavigateHome, Screen::Arcade) => {
            Some((if root { HomeToArcade } else { ConsolesToSystem }, Reverse))
        }
        _ => None,
    }
}

/// The CRT geometry inputs: the route's content rectangle and UI metrics.
pub fn crt_navigation_layout(
    content: CrtContentRect,
    metrics: &CrtUiMetrics,
) -> CrtNavigationLayout {
    CrtNavigationLayout {
        content_x: content.x,
        content_y: content.y,
        content_width: content.width,
        content_height: content.height,
        grid_x: metrics.grid_x.max(1) as usize,
        grid_y: metrics.grid_y.max(1) as usize,
        header_height: metrics.header_height.max(1) as usize,
        footer_height: metrics.footer_height.max(1) as usize,
        heading_font_height: metrics.heading_font.pixels().max(1) as usize,
        title_font_height: metrics.card_title_font.pixels().max(1) as usize,
        detail_font_height: metrics.card_detail_font.pixels().max(1) as usize,
        game_row_height: metrics.game_row_height.max(1) as usize,
    }
}

/// The display facts navigation geometry needs, besides the navigation state.
#[derive(Clone, Copy, Debug)]
pub struct NavigationDisplay {
    pub frame_width: usize,
    pub frame_height: usize,
    pub crt: Option<CrtNavigationLayout>,
    pub card_home_rect: Option<NavigationTransitionRect>,
}

/// Geometry for `edge` in either direction, from the navigation state alone.
pub fn navigation_geometry(
    nav: &LauncherNav,
    display: &NavigationDisplay,
    edge: NavigationTransitionEdge,
) -> NavigationTransitionGeometry {
    let (selected, scroll_x) = nav.menu_tile_view();
    let items = nav.current_menu_items();
    derive_navigation_geometry(
        &NavigationGeometryContext {
            frame_width: display.frame_width,
            frame_height: display.frame_height,
            crt: display.crt,
            selected,
            scroll_x,
            item_count: items.len(),
            root_menu: nav.current_menu_id() == ROOT_MENU_ID,
            selected_label: items.get(selected).map_or("", |item| item.title.as_str()),
            card_home_rect: display.card_home_rect,
        },
        edge,
    )
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
