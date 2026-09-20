// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Production data model for the six-card root launcher.

use crate::arcade_catalog::{ArcadeCatalog, MENU_ARCADE_SYSTEM_ID};
use crate::launcher::LauncherNav;
use crate::launcher_taxonomy::{COMPUTERS_MENU_ID, CONSOLES_MENU_ID, HANDHELDS_MENU_ID};
use mister_magik_framebuffer_scenes::launcher::{LauncherCard, LauncherCardId};

pub const CARD_COUNT: usize = 6;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LauncherHomeCounts {
    pub arcade: u32,
    pub consoles: u32,
    pub computers: u32,
    pub handhelds: u32,
    pub favourites: u32,
    pub library_games: u32,
    pub collections: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LauncherHomeCard {
    pub id: LauncherCardId,
    pub label: &'static str,
    pub games: Option<u32>,
    pub colour: u16,
}

impl LauncherHomeCard {
    pub const fn borrowed(&self) -> LauncherCard<'_> {
        LauncherCard {
            id: self.id,
            name: self.label,
            games: self.games,
            colour: self.colour,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LauncherHomeSnapshot {
    pub cards: [LauncherHomeCard; CARD_COUNT],
    pub library_games: u32,
    pub collections: u32,
    pub favourites: u32,
}

impl LauncherHomeSnapshot {
    pub fn from_runtime(nav: &LauncherNav, catalog: &ArcadeCatalog) -> Self {
        let menu_count = |id: &str| {
            nav.current_menu_items()
                .iter()
                .find(|item| item.id == id)
                .map_or(0, |item| saturating_u32(item.count))
        };
        Self::from_counts(LauncherHomeCounts {
            arcade: menu_count(MENU_ARCADE_SYSTEM_ID),
            consoles: menu_count(CONSOLES_MENU_ID),
            computers: menu_count(COMPUTERS_MENU_ID),
            handhelds: menu_count(HANDHELDS_MENU_ID),
            favourites: saturating_u32(nav.favourite_count()),
            library_games: saturating_u32(catalog.games.len()),
            collections: saturating_u32(catalog.systems.len()),
        })
    }

    pub const fn from_counts(counts: LauncherHomeCounts) -> Self {
        Self {
            cards: [
                card(
                    LauncherCardId::Arcade,
                    "ARCADE",
                    Some(counts.arcade),
                    0xe1a5,
                ),
                card(
                    LauncherCardId::Consoles,
                    "CONSOLES",
                    Some(counts.consoles),
                    0x2a7f,
                ),
                card(
                    LauncherCardId::Computers,
                    "COMPUTERS",
                    Some(counts.computers),
                    0xedc6,
                ),
                card(
                    LauncherCardId::Handhelds,
                    "HANDHELDS",
                    Some(counts.handhelds),
                    0x2df2,
                ),
                card(
                    LauncherCardId::Favourites,
                    "FAVOURITES",
                    Some(counts.favourites),
                    0xe12f,
                ),
                card(LauncherCardId::Settings, "SETTINGS", None, 0x8b7f),
            ],
            library_games: counts.library_games,
            collections: counts.collections,
            favourites: counts.favourites,
        }
    }
}

const fn card(
    id: LauncherCardId,
    label: &'static str,
    games: Option<u32>,
    colour: u16,
) -> LauncherHomeCard {
    LauncherHomeCard {
        id,
        label,
        games,
        colour,
    }
}

fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_keep_the_approved_order_and_settings_has_no_fake_count() {
        let snapshot = LauncherHomeSnapshot::from_counts(LauncherHomeCounts {
            arcade: 1,
            consoles: 2,
            computers: 3,
            handhelds: 4,
            favourites: 5,
            library_games: 15,
            collections: 4,
        });
        assert_eq!(
            snapshot.cards.map(|card| card.id),
            [
                LauncherCardId::Arcade,
                LauncherCardId::Consoles,
                LauncherCardId::Computers,
                LauncherCardId::Handhelds,
                LauncherCardId::Favourites,
                LauncherCardId::Settings,
            ]
        );
        assert_eq!(snapshot.cards[4].games, Some(5));
        assert_eq!(snapshot.cards[5].games, None);
    }
}
