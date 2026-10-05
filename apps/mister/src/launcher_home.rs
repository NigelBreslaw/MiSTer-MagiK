// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Production data model for the card launcher: the six root cards and the
//! cards of every nested hierarchy level, with stable artwork identities.

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
        // The root's counts, whichever level is showing: the parent of a
        // nested level is prepared from here too.
        let menu_count = |id: &str| {
            nav.menu_items_of(ROOT_MENU_ID)
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
                    mister_magik_framebuffer_scenes::launcher::LauncherCardStyle::root(
                        LauncherCardId::Arcade,
                    )
                    .colour,
                ),
                card(
                    LauncherCardId::Consoles,
                    "CONSOLES",
                    Some(counts.consoles),
                    mister_magik_framebuffer_scenes::launcher::LauncherCardStyle::root(
                        LauncherCardId::Consoles,
                    )
                    .colour,
                ),
                card(
                    LauncherCardId::Computers,
                    "COMPUTERS",
                    Some(counts.computers),
                    mister_magik_framebuffer_scenes::launcher::LauncherCardStyle::root(
                        LauncherCardId::Computers,
                    )
                    .colour,
                ),
                card(
                    LauncherCardId::Handhelds,
                    "HANDHELDS",
                    Some(counts.handhelds),
                    mister_magik_framebuffer_scenes::launcher::LauncherCardStyle::root(
                        LauncherCardId::Handhelds,
                    )
                    .colour,
                ),
                card(
                    LauncherCardId::Favourites,
                    "FAVOURITES",
                    Some(counts.favourites),
                    mister_magik_framebuffer_scenes::launcher::LauncherCardStyle::root(
                        LauncherCardId::Favourites,
                    )
                    .colour,
                ),
                card(
                    LauncherCardId::Settings,
                    "SETTINGS",
                    None,
                    mister_magik_framebuffer_scenes::launcher::LauncherCardStyle::root(
                        LauncherCardId::Settings,
                    )
                    .colour,
                ),
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

/// One carousel level with owned labels and stable artwork identities.
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
    /// Stable navigation identity. Artwork lookup keys have a separate namespace.
    pub navigation_id: String,
    /// Asset lookup key; root artwork and navigation use distinct namespaces.
    pub artwork_key: String,
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
        Self::for_menu(nav, catalog, nav.current_menu_id())
    }

    /// The level shown when `menu_id` is the current menu. Used to prepare a
    /// level before it is entered.
    pub fn for_menu(nav: &LauncherNav, catalog: &ArcadeCatalog, menu_id: &str) -> Self {
        if menu_id == ROOT_MENU_ID {
            return Self::root(&LauncherHomeSnapshot::from_runtime(nav, catalog));
        }
        let path = nav.menu_path_to(menu_id).unwrap_or_default();
        let style = mister_magik_framebuffer_scenes::launcher::LauncherCardStyle::section(
            path.get(1).map_or("", String::as_str),
        );
        let (id, accent) = (style.id, style.colour);
        let items = nav.menu_items_of(menu_id);
        let cards: Vec<_> = items
            .iter()
            .map(|item| LevelCard {
                navigation_id: item.id.clone(),
                artwork_key: item.id.clone(),
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
            menu_id: menu_id.to_owned(),
            depth: path.len().saturating_sub(1),
            summary: LevelSummary::Nested {
                path: nav
                    .menu_path_titles_of(menu_id)
                    .into_iter()
                    .map(str::to_uppercase)
                    .collect(),
                games: items.iter().fold(0_u32, |sum, item| {
                    sum.saturating_add(saturating_u32(item.count))
                }),
                children_label: if groups { "MAKERS" } else { "SYSTEMS" },
                systems: groups.then(|| saturating_u32(nav.menu_collection_count_of(menu_id))),
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
                    navigation_id: crate::launcher::root_home_card_identity(card.id).to_owned(),
                    id: card.id,
                    artwork_key: match card.id {
                        LauncherCardId::Arcade => "root:arcade",
                        LauncherCardId::Consoles => "root:consoles",
                        LauncherCardId::Computers => "root:computers",
                        LauncherCardId::Handhelds => "root:handhelds",
                        LauncherCardId::Favourites => "root:favourites",
                        LauncherCardId::Settings => "root:settings",
                    }
                    .to_owned(),
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
                card.artwork_key == item.id
                    && card.games == Some(saturating_u32(item.count))
                    && card.name.eq_ignore_ascii_case(&item.title)
            })
    }

    /// Whether browsing wraps: always at the root, and in a nested level once
    /// it has enough cards to fill the carousel without repeating one.
    pub fn cycles(&self) -> bool {
        self.is_root()
            || self.cards.len() >= mister_magik_framebuffer_scenes::launcher::CYCLIC_LEVEL_MIN_CARDS
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
