// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Production data model for the card launcher: the six root cards and the
//! generic cards of every nested hierarchy level.

use crate::arcade_catalog::{ArcadeCatalog, MENU_ARCADE_SYSTEM_ID};
use crate::launcher::LauncherNav;
use crate::launcher_taxonomy::{
    COMPUTERS_MENU_ID, CONSOLES_MENU_ID, HANDHELDS_MENU_ID, LauncherMenuItemKind, ROOT_MENU_ID,
};
use mister_magik_framebuffer_scenes::launcher::{
    LauncherCard, LauncherCardId, LauncherData, LauncherLevel, NestedLevel,
};

pub const CARD_COUNT: usize = 6;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LauncherHomeCounts {
    pub arcade: u32,
    pub consoles: u32,
    pub computers: u32,
    pub handhelds: u32,
    pub favourites: u32,
    pub collections: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LauncherHomeSnapshot {
    pub cards: [LauncherCard<'static>; CARD_COUNT],
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
        let arcade = menu_count(MENU_ARCADE_SYSTEM_ID);
        let consoles = menu_count(CONSOLES_MENU_ID);
        let computers = menu_count(COMPUTERS_MENU_ID);
        let handhelds = menu_count(HANDHELDS_MENU_ID);
        Self::from_counts(LauncherHomeCounts {
            arcade,
            consoles,
            computers,
            handhelds,
            favourites: saturating_u32(nav.favourite_count()),
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
            library_games: counts
                .arcade
                .saturating_add(counts.consoles)
                .saturating_add(counts.computers)
                .saturating_add(counts.handhelds),
            collections: counts.collections,
            favourites: counts.favourites,
        }
    }
}

/// Root collection colours, shared by every card below each root card.
const CONSOLES_COLOUR: u16 = 0x2a7f;
const COMPUTERS_COLOUR: u16 = 0xedc6;
const HANDHELDS_COLOUR: u16 = 0x2df2;

/// One carousel level with owned labels. The root keeps its approved artwork
/// cards; nested levels are generic cards in their root collection's colour.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CardLevelSnapshot {
    /// Stable level identity; a change is a level change, not a data refresh.
    pub menu_id: String,
    /// 0 at the root, 1 for Consoles, and so on.
    pub depth: usize,
    pub cards: Vec<LevelCard>,
    pub summary: LevelSummary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LevelCard {
    pub id: LauncherCardId,
    pub name: String,
    pub games: Option<u32>,
    pub colour: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LevelSummary {
    Root {
        library_games: u32,
        collections: u32,
        favourites: u32,
    },
    Nested {
        path: Vec<String>,
        games: u32,
        children_label: &'static str,
        systems: Option<u32>,
        accent: u16,
    },
}

impl CardLevelSnapshot {
    pub fn from_runtime(nav: &LauncherNav, catalog: &ArcadeCatalog) -> Self {
        if nav.current_menu_id() == ROOT_MENU_ID {
            return Self::root(&LauncherHomeSnapshot::from_runtime(nav, catalog));
        }
        let (id, accent) = match nav.current_menu_root_id() {
            Some(COMPUTERS_MENU_ID) => (LauncherCardId::Computers, COMPUTERS_COLOUR),
            Some(HANDHELDS_MENU_ID) => (LauncherCardId::Handhelds, HANDHELDS_COLOUR),
            _ => (LauncherCardId::Consoles, CONSOLES_COLOUR),
        };
        let items = nav.current_menu_items();
        let cards: Vec<_> = items
            .iter()
            .map(|item| LevelCard {
                id,
                name: item.title.to_uppercase(),
                games: Some(saturating_u32(item.count)),
                colour: accent,
            })
            .collect();
        let groups = items
            .iter()
            .any(|item| item.kind == LauncherMenuItemKind::Menu);
        Self {
            menu_id: nav.current_menu_id().to_owned(),
            depth: nav.current_menu_depth(),
            summary: LevelSummary::Nested {
                path: nav
                    .current_menu_path_titles()
                    .into_iter()
                    .map(str::to_uppercase)
                    .collect(),
                games: items.iter().fold(0_u32, |sum, item| {
                    sum.saturating_add(saturating_u32(item.count))
                }),
                children_label: if groups { "MAKERS" } else { "SYSTEMS" },
                systems: groups.then(|| saturating_u32(nav.current_menu_collection_count())),
                accent,
            },
            cards,
        }
    }

    pub fn root(snapshot: &LauncherHomeSnapshot) -> Self {
        Self {
            menu_id: ROOT_MENU_ID.to_owned(),
            depth: 0,
            cards: snapshot
                .cards
                .iter()
                .map(|card| LevelCard {
                    id: card.id,
                    name: card.name.to_owned(),
                    games: card.games,
                    colour: card.colour,
                })
                .collect(),
            summary: LevelSummary::Root {
                library_games: snapshot.library_games,
                collections: snapshot.collections,
                favourites: snapshot.favourites,
            },
        }
    }

    /// True while this snapshot still describes the navigation state. Checks
    /// without allocating, so the loop rebuilds only after a real change.
    pub fn matches_runtime(&self, nav: &LauncherNav, catalog: &ArcadeCatalog) -> bool {
        if self.menu_id != nav.current_menu_id() {
            return false;
        }
        if nav.current_menu_id() == ROOT_MENU_ID {
            let home = LauncherHomeSnapshot::from_runtime(nav, catalog);
            return matches!(
                self.summary,
                LevelSummary::Root {
                    library_games,
                    collections,
                    favourites,
                } if library_games == home.library_games
                    && collections == home.collections
                    && favourites == home.favourites
            ) && self.cards.len() == home.cards.len()
                && self.cards.iter().zip(&home.cards).all(|(card, root)| {
                    card.id == root.id
                        && card.name == root.name
                        && card.games == root.games
                        && card.colour == root.colour
                });
        }
        let items = nav.current_menu_items();
        items.len() == self.cards.len()
            && items.iter().zip(&self.cards).all(|(item, card)| {
                card.games == Some(saturating_u32(item.count))
                    && card.name.eq_ignore_ascii_case(&item.title)
            })
    }

    pub const fn is_root(&self) -> bool {
        matches!(self.summary, LevelSummary::Root { .. })
    }

    /// Borrow this level as renderer data. Allocates only small views, so
    /// call it when preparing or refreshing chrome, never per frame.
    pub fn with_data<R>(
        &self,
        selected: usize,
        clock: &str,
        render: impl FnOnce(LauncherData<'_>) -> R,
    ) -> R {
        let cards: Vec<_> = self
            .cards
            .iter()
            .map(|card| LauncherCard {
                id: card.id,
                name: &card.name,
                games: card.games,
                colour: card.colour,
            })
            .collect();
        match &self.summary {
            LevelSummary::Root {
                library_games,
                collections,
                favourites,
            } => render(LauncherData {
                cards: &cards,
                selected,
                library_games: *library_games,
                collections: *collections,
                favourites: *favourites,
                clock,
                level: LauncherLevel::Root,
            }),
            LevelSummary::Nested {
                path,
                games,
                children_label,
                systems,
                accent,
            } => {
                let path: Vec<_> = path.iter().map(String::as_str).collect();
                render(LauncherData {
                    cards: &cards,
                    selected,
                    library_games: 0,
                    collections: 0,
                    favourites: 0,
                    clock,
                    level: LauncherLevel::Nested(NestedLevel {
                        path: &path,
                        games: *games,
                        children: self.cards.len() as u32,
                        children_label,
                        detail: systems.map(|systems| (systems, "SYSTEMS")),
                        accent: *accent,
                    }),
                })
            }
        }
    }
}

const fn card(
    id: LauncherCardId,
    label: &'static str,
    games: Option<u32>,
    colour: u16,
) -> LauncherCard<'static> {
    LauncherCard {
        id,
        name: label,
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
