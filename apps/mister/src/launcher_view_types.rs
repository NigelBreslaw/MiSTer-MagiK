// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Exhaustive conversion from Rust launcher domains into the shared Slint
//! semantic vocabulary. Numeric values remain presentation data or dynamic
//! model indices; they are never finite-state discriminants in this module.

use crate::launcher::{
    ArcadeSearchPane as DomainArcadeSearchPane, ArcadeSearchStatus as DomainArcadeSearchStatus,
    ArcadeUserListMode, ConfirmAction, DisplayTransactionPhase, Screen,
};
use crate::setup_nav::SetupPhase as DomainSetupPhase;
use crate::ui_display::ScreenOrientation as DomainScreenOrientation;
use mister_magik_ui::launcher as view;

fn display_choice(
    mode: Option<&mister_magik_mister_runtime::display_resolution::DisplayResolution>,
) -> view::ChoiceOption {
    mode.map_or_else(view::ChoiceOption::default, |mode| view::ChoiceOption {
        id: mode.id.into(),
        label: mode.label.into(),
    })
}

pub fn active_display_choice(runtime_index: usize) -> view::ChoiceOption {
    display_choice(
        mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS.get(runtime_index),
    )
}

pub fn selected_display_choice(runtime_index: usize) -> view::ChoiceOption {
    crate::launcher::settings_display_selection_index(runtime_index)
        .map_or_else(view::ChoiceOption::default, settings_display_choice)
}

pub fn settings_display_choice(settings_index: usize) -> view::ChoiceOption {
    display_choice(crate::launcher::settings_display_resolution(settings_index))
}

pub const fn launcher_screen(value: Screen) -> view::LauncherScreen {
    match value {
        Screen::Home => view::LauncherScreen::Home,
        Screen::SystemHub => view::LauncherScreen::SystemHub,
        Screen::Controller => view::LauncherScreen::Controller,
        Screen::Arcade => view::LauncherScreen::Arcade,
        Screen::Settings => view::LauncherScreen::Settings,
        Screen::Screensaver => view::LauncherScreen::ScreensaverSettings,
        Screen::About => view::LauncherScreen::About,
        Screen::Licenses => view::LauncherScreen::Licenses,
        Screen::Info => view::LauncherScreen::Info,
    }
}

pub const fn system_hub_section(index: usize) -> view::SystemHubSection {
    match index {
        0 => view::SystemHubSection::Games,
        1 => view::SystemHubSection::Recent,
        2 => view::SystemHubSection::Favourites,
        3 => view::SystemHubSection::Information,
        _ => panic!("system hub selection is outside its finite domain"),
    }
}

pub const fn home_focus(settings_focused: bool) -> view::HomeFocus {
    if settings_focused {
        view::HomeFocus::Settings
    } else {
        view::HomeFocus::Menu
    }
}

pub const fn menu_hierarchy(root: bool) -> view::MenuHierarchy {
    if root {
        view::MenuHierarchy::Root
    } else {
        view::MenuHierarchy::Nested
    }
}

pub const fn home_scroll_phase(held: bool, repeating: bool) -> view::HomeScrollPhase {
    if repeating {
        view::HomeScrollPhase::Repeating
    } else if held {
        view::HomeScrollPhase::Held
    } else {
        view::HomeScrollPhase::Idle
    }
}

pub const fn navigation_transition_state(active: bool) -> view::NavigationTransitionState {
    if active {
        view::NavigationTransitionState::Active
    } else {
        view::NavigationTransitionState::Idle
    }
}

pub const fn settings_section(index: usize) -> view::SettingsSection {
    match index {
        0 => view::SettingsSection::Display,
        1 => view::SettingsSection::Orientation,
        2 => view::SettingsSection::Screensaver,
        3 => view::SettingsSection::ReduceMotion,
        4 => view::SettingsSection::Exit,
        5 => view::SettingsSection::Rebuild,
        6 => view::SettingsSection::About,
        _ => panic!("settings selection is outside its finite domain"),
    }
}

pub const fn settings_popup(display_open: bool, orientation_open: bool) -> view::SettingsPopup {
    match (display_open, orientation_open) {
        (false, false) => view::SettingsPopup::None,
        (true, false) => view::SettingsPopup::DisplayResolution,
        (false, true) => view::SettingsPopup::ScreenOrientation,
        (true, true) => panic!("settings popups are mutually exclusive"),
    }
}

pub const fn screensaver_setting(index: usize) -> view::ScreensaverSetting {
    match index {
        0 => view::ScreensaverSetting::Enabled,
        1 => view::ScreensaverSetting::Delay,
        2 => view::ScreensaverSetting::Preview,
        _ => panic!("screensaver selection is outside its finite domain"),
    }
}

pub const fn about_section(index: usize) -> view::AboutSection {
    match index {
        0 => view::AboutSection::Information,
        1 => view::AboutSection::Licenses,
        _ => panic!("about selection is outside its finite domain"),
    }
}

pub fn orientation_at(index: usize) -> view::ScreenOrientation {
    let orientation = DomainScreenOrientation::ALL
        .get(index)
        .copied()
        .expect("orientation selection is outside its finite domain");
    screen_orientation(orientation)
}

pub const fn screen_orientation(value: DomainScreenOrientation) -> view::ScreenOrientation {
    match value {
        DomainScreenOrientation::Normal => view::ScreenOrientation::Normal,
        DomainScreenOrientation::MonitorClockwise => view::ScreenOrientation::MonitorClockwise,
        DomainScreenOrientation::MonitorCounterclockwise => {
            view::ScreenOrientation::MonitorCounterclockwise
        }
    }
}

pub const fn setup_phase(value: DomainSetupPhase) -> view::SetupPhase {
    match value {
        DomainSetupPhase::None => view::SetupPhase::None,
        DomainSetupPhase::Detected => view::SetupPhase::Detected,
        DomainSetupPhase::NewOrExisting => view::SetupPhase::NewOrExisting,
        DomainSetupPhase::PickExisting => view::SetupPhase::PickExisting,
        DomainSetupPhase::Configure => view::SetupPhase::Configure,
        DomainSetupPhase::NameKind => view::SetupPhase::NameKind,
    }
}

pub const fn arcade_list_mode(value: ArcadeUserListMode) -> view::ArcadeListMode {
    match value {
        ArcadeUserListMode::Games => view::ArcadeListMode::Games,
        ArcadeUserListMode::Recent => view::ArcadeListMode::Recent,
        ArcadeUserListMode::Favourites => view::ArcadeListMode::Favourites,
    }
}

pub const fn arcade_search_status(value: DomainArcadeSearchStatus) -> view::ArcadeSearchStatus {
    match value {
        DomainArcadeSearchStatus::Idle => view::ArcadeSearchStatus::Idle,
        DomainArcadeSearchStatus::Searching => view::ArcadeSearchStatus::Searching,
        DomainArcadeSearchStatus::Ready => view::ArcadeSearchStatus::Ready,
        DomainArcadeSearchStatus::Failed => view::ArcadeSearchStatus::Failed,
    }
}

pub const fn arcade_search_pane(value: DomainArcadeSearchPane) -> view::ArcadeSearchPane {
    match value {
        DomainArcadeSearchPane::Keyboard => view::ArcadeSearchPane::Keyboard,
        DomainArcadeSearchPane::Results => view::ArcadeSearchPane::Results,
    }
}

pub const fn display_transaction_state(
    value: DisplayTransactionPhase,
) -> view::DisplayTransactionState {
    match value {
        DisplayTransactionPhase::Idle => view::DisplayTransactionState::Idle,
        DisplayTransactionPhase::Provisional => view::DisplayTransactionState::Provisional,
        DisplayTransactionPhase::Persisting => view::DisplayTransactionState::Persisting,
        DisplayTransactionPhase::Failed => view::DisplayTransactionState::Failed,
    }
}

pub const fn confirmation_kind(value: Option<ConfirmAction>) -> view::ConfirmationKind {
    match value {
        None => view::ConfirmationKind::None,
        Some(ConfirmAction::ExitToMister) => view::ConfirmationKind::ExitToMister,
        Some(ConfirmAction::RebuildDatabase) => view::ConfirmationKind::RebuildDatabase,
        Some(ConfirmAction::Restart) => view::ConfirmationKind::Restart,
        Some(ConfirmAction::LibraryChanged) => view::ConfirmationKind::LibraryChanged,
        Some(ConfirmAction::LibraryUpdateFailed) => view::ConfirmationKind::LibraryUpdateFailed,
        Some(ConfirmAction::DisplayResolution) => view::ConfirmationKind::DisplayResolution,
        Some(ConfirmAction::DisplayResolutionError) => {
            view::ConfirmationKind::DisplayResolutionError
        }
        Some(ConfirmAction::ScreenOrientation) => view::ConfirmationKind::ScreenOrientation,
        Some(ConfirmAction::AddFavourite) => view::ConfirmationKind::AddFavourite,
        Some(ConfirmAction::RemoveFavourite) => view::ConfirmationKind::RemoveFavourite,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_domain_variant_has_a_named_view_variant() {
        assert_eq!(
            launcher_screen(Screen::SystemHub),
            view::LauncherScreen::SystemHub
        );
        assert_eq!(system_hub_section(3), view::SystemHubSection::Information);
        assert_eq!(menu_hierarchy(true), view::MenuHierarchy::Root);
        assert_eq!(menu_hierarchy(false), view::MenuHierarchy::Nested);
        assert_eq!(
            setup_phase(DomainSetupPhase::NameKind),
            view::SetupPhase::NameKind
        );
        assert_eq!(
            arcade_search_status(DomainArcadeSearchStatus::Failed),
            view::ArcadeSearchStatus::Failed
        );
        assert_eq!(
            arcade_list_mode(ArcadeUserListMode::Favourites),
            view::ArcadeListMode::Favourites
        );
        assert_eq!(
            arcade_search_pane(DomainArcadeSearchPane::Results),
            view::ArcadeSearchPane::Results
        );
        assert_eq!(
            confirmation_kind(Some(ConfirmAction::RemoveFavourite)),
            view::ConfirmationKind::RemoveFavourite
        );
    }

    #[test]
    fn active_display_identity_is_not_confused_with_the_filtered_settings_index() {
        let runtime_index = mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS
            .iter()
            .position(|mode| mode.id == "crt-480p60")
            .expect("CRT 480p runtime mode");

        assert_eq!(
            active_display_choice(runtime_index).id.as_str(),
            "crt-480p60"
        );
        assert!(selected_display_choice(runtime_index).id.is_empty());
        assert_ne!(
            settings_display_choice(runtime_index).id.as_str(),
            "crt-480p60"
        );
    }
}
