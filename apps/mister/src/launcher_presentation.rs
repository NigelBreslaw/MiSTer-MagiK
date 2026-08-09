// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Portable projection of launcher navigation state into the compiled Slint UI.
//!
//! Device lifecycle, controller discovery, preview loading, and scanout remain
//! in `ui_runner`; this presenter is deliberately shared with the macOS host.

use crate::arcade_catalog::{ArcadeCatalog, ArcadeGameView};
use crate::launcher::{
    ArcadeSearchPane, ArcadeSearchStatus, CatalogMenuItemStatus, LauncherNav, Screen,
};
use crate::launcher_taxonomy::LauncherMenuItemKind;
use mister_magik_ui::launcher::{Launcher, MenuItem, MenuItemKind, MenuItemStatus, MisterBridge};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;

macro_rules! set_if_changed {
    ($bridge:expr, $getter:ident, $setter:ident, $value:expr) => {{
        let value = $value;
        if $bridge.$getter() != value {
            $bridge.$setter(value);
        }
    }};
}

macro_rules! set_string_if_changed {
    ($bridge:expr, $getter:ident, $setter:ident, $value:expr) => {{
        let value: SharedString = ($value).into();
        if $bridge.$getter() != value {
            $bridge.$setter(value);
        }
    }};
}

#[derive(Default)]
pub struct LauncherBridgePresenter {
    menu_items_key: Option<(usize, String)>,
    menu_items: Option<Rc<VecModel<MenuItem>>>,
    license_lines_index: Option<usize>,
    license_lines: Option<Rc<VecModel<SharedString>>>,
}

impl LauncherBridgePresenter {
    pub fn sync(
        &mut self,
        app: &Launcher,
        nav: &LauncherNav,
        catalog: &ArcadeCatalog,
        catalog_version: Option<usize>,
        defer_arcade_overlay: bool,
    ) {
        let bridge = app.global::<MisterBridge>();
        set_if_changed!(
            bridge,
            get_screen_mode,
            set_screen_mode,
            screen_mode(nav.screen)
        );
        set_if_changed!(
            bridge,
            get_selected_index,
            set_selected_index,
            nav.selected as i32
        );
        set_if_changed!(
            bridge,
            get_home_scroll_held,
            set_home_scroll_held,
            nav.home_horizontal_held()
        );
        set_if_changed!(
            bridge,
            get_home_scroll_repeat_active,
            set_home_scroll_repeat_active,
            nav.home_horizontal_repeat_active()
        );
        set_if_changed!(bridge, get_home_scroll_x, set_home_scroll_x, nav.scroll_x);
        set_string_if_changed!(
            bridge,
            get_menu_title,
            set_menu_title,
            nav.current_menu_title()
        );
        set_string_if_changed!(
            bridge,
            get_menu_breadcrumb,
            set_menu_breadcrumb,
            nav.current_menu_breadcrumb()
        );
        set_if_changed!(
            bridge,
            get_settings_focused,
            set_settings_focused,
            nav.settings_focused
        );
        set_if_changed!(
            bridge,
            get_settings_selected,
            set_settings_selected,
            nav.settings_selected as i32
        );
        set_if_changed!(
            bridge,
            get_about_selected,
            set_about_selected,
            nav.about_selected as i32
        );
        set_if_changed!(
            bridge,
            get_display_combo_open,
            set_display_combo_open,
            nav.display_combo_open
        );
        set_if_changed!(
            bridge,
            get_display_selected,
            set_display_selected,
            nav.display_selected as i32
        );
        set_if_changed!(
            bridge,
            get_display_highlighted,
            set_display_highlighted,
            nav.display_highlighted as i32
        );
        set_if_changed!(
            bridge,
            get_display_confirm_remaining,
            set_display_confirm_remaining,
            nav.display_confirm_remaining as i32
        );
        set_if_changed!(
            bridge,
            get_simple_joystick_handling,
            set_simple_joystick_handling,
            nav.settings.simple_joystick_handling
        );
        set_if_changed!(
            bridge,
            get_reduce_motion,
            set_reduce_motion,
            nav.settings.reduce_motion
        );
        set_if_changed!(
            bridge,
            get_screensaver_settings_selected,
            set_screensaver_settings_selected,
            nav.screensaver_selected as i32
        );
        set_if_changed!(
            bridge,
            get_screensaver_enabled,
            set_screensaver_enabled,
            nav.settings.screensaver_enabled
        );
        set_if_changed!(
            bridge,
            get_screensaver_delay_minutes,
            set_screensaver_delay_minutes,
            nav.settings.screensaver_delay_minutes as i32
        );
        set_if_changed!(
            bridge,
            get_licenses_selected,
            set_licenses_selected,
            nav.licenses_selected as i32
        );
        set_if_changed!(
            bridge,
            get_licenses_expanded,
            set_licenses_expanded,
            nav.licenses_expanded
        );
        set_if_changed!(
            bridge,
            get_licenses_scroll_y,
            set_licenses_scroll_y,
            nav.licenses_scroll_y()
        );
        if self.license_lines_index != Some(nav.licenses_selected) {
            bridge.set_license_lines(self.license_lines(nav.licenses_selected));
        }

        if let Some(catalog_version) = catalog_version {
            let key = (catalog_version, nav.current_menu_id().to_string());
            if self.menu_items_key.as_ref() != Some(&key) {
                bridge.set_menu_items(self.menu_items(nav, catalog_version));
            }
        }

        let games = active_game_view(catalog, nav);
        let (title, count) = active_header(catalog, nav, games.len());
        set_string_if_changed!(
            bridge,
            get_active_system_title,
            set_active_system_title,
            title
        );
        set_if_changed!(
            bridge,
            get_active_system_count,
            set_active_system_count,
            count as i32
        );
        set_if_changed!(
            bridge,
            get_arcade_games_loading,
            set_arcade_games_loading,
            active_games_loading(catalog, nav)
        );
        if !(defer_arcade_overlay && nav.screen == Screen::Arcade) {
            set_if_changed!(
                bridge,
                get_arcade_selected,
                set_arcade_selected,
                nav.arcade.selected as i32
            );
            set_if_changed!(
                bridge,
                get_arcade_scroll_y,
                set_arcade_scroll_y,
                nav.arcade.scroll_y
            );
        }
        sync_search(&bridge, nav);
    }

    pub fn menu_items(&mut self, nav: &LauncherNav, catalog_version: usize) -> ModelRc<MenuItem> {
        let key = (catalog_version, nav.current_menu_id().to_string());
        if self.menu_items_key.as_ref() != Some(&key) {
            self.menu_items = Some(build_menu_items(nav));
            self.menu_items_key = Some(key);
        }
        ModelRc::from(
            self.menu_items
                .as_ref()
                .expect("launcher menu model initialized")
                .clone(),
        )
    }

    pub fn license_lines(&mut self, index: usize) -> ModelRc<SharedString> {
        if self.license_lines_index != Some(index) {
            let lines = crate::licenses::wrapped_lines(index)
                .iter()
                .map(|line| SharedString::from(line.as_str()))
                .collect::<Vec<_>>();
            self.license_lines = Some(Rc::new(VecModel::from(lines)));
            self.license_lines_index = Some(index);
        }
        ModelRc::from(
            self.license_lines
                .as_ref()
                .expect("license line model initialized")
                .clone(),
        )
    }
}

pub fn screen_mode(screen: Screen) -> i32 {
    match screen {
        Screen::Home => 0,
        Screen::SystemHub => 8,
        Screen::Controller => 1,
        Screen::Arcade => 2,
        Screen::Settings => 3,
        Screen::About => 4,
        Screen::Licenses => 5,
        Screen::Info => 6,
        Screen::Screensaver => 7,
    }
}

fn build_menu_items(nav: &LauncherNav) -> Rc<VecModel<MenuItem>> {
    let rows = nav
        .current_menu_items()
        .iter()
        .map(|item| {
            let presentation = nav.menu_item_catalog_presentation(item);
            MenuItem {
                id: item.id.clone().into(),
                label: item.title.clone().into(),
                subtitle: match presentation.status {
                    CatalogMenuItemStatus::Scanning => match item.kind {
                        LauncherMenuItemKind::Menu => {
                            let systems = nav.menu_discovered_system_count(&item.id);
                            format!(
                                "{systems} system{} found",
                                if systems == 1 { "" } else { "s" }
                            )
                            .into()
                        }
                        LauncherMenuItemKind::Collection => {
                            if presentation.available {
                                format!("{} games available", item.count).into()
                            } else {
                                "".into()
                            }
                        }
                    },
                    CatalogMenuItemStatus::Partial => "Some items failed".into(),
                    CatalogMenuItemStatus::UpdateFailed if presentation.available => {
                        format!("Update failed • {} games", item.count).into()
                    }
                    CatalogMenuItemStatus::UpdateFailed => "Update failed".into(),
                    CatalogMenuItemStatus::LoadFailed => "Load failed — A to retry".into(),
                    CatalogMenuItemStatus::Ready => format!("{} games", item.count).into(),
                },
                available: presentation.available,
                node_kind: match item.kind {
                    LauncherMenuItemKind::Menu => MenuItemKind::Group,
                    LauncherMenuItemKind::Collection => MenuItemKind::Collection,
                },
                status: match presentation.status {
                    CatalogMenuItemStatus::Ready => MenuItemStatus::Ready,
                    CatalogMenuItemStatus::Scanning => MenuItemStatus::Scanning,
                    CatalogMenuItemStatus::Partial => MenuItemStatus::Partial,
                    CatalogMenuItemStatus::UpdateFailed if presentation.available => {
                        MenuItemStatus::UpdateFailed
                    }
                    CatalogMenuItemStatus::UpdateFailed | CatalogMenuItemStatus::LoadFailed => {
                        MenuItemStatus::Failed
                    }
                },
            }
        })
        .collect::<Vec<_>>();
    Rc::new(VecModel::from(rows))
}

fn active_game_view<'a>(catalog: &'a ArcadeCatalog, nav: &'a LauncherNav) -> ArcadeGameView<'a> {
    nav.active_collection()
        .map(|collection| nav.active_arcade_game_view(catalog, &collection.id))
        .unwrap_or_else(ArcadeGameView::empty)
}

fn active_header(
    catalog: &ArcadeCatalog,
    nav: &LauncherNav,
    fallback_count: usize,
) -> (String, usize) {
    let Some(collection) = nav.active_collection() else {
        return ("Games".to_string(), fallback_count);
    };
    let filter_label = nav.arcade_filter.active_label();
    let title = if filter_label == "Games A-Z" {
        collection.title.clone()
    } else {
        format!("{} - {filter_label}", collection.title)
    };
    let hydrated_count = nav.active_arcade_game_count(catalog, &collection.id);
    let count = if hydrated_count == 0 && collection.count > 0 {
        collection.count
    } else {
        hydrated_count
    };
    (title, count)
}

fn active_games_loading(catalog: &ArcadeCatalog, nav: &LauncherNav) -> bool {
    nav.active_collection().is_some_and(|collection| {
        collection.count > 0 && catalog.system_game_count(&collection.id) < collection.count
    })
}

fn sync_search(bridge: &MisterBridge, nav: &LauncherNav) {
    set_if_changed!(
        bridge,
        get_arcade_search_active,
        set_arcade_search_active,
        nav.arcade_search.is_active(&nav.arcade_filter.active)
    );
    set_string_if_changed!(
        bridge,
        get_arcade_search_query,
        set_arcade_search_query,
        nav.arcade_search.query.clone()
    );
    set_string_if_changed!(
        bridge,
        get_arcade_search_suggestion,
        set_arcade_search_suggestion,
        nav.arcade_search.suggestion.clone()
    );
    set_if_changed!(
        bridge,
        get_arcade_search_status,
        set_arcade_search_status,
        match nav.arcade_search.status {
            ArcadeSearchStatus::Idle => 0,
            ArcadeSearchStatus::Searching => 1,
            ArcadeSearchStatus::Ready => 2,
            ArcadeSearchStatus::Failed => 3,
        }
    );
    set_if_changed!(
        bridge,
        get_arcade_search_key_selected,
        set_arcade_search_key_selected,
        nav.arcade_search.selected_key as i32
    );
    set_if_changed!(
        bridge,
        get_arcade_search_pane,
        set_arcade_search_pane,
        match nav.arcade_search.pane {
            ArcadeSearchPane::Keyboard => 0,
            ArcadeSearchPane::Results => 1,
        }
    );
}
