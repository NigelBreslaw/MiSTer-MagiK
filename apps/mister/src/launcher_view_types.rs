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
        assert_eq!(
            setup_phase(DomainSetupPhase::NameKind),
            view::SetupPhase::NameKind
        );
        assert_eq!(
            arcade_search_status(DomainArcadeSearchStatus::Failed),
            view::ArcadeSearchStatus::Failed
        );
        assert_eq!(
            confirmation_kind(Some(ConfirmAction::RemoveFavourite)),
            view::ConfirmationKind::RemoveFavourite
        );
    }
}
