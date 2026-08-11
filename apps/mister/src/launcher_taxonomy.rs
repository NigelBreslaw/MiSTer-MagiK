// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Stable, catalog-derived hierarchy used by the launcher home screen.

use crate::arcade_catalog::{ArcadeCatalog, GameSystemEntry, MENU_ARCADE_SYSTEM_ID, PlatformKind};
#[cfg(test)]
use mister_magik_catalog::catalog_classify::system_definitions;
use mister_magik_catalog::catalog_classify::{
    LauncherSection, normalize_system_id, system_definition,
};
use std::collections::HashMap;
use std::sync::Arc;

pub const ROOT_MENU_ID: &str = "menu:root";
pub const SNK_NEOGEO_MENU_ID: &str = "menu:snk-neogeo";
pub const CONSOLES_MENU_ID: &str = "menu:consoles";
pub const HANDHELDS_MENU_ID: &str = "menu:handhelds";
pub const COMPUTERS_MENU_ID: &str = "menu:computers";

const CONSOLES_ATARI_MENU_ID: &str = "menu:consoles:atari";
const CONSOLES_SEGA_MENU_ID: &str = "menu:consoles:sega";
const CONSOLES_SONY_MENU_ID: &str = "menu:consoles:sony";
const CONSOLES_NINTENDO_MENU_ID: &str = "menu:consoles:nintendo";
const CONSOLES_NEC_MENU_ID: &str = "menu:consoles:nec";
const CONSOLES_OTHER_MENU_ID: &str = "menu:consoles:other";

const HANDHELDS_NINTENDO_MENU_ID: &str = "menu:handhelds:nintendo";
const HANDHELDS_SEGA_MENU_ID: &str = "menu:handhelds:sega";
const HANDHELDS_ATARI_MENU_ID: &str = "menu:handhelds:atari";
const HANDHELDS_SNK_MENU_ID: &str = "menu:handhelds:snk";
const HANDHELDS_BANDAI_MENU_ID: &str = "menu:handhelds:bandai";
const HANDHELDS_OTHER_MENU_ID: &str = "menu:handhelds:other";

const COMPUTERS_ACORN_MENU_ID: &str = "menu:computers:acorn";
const COMPUTERS_APPLE_MENU_ID: &str = "menu:computers:apple";
const COMPUTERS_COMMODORE_MENU_ID: &str = "menu:computers:commodore";
const COMPUTERS_ATARI_MENU_ID: &str = "menu:computers:atari";
const COMPUTERS_SINCLAIR_MENU_ID: &str = "menu:computers:sinclair";
const COMPUTERS_TANDY_MENU_ID: &str = "menu:computers:tandy";
const COMPUTERS_DOS_PC_MENU_ID: &str = "menu:computers:dos-pc";
const COMPUTERS_JAPANESE_MENU_ID: &str = "menu:computers:japanese";
const COMPUTERS_OTHER_MENU_ID: &str = "menu:computers:other";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LauncherMenuItemKind {
    Menu,
    Collection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherMenuItem {
    pub id: String,
    pub title: String,
    pub count: usize,
    pub kind: LauncherMenuItemKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherMenu {
    pub id: String,
    pub title: String,
    pub parent_id: Option<String>,
    pub items: Vec<LauncherMenuItem>,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherCollection {
    /// Stable scope ID accepted by the catalog view/filter/search APIs.
    pub id: String,
    pub title: String,
    pub count: usize,
    /// Exact catalog system for ordinary leaves. Virtual collections have no
    /// exact system and retain a legacy system hint instead.
    pub system_id: Option<String>,
    pub legacy_system_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherDestination {
    /// Root-to-parent menu IDs for the collection.
    pub menu_path: Vec<String>,
    pub collection_id: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LauncherTaxonomyToken {
    games_ptr: usize,
    systems_ptr: usize,
    systems_len: usize,
}

impl LauncherTaxonomyToken {
    pub fn from_catalog(catalog: &ArcadeCatalog) -> Self {
        Self {
            games_ptr: Arc::as_ptr(&catalog.games) as usize,
            systems_ptr: catalog.systems.as_ptr() as usize,
            systems_len: catalog.systems.len(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LauncherTaxonomy {
    token: LauncherTaxonomyToken,
    menus: HashMap<String, LauncherMenu>,
    collections: HashMap<String, LauncherCollection>,
    primary_system_destinations: HashMap<String, LauncherDestination>,
    primary_collection_destinations: HashMap<String, LauncherDestination>,
    diagnostics: Vec<String>,
}

impl LauncherTaxonomy {
    pub fn from_catalog(catalog: &ArcadeCatalog) -> Self {
        TaxonomyBuilder::new(catalog, None).build()
    }

    pub fn from_catalog_with_shells(
        catalog: &ArcadeCatalog,
        visible_zero_systems: &std::collections::HashSet<String>,
    ) -> Self {
        TaxonomyBuilder::new(catalog, Some(visible_zero_systems)).build()
    }

    pub fn token(&self) -> LauncherTaxonomyToken {
        self.token
    }

    pub fn matches_catalog(&self, catalog: &ArcadeCatalog) -> bool {
        self.token == LauncherTaxonomyToken::from_catalog(catalog)
    }

    pub fn menu(&self, id: &str) -> Option<&LauncherMenu> {
        self.menus.get(resolve_menu_alias(id))
    }

    pub fn root_menu(&self) -> Option<&LauncherMenu> {
        self.menu(ROOT_MENU_ID)
    }

    pub fn collection(&self, id: &str) -> Option<&LauncherCollection> {
        self.collections.get(id)
    }

    pub fn primary_destination_for_system(&self, system_id: &str) -> Option<&LauncherDestination> {
        self.primary_system_destinations
            .get(&normalize_system_id(system_id))
    }

    pub fn primary_destination_for_collection(
        &self,
        collection_id: &str,
    ) -> Option<&LauncherDestination> {
        self.primary_collection_destinations.get(collection_id)
    }

    pub fn path_to_menu(&self, id: &str) -> Option<Vec<String>> {
        let mut current = self.menu(id)?;
        let mut reversed = vec![current.id.clone()];
        while let Some(parent) = current.parent_id.as_deref() {
            current = self.menu(parent)?;
            reversed.push(current.id.clone());
        }
        reversed.reverse();
        (reversed.first().is_some_and(|id| id == ROOT_MENU_ID)).then_some(reversed)
    }

    pub fn menu_contains_item(&self, menu_id: &str, item_id: &str) -> bool {
        self.menu(menu_id)
            .is_some_and(|menu| menu.items.iter().any(|item| item.id == item_id))
    }

    pub fn collection_path_is_valid(&self, menu_path: &[String], collection_id: &str) -> bool {
        if menu_path.first().map(String::as_str) != Some(ROOT_MENU_ID) {
            return false;
        }
        for pair in menu_path.windows(2) {
            if !self.menu_contains_item(&pair[0], &pair[1]) || self.menu(&pair[1]).is_none() {
                return false;
            }
        }
        menu_path
            .last()
            .is_some_and(|parent| self.menu_contains_item(parent, collection_id))
            && self.collection(collection_id).is_some()
    }

    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
}

fn resolve_menu_alias(id: &str) -> &str {
    let trimmed = id.trim();
    if trimmed.eq_ignore_ascii_case("root") || trimmed.eq_ignore_ascii_case(ROOT_MENU_ID) {
        ROOT_MENU_ID
    } else if trimmed.eq_ignore_ascii_case("snk-neogeo")
        || trimmed.eq_ignore_ascii_case("snk_neogeo")
        || trimmed.eq_ignore_ascii_case(SNK_NEOGEO_MENU_ID)
    {
        SNK_NEOGEO_MENU_ID
    } else if trimmed.eq_ignore_ascii_case("consoles")
        || trimmed.eq_ignore_ascii_case(CONSOLES_MENU_ID)
    {
        CONSOLES_MENU_ID
    } else if trimmed.eq_ignore_ascii_case("handhelds")
        || trimmed.eq_ignore_ascii_case(HANDHELDS_MENU_ID)
    {
        HANDHELDS_MENU_ID
    } else if trimmed.eq_ignore_ascii_case("computers")
        || trimmed.eq_ignore_ascii_case(COMPUTERS_MENU_ID)
    {
        COMPUTERS_MENU_ID
    } else {
        trimmed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Bucket {
    ConsoleAtari,
    ConsoleSega,
    ConsoleSony,
    ConsoleNintendo,
    ConsoleNec,
    ConsoleOther,
    HandheldNintendo,
    HandheldSega,
    HandheldAtari,
    HandheldSnk,
    HandheldBandai,
    HandheldOther,
    ComputerAcorn,
    ComputerApple,
    ComputerCommodore,
    ComputerAtari,
    ComputerSinclair,
    ComputerTandy,
    ComputerDosPc,
    ComputerJapanese,
    ComputerOther,
    SnkOnly,
    ArcadeAggregate,
}

#[derive(Clone)]
struct ClassifiedSystem {
    entry: GameSystemEntry,
    count: usize,
    rank: usize,
}

struct TaxonomyBuilder<'a> {
    catalog: &'a ArcadeCatalog,
    visible_zero_systems: Option<&'a std::collections::HashSet<String>>,
    taxonomy: LauncherTaxonomy,
    buckets: HashMap<Bucket, Vec<ClassifiedSystem>>,
    snk_systems: Vec<ClassifiedSystem>,
}

impl<'a> TaxonomyBuilder<'a> {
    fn new(
        catalog: &'a ArcadeCatalog,
        visible_zero_systems: Option<&'a std::collections::HashSet<String>>,
    ) -> Self {
        Self {
            catalog,
            visible_zero_systems,
            taxonomy: LauncherTaxonomy {
                token: LauncherTaxonomyToken::from_catalog(catalog),
                ..LauncherTaxonomy::default()
            },
            buckets: HashMap::new(),
            snk_systems: Vec::new(),
        }
    }

    fn build(mut self) -> LauncherTaxonomy {
        self.classify_catalog_systems();
        self.build_exact_collection_menus();
        self.build_snk_menu();
        self.build_category_menus();
        self.build_root_menu();
        self.record_collection_destinations();
        self.taxonomy
    }

    fn classify_catalog_systems(&mut self) {
        for system in &self.catalog.systems {
            let id = normalize_system_id(&system.id);
            let count = system.count.max(self.catalog.system_game_count(&system.id));
            if count == 0
                && !self
                    .visible_zero_systems
                    .is_some_and(|systems| systems.contains(&id))
            {
                continue;
            }
            let kind = self.catalog.platform_kind(&system.id);
            let (bucket, rank) = classify(&id, kind);
            if bucket == Bucket::ArcadeAggregate {
                self.taxonomy.primary_system_destinations.insert(
                    id,
                    LauncherDestination {
                        menu_path: vec![ROOT_MENU_ID.to_string()],
                        collection_id: MENU_ARCADE_SYSTEM_ID.to_string(),
                    },
                );
                continue;
            }

            let classified = ClassifiedSystem {
                entry: system.clone(),
                count,
                rank,
            };
            self.add_exact_collection(&classified);
            if bucket == Bucket::SnkOnly {
                self.snk_systems.push(classified);
            } else {
                self.buckets.entry(bucket).or_default().push(classified);
            }
            if kind == PlatformKind::Unknown && bucket == Bucket::ConsoleOther {
                self.taxonomy.diagnostics.push(format!(
                    "launcher taxonomy: unknown platform kind for system {}; classified as Consoles / Other",
                    system.id
                ));
            }
        }
    }

    fn add_exact_collection(&mut self, system: &ClassifiedSystem) {
        self.taxonomy.collections.insert(
            system.entry.id.clone(),
            LauncherCollection {
                id: system.entry.id.clone(),
                title: system.entry.title.clone(),
                count: system.count,
                system_id: Some(system.entry.id.clone()),
                legacy_system_id: system.entry.id.clone(),
            },
        );
    }

    fn build_exact_collection_menus(&mut self) {
        const SPECS: &[(Bucket, &str, &str, &str)] = &[
            (
                Bucket::ConsoleAtari,
                CONSOLES_ATARI_MENU_ID,
                "Atari",
                CONSOLES_MENU_ID,
            ),
            (
                Bucket::ConsoleSega,
                CONSOLES_SEGA_MENU_ID,
                "Sega",
                CONSOLES_MENU_ID,
            ),
            (
                Bucket::ConsoleSony,
                CONSOLES_SONY_MENU_ID,
                "Sony",
                CONSOLES_MENU_ID,
            ),
            (
                Bucket::ConsoleNintendo,
                CONSOLES_NINTENDO_MENU_ID,
                "Nintendo",
                CONSOLES_MENU_ID,
            ),
            (
                Bucket::ConsoleNec,
                CONSOLES_NEC_MENU_ID,
                "NEC",
                CONSOLES_MENU_ID,
            ),
            (
                Bucket::ConsoleOther,
                CONSOLES_OTHER_MENU_ID,
                "Other",
                CONSOLES_MENU_ID,
            ),
            (
                Bucket::HandheldNintendo,
                HANDHELDS_NINTENDO_MENU_ID,
                "Nintendo",
                HANDHELDS_MENU_ID,
            ),
            (
                Bucket::HandheldSega,
                HANDHELDS_SEGA_MENU_ID,
                "Sega",
                HANDHELDS_MENU_ID,
            ),
            (
                Bucket::HandheldAtari,
                HANDHELDS_ATARI_MENU_ID,
                "Atari",
                HANDHELDS_MENU_ID,
            ),
            (
                Bucket::HandheldSnk,
                HANDHELDS_SNK_MENU_ID,
                "SNK",
                HANDHELDS_MENU_ID,
            ),
            (
                Bucket::HandheldBandai,
                HANDHELDS_BANDAI_MENU_ID,
                "Bandai",
                HANDHELDS_MENU_ID,
            ),
            (
                Bucket::HandheldOther,
                HANDHELDS_OTHER_MENU_ID,
                "Other",
                HANDHELDS_MENU_ID,
            ),
            (
                Bucket::ComputerAcorn,
                COMPUTERS_ACORN_MENU_ID,
                "Acorn",
                COMPUTERS_MENU_ID,
            ),
            (
                Bucket::ComputerApple,
                COMPUTERS_APPLE_MENU_ID,
                "Apple",
                COMPUTERS_MENU_ID,
            ),
            (
                Bucket::ComputerCommodore,
                COMPUTERS_COMMODORE_MENU_ID,
                "Commodore",
                COMPUTERS_MENU_ID,
            ),
            (
                Bucket::ComputerAtari,
                COMPUTERS_ATARI_MENU_ID,
                "Atari",
                COMPUTERS_MENU_ID,
            ),
            (
                Bucket::ComputerSinclair,
                COMPUTERS_SINCLAIR_MENU_ID,
                "Sinclair",
                COMPUTERS_MENU_ID,
            ),
            (
                Bucket::ComputerTandy,
                COMPUTERS_TANDY_MENU_ID,
                "Tandy/Radio Shack",
                COMPUTERS_MENU_ID,
            ),
            (
                Bucket::ComputerDosPc,
                COMPUTERS_DOS_PC_MENU_ID,
                "DOS/PC",
                COMPUTERS_MENU_ID,
            ),
            (
                Bucket::ComputerJapanese,
                COMPUTERS_JAPANESE_MENU_ID,
                "Japanese Computers",
                COMPUTERS_MENU_ID,
            ),
            (
                Bucket::ComputerOther,
                COMPUTERS_OTHER_MENU_ID,
                "Other",
                COMPUTERS_MENU_ID,
            ),
        ];

        for &(bucket, menu_id, title, parent_id) in SPECS {
            let Some(mut systems) = self.buckets.remove(&bucket) else {
                continue;
            };
            systems.sort_by(|a, b| {
                a.rank
                    .cmp(&b.rank)
                    .then_with(|| a.entry.title.cmp(&b.entry.title))
                    .then_with(|| a.entry.id.cmp(&b.entry.id))
            });
            let items = systems
                .iter()
                .map(|system| LauncherMenuItem {
                    id: system.entry.id.clone(),
                    title: system.entry.title.clone(),
                    count: system.count,
                    kind: LauncherMenuItemKind::Collection,
                })
                .collect::<Vec<_>>();
            self.insert_menu(menu_id, title, Some(parent_id), items);
            for system in systems {
                self.taxonomy.primary_system_destinations.insert(
                    normalize_system_id(&system.entry.id),
                    LauncherDestination {
                        menu_path: vec![
                            ROOT_MENU_ID.to_string(),
                            parent_id.to_string(),
                            menu_id.to_string(),
                        ],
                        collection_id: system.entry.id,
                    },
                );
            }
        }
    }

    fn build_category_menus(&mut self) {
        self.insert_parent_menu(
            CONSOLES_MENU_ID,
            "Consoles",
            &[
                CONSOLES_ATARI_MENU_ID,
                CONSOLES_SEGA_MENU_ID,
                CONSOLES_SONY_MENU_ID,
                CONSOLES_NINTENDO_MENU_ID,
                CONSOLES_NEC_MENU_ID,
                SNK_NEOGEO_MENU_ID,
                CONSOLES_OTHER_MENU_ID,
            ],
        );
        self.insert_parent_menu(
            HANDHELDS_MENU_ID,
            "Handhelds",
            &[
                HANDHELDS_NINTENDO_MENU_ID,
                HANDHELDS_SEGA_MENU_ID,
                HANDHELDS_ATARI_MENU_ID,
                HANDHELDS_SNK_MENU_ID,
                HANDHELDS_BANDAI_MENU_ID,
                HANDHELDS_OTHER_MENU_ID,
            ],
        );
        self.insert_parent_menu(
            COMPUTERS_MENU_ID,
            "Computers",
            &[
                COMPUTERS_ACORN_MENU_ID,
                COMPUTERS_APPLE_MENU_ID,
                COMPUTERS_COMMODORE_MENU_ID,
                COMPUTERS_ATARI_MENU_ID,
                COMPUTERS_SINCLAIR_MENU_ID,
                COMPUTERS_TANDY_MENU_ID,
                COMPUTERS_DOS_PC_MENU_ID,
                COMPUTERS_JAPANESE_MENU_ID,
                COMPUTERS_OTHER_MENU_ID,
            ],
        );
    }

    fn build_snk_menu(&mut self) {
        let mut items = Vec::new();
        self.snk_systems.sort_by(|a, b| {
            a.rank
                .cmp(&b.rank)
                .then_with(|| a.entry.title.cmp(&b.entry.title))
        });
        for system in &self.snk_systems {
            items.push(LauncherMenuItem {
                id: system.entry.id.clone(),
                title: snk_title(&system.entry.id, &system.entry.title),
                count: system.count,
                kind: LauncherMenuItemKind::Collection,
            });
            self.taxonomy.primary_system_destinations.insert(
                normalize_system_id(&system.entry.id),
                LauncherDestination {
                    menu_path: vec![
                        ROOT_MENU_ID.to_string(),
                        CONSOLES_MENU_ID.to_string(),
                        SNK_NEOGEO_MENU_ID.to_string(),
                    ],
                    collection_id: system.entry.id.clone(),
                },
            );
        }

        self.insert_menu(
            SNK_NEOGEO_MENU_ID,
            "SNK NeoGeo",
            Some(CONSOLES_MENU_ID),
            items,
        );
    }

    fn build_root_menu(&mut self) {
        let resident_arcade_count = self.catalog.system_game_count(MENU_ARCADE_SYSTEM_ID);
        let declared_arcade_count = self
            .catalog
            .systems
            .iter()
            .filter(|system| self.catalog.platform_kind(&system.id) == PlatformKind::Arcade)
            .fold(0usize, |total, system| total.saturating_add(system.count));
        let arcade_count = resident_arcade_count.max(declared_arcade_count);
        let mut items = Vec::new();
        if arcade_count > 0 {
            self.taxonomy.collections.insert(
                MENU_ARCADE_SYSTEM_ID.to_string(),
                LauncherCollection {
                    id: MENU_ARCADE_SYSTEM_ID.to_string(),
                    title: "Arcade".to_string(),
                    count: arcade_count,
                    system_id: None,
                    legacy_system_id: "arcade".to_string(),
                },
            );
            items.push(LauncherMenuItem {
                id: MENU_ARCADE_SYSTEM_ID.to_string(),
                title: "Arcade".to_string(),
                count: arcade_count,
                kind: LauncherMenuItemKind::Collection,
            });
        }
        for menu_id in [CONSOLES_MENU_ID, COMPUTERS_MENU_ID, HANDHELDS_MENU_ID] {
            if let Some(menu) = self.taxonomy.menu(menu_id) {
                items.push(LauncherMenuItem {
                    id: menu.id.clone(),
                    title: menu.title.clone(),
                    count: menu.count,
                    kind: LauncherMenuItemKind::Menu,
                });
            }
        }
        self.insert_menu(ROOT_MENU_ID, "MiSTer MagiK", None, items);
    }

    fn record_collection_destinations(&mut self) {
        for destination in self.taxonomy.primary_system_destinations.values() {
            self.taxonomy
                .primary_collection_destinations
                .entry(destination.collection_id.clone())
                .or_insert_with(|| destination.clone());
        }
        if self.taxonomy.collection(MENU_ARCADE_SYSTEM_ID).is_some() {
            self.taxonomy.primary_collection_destinations.insert(
                MENU_ARCADE_SYSTEM_ID.to_string(),
                LauncherDestination {
                    menu_path: vec![ROOT_MENU_ID.to_string()],
                    collection_id: MENU_ARCADE_SYSTEM_ID.to_string(),
                },
            );
        }
    }

    fn insert_parent_menu(&mut self, id: &str, title: &str, child_ids: &[&str]) {
        let mut items = Vec::new();
        for child_id in child_ids {
            let Some(menu) = self.taxonomy.menu(child_id).cloned() else {
                continue;
            };
            if *child_id != SNK_NEOGEO_MENU_ID
                && menu.items.len() == 1
                && menu.items[0].kind == LauncherMenuItemKind::Collection
            {
                let mut item = menu.items[0].clone();
                item.title = flattened_vendor_title(&menu.title, &item.title);
                if let Some(destination) = self
                    .taxonomy
                    .primary_system_destinations
                    .get_mut(&normalize_system_id(&item.id))
                {
                    destination.menu_path = vec![ROOT_MENU_ID.to_string(), id.to_string()];
                }
                self.taxonomy.menus.remove(*child_id);
                items.push(item);
            } else {
                items.push(LauncherMenuItem {
                    id: menu.id,
                    title: menu.title,
                    count: menu.count,
                    kind: LauncherMenuItemKind::Menu,
                });
            }
        }
        self.insert_menu(id, title, Some(ROOT_MENU_ID), items);
    }

    fn insert_menu(
        &mut self,
        id: &str,
        title: &str,
        parent_id: Option<&str>,
        items: Vec<LauncherMenuItem>,
    ) {
        if id != ROOT_MENU_ID && items.is_empty() {
            return;
        }
        let count = items.iter().map(|item| item.count).sum();
        self.taxonomy.menus.insert(
            id.to_string(),
            LauncherMenu {
                id: id.to_string(),
                title: title.to_string(),
                parent_id: parent_id.map(str::to_string),
                items,
                count,
            },
        );
    }
}

fn classify(id: &str, kind: PlatformKind) -> (Bucket, usize) {
    if let Some(definition) = system_definition(id) {
        let bucket = match definition.section {
            LauncherSection::Arcade => Bucket::ArcadeAggregate,
            LauncherSection::SnkNeogeo => Bucket::SnkOnly,
            LauncherSection::Consoles => match definition.family.as_str() {
                "atari" => Bucket::ConsoleAtari,
                "sega" => Bucket::ConsoleSega,
                "sony" => Bucket::ConsoleSony,
                "nintendo" => Bucket::ConsoleNintendo,
                "nec" => Bucket::ConsoleNec,
                _ => Bucket::ConsoleOther,
            },
            LauncherSection::Handhelds => match definition.family.as_str() {
                "nintendo" => Bucket::HandheldNintendo,
                "sega" => Bucket::HandheldSega,
                "atari" => Bucket::HandheldAtari,
                "snk" => Bucket::HandheldSnk,
                "bandai" => Bucket::HandheldBandai,
                _ => Bucket::HandheldOther,
            },
            LauncherSection::Computers => match definition.family.as_str() {
                "acorn" => Bucket::ComputerAcorn,
                "apple" => Bucket::ComputerApple,
                "commodore" => Bucket::ComputerCommodore,
                "atari" => Bucket::ComputerAtari,
                "sinclair" => Bucket::ComputerSinclair,
                "tandy" => Bucket::ComputerTandy,
                "dos-pc" => Bucket::ComputerDosPc,
                "japanese" => Bucket::ComputerJapanese,
                _ => Bucket::ComputerOther,
            },
            LauncherSection::Other => Bucket::ConsoleOther,
        };
        return (bucket, usize::from(definition.order));
    }
    match kind {
        PlatformKind::Arcade => (Bucket::ArcadeAggregate, usize::MAX),
        PlatformKind::Console => (Bucket::ConsoleOther, usize::MAX),
        PlatformKind::Handheld => (Bucket::HandheldOther, usize::MAX),
        PlatformKind::Computer => (Bucket::ComputerOther, usize::MAX),
        PlatformKind::Unknown => (Bucket::ConsoleOther, usize::MAX),
    }
}

fn snk_title(id: &str, fallback: &str) -> String {
    match normalize_system_id(id).as_str() {
        "neogeo" | "neo-geo" | "snk-neo-geo" => "NeoGeo".to_string(),
        "neogeo-cd" => "NeoGeo CD".to_string(),
        "neogeopocket" | "ngpc" => "NeoGeo Pocket".to_string(),
        _ => fallback.to_string(),
    }
}

fn flattened_vendor_title(vendor: &str, collection: &str) -> String {
    if vendor.eq_ignore_ascii_case("Other")
        || collection
            .to_ascii_lowercase()
            .starts_with(&vendor.to_ascii_lowercase())
    {
        collection.to_string()
    } else {
        format!("{vendor} {collection}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arcade_catalog::{ArcadeGameEntry, DEFAULT_ARCADE_ROOT};
    use std::path::PathBuf;

    fn system(id: &str, count: usize) -> GameSystemEntry {
        GameSystemEntry {
            id: id.to_string(),
            title: id.to_string(),
            count,
        }
    }

    fn game(system_id: &str, title: &str, manufacturer: &str) -> ArcadeGameEntry {
        ArcadeGameEntry {
            title: title.into(),
            mra_path: format!("/{system_id}/{title}").into(),
            preview_archive_path: "".into(),
            preview_asset_key: "".into(),
            has_preview: false,
            system_id: system_id.into(),
            year: None,
            manufacturer: manufacturer.into(),
            category: "".into(),
            players: None,
            control: "".into(),
            is_new: false,
        }
    }

    fn catalog(games: Vec<ArcadeGameEntry>, systems: Vec<GameSystemEntry>) -> ArcadeCatalog {
        ArcadeCatalog::new(PathBuf::from(DEFAULT_ARCADE_ROOT), games, systems)
    }

    fn catalog_with_kind(id: &str, kind: PlatformKind) -> ArcadeCatalog {
        ArcadeCatalog::new_with_deferred_text_indexes_and_platform_kinds(
            PathBuf::from(DEFAULT_ARCADE_ROOT),
            Vec::new(),
            vec![system(id, 1)],
            Vec::new(),
            HashMap::from([(id.to_string(), kind)]),
        )
    }

    #[test]
    fn root_uses_fixed_four_item_order_when_every_branch_is_populated() {
        let catalog = catalog(
            vec![game("arcade", "Metal Slug", "SNK")],
            vec![
                system("arcade", 1),
                system("neogeo", 1),
                system("nes", 1),
                system("gb", 1),
                system("amiga", 1),
            ],
        );
        let taxonomy = LauncherTaxonomy::from_catalog(&catalog);
        let ids = taxonomy
            .root_menu()
            .expect("root")
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![
                MENU_ARCADE_SYSTEM_ID,
                CONSOLES_MENU_ID,
                COMPUTERS_MENU_ID,
                HANDHELDS_MENU_ID,
            ]
        );
    }

    #[test]
    fn registry_only_arcade_count_keeps_the_home_tile_without_resident_rows() {
        let catalog = catalog_with_kind("arcade", PlatformKind::Arcade);
        assert_eq!(catalog.system_game_count(MENU_ARCADE_SYSTEM_ID), 0);

        let taxonomy = LauncherTaxonomy::from_catalog(&catalog);
        let arcade = taxonomy
            .collection(MENU_ARCADE_SYSTEM_ID)
            .expect("registry Arcade tile");

        assert_eq!(arcade.count, 1);
        assert_eq!(
            taxonomy
                .primary_destination_for_system("arcade")
                .unwrap()
                .collection_id,
            MENU_ARCADE_SYSTEM_ID
        );
    }

    #[test]
    fn empty_groups_are_pruned_recursively() {
        let taxonomy = LauncherTaxonomy::from_catalog(&catalog(Vec::new(), vec![system("nes", 4)]));
        let root = taxonomy.root_menu().expect("root");
        assert_eq!(root.items.len(), 1);
        assert_eq!(root.items[0].id, CONSOLES_MENU_ID);
        assert!(taxonomy.menu(HANDHELDS_MENU_ID).is_none());
        assert!(taxonomy.menu(CONSOLES_OTHER_MENU_ID).is_none());
    }

    #[test]
    fn neogeo_pocket_only_has_handheld_primary_path() {
        let taxonomy =
            LauncherTaxonomy::from_catalog(&catalog(Vec::new(), vec![system("neogeopocket", 3)]));
        let primary = taxonomy
            .primary_destination_for_system("neogeopocket")
            .expect("primary destination");
        assert_eq!(primary.menu_path, vec![ROOT_MENU_ID, HANDHELDS_MENU_ID]);
        assert!(!taxonomy.menu_contains_item(SNK_NEOGEO_MENU_ID, "neogeopocket"));
    }

    #[test]
    fn snk_neogeo_is_a_consoles_group_without_an_arcade_shortcut() {
        let taxonomy = LauncherTaxonomy::from_catalog(&catalog(
            vec![game("arcade", "P.O.W.", "SNK")],
            vec![
                system("arcade", 1),
                system("neogeo", 2),
                system("neogeo-cd", 3),
            ],
        ));

        assert!(!taxonomy.menu_contains_item(ROOT_MENU_ID, SNK_NEOGEO_MENU_ID));
        assert!(taxonomy.menu_contains_item(CONSOLES_MENU_ID, SNK_NEOGEO_MENU_ID));
        let snk = taxonomy.menu(SNK_NEOGEO_MENU_ID).expect("SNK NeoGeo");
        assert_eq!(snk.parent_id.as_deref(), Some(CONSOLES_MENU_ID));
        assert_eq!(
            snk.items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["neogeo", "neogeo-cd"]
        );
        assert_eq!(
            taxonomy
                .primary_destination_for_system("neogeo")
                .expect("NeoGeo destination")
                .menu_path,
            vec![ROOT_MENU_ID, CONSOLES_MENU_ID, SNK_NEOGEO_MENU_ID]
        );
    }

    #[test]
    fn single_collection_vendor_is_flattened_into_its_category() {
        let taxonomy = LauncherTaxonomy::from_catalog(&catalog(
            Vec::new(),
            vec![GameSystemEntry {
                id: "psx".to_string(),
                title: "PlayStation".to_string(),
                count: 7,
            }],
        ));
        let consoles = taxonomy.menu(CONSOLES_MENU_ID).expect("Consoles");
        assert_eq!(consoles.items.len(), 1);
        assert_eq!(consoles.items[0].id, "psx");
        assert_eq!(consoles.items[0].title, "Sony PlayStation");
        assert_eq!(consoles.items[0].kind, LauncherMenuItemKind::Collection);
        assert_eq!(
            taxonomy
                .primary_destination_for_system("psx")
                .expect("PlayStation destination")
                .menu_path,
            vec![ROOT_MENU_ID, CONSOLES_MENU_ID]
        );
    }

    #[test]
    fn amiga_cd32_is_grouped_with_commodore_computers() {
        let taxonomy = LauncherTaxonomy::from_catalog(&catalog(
            Vec::new(),
            vec![system("amiga", 4), system("amigacd32", 2)],
        ));

        assert!(taxonomy.menu_contains_item(COMPUTERS_MENU_ID, COMPUTERS_COMMODORE_MENU_ID));
        assert_eq!(
            taxonomy
                .menu(COMPUTERS_COMMODORE_MENU_ID)
                .expect("Commodore")
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["amiga", "amigacd32"]
        );
        assert_eq!(
            taxonomy
                .primary_destination_for_system("amigacd32")
                .expect("Amiga CD32 destination")
                .menu_path,
            vec![ROOT_MENU_ID, COMPUTERS_MENU_ID, COMPUTERS_COMMODORE_MENU_ID]
        );
    }

    #[test]
    fn separator_variants_share_one_system_destination() {
        let taxonomy =
            LauncherTaxonomy::from_catalog(&catalog(Vec::new(), vec![system("snk_neogeo", 2)]));
        assert_eq!(
            taxonomy.primary_destination_for_system("snk_neogeo"),
            taxonomy.primary_destination_for_system("snk-neogeo")
        );
    }

    #[test]
    fn unknown_system_falls_back_to_console_other_with_diagnostic() {
        let taxonomy =
            LauncherTaxonomy::from_catalog(&catalog(Vec::new(), vec![system("mystery-box", 2)]));
        assert!(taxonomy.menu_contains_item(CONSOLES_MENU_ID, "mystery-box"));
        assert!(taxonomy.menu(CONSOLES_OTHER_MENU_ID).is_none());
        assert_eq!(taxonomy.diagnostics().len(), 1);
    }

    #[test]
    fn every_canonical_system_has_a_single_taxonomy_driven_path() {
        for definition in system_definitions().expect("canonical system taxonomy") {
            let id = definition.id.as_str();
            let taxonomy =
                LauncherTaxonomy::from_catalog(&catalog(Vec::new(), vec![system(id, 1)]));
            let destination = taxonomy
                .primary_destination_for_system(id)
                .unwrap_or_else(|| panic!("missing primary destination for {id}"));
            let expected = match definition.section {
                LauncherSection::Arcade => vec![ROOT_MENU_ID],
                LauncherSection::SnkNeogeo => {
                    vec![ROOT_MENU_ID, CONSOLES_MENU_ID, SNK_NEOGEO_MENU_ID]
                }
                LauncherSection::Consoles => vec![ROOT_MENU_ID, CONSOLES_MENU_ID],
                LauncherSection::Handhelds => vec![ROOT_MENU_ID, HANDHELDS_MENU_ID],
                LauncherSection::Computers => vec![ROOT_MENU_ID, COMPUTERS_MENU_ID],
                LauncherSection::Other => vec![ROOT_MENU_ID, CONSOLES_MENU_ID],
            };
            assert_eq!(
                destination.menu_path, expected,
                "wrong primary path for {id}"
            );
        }
    }

    #[test]
    fn category_fallbacks_are_complete_and_unknown_is_diagnostic() {
        for (kind, parent, _vendor) in [
            (
                PlatformKind::Console,
                CONSOLES_MENU_ID,
                CONSOLES_OTHER_MENU_ID,
            ),
            (
                PlatformKind::Handheld,
                HANDHELDS_MENU_ID,
                HANDHELDS_OTHER_MENU_ID,
            ),
            (
                PlatformKind::Computer,
                COMPUTERS_MENU_ID,
                COMPUTERS_OTHER_MENU_ID,
            ),
            (
                PlatformKind::Unknown,
                CONSOLES_MENU_ID,
                CONSOLES_OTHER_MENU_ID,
            ),
        ] {
            let id = format!("new-{kind:?}").to_ascii_lowercase();
            let taxonomy = LauncherTaxonomy::from_catalog(&catalog_with_kind(&id, kind));
            let destination = taxonomy
                .primary_destination_for_system(&id)
                .expect("fallback destination");
            assert_eq!(destination.menu_path, vec![ROOT_MENU_ID, parent]);
            assert_eq!(
                taxonomy.diagnostics().len(),
                usize::from(kind == PlatformKind::Unknown)
            );
        }

        let taxonomy =
            LauncherTaxonomy::from_catalog(&catalog_with_kind("new-arcade", PlatformKind::Arcade));
        let destination = taxonomy
            .primary_destination_for_system("new-arcade")
            .expect("Arcade fallback destination");
        assert_eq!(destination.menu_path, vec![ROOT_MENU_ID]);
        assert_eq!(destination.collection_id, MENU_ARCADE_SYSTEM_ID);
    }

    #[test]
    fn flattened_vendor_items_keep_fixed_order_and_aggregate_counts_exclude_duplicates() {
        let mut snk_arcade = game("arcade", "Metal Slug", "SNK");
        snk_arcade.year = Some(1996);
        let snk_cps = game("cps1", "P.O.W.", "SNK (license)");
        let taxonomy = LauncherTaxonomy::from_catalog(&catalog(
            vec![snk_arcade, snk_cps, game("neogeo", "Last Resort", "SNK")],
            vec![
                system("arcade", 1),
                system("cps1", 1),
                system("neogeo", 1),
                system("atari2600", 1),
                system("sms", 1),
                system("psx", 1),
                system("nes", 1),
                system("tgfx16", 1),
                system("colecovision", 1),
                system("gb", 1),
                system("gamegear", 1),
                system("atarilynx", 1),
                system("neogeopocket", 1),
                system("wonderswan", 1),
                system("supervision", 1),
                system("acornatom", 1),
                system("apple-ii", 1),
                system("amiga", 1),
                system("atari800", 1),
                system("zx81", 1),
                system("trs-80", 1),
                system("dos", 1),
                system("msx", 1),
                system("amstrad", 1),
            ],
        ));

        let menu_ids = |menu_id| {
            taxonomy
                .menu(menu_id)
                .expect("menu")
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            menu_ids(CONSOLES_MENU_ID),
            vec![
                "atari2600",
                "sms",
                "psx",
                "nes",
                "tgfx16",
                SNK_NEOGEO_MENU_ID,
                "colecovision",
            ]
        );
        assert_eq!(
            menu_ids(HANDHELDS_MENU_ID),
            vec![
                "gb",
                "gamegear",
                "atarilynx",
                "neogeopocket",
                "wonderswan",
                "supervision",
            ]
        );
        assert_eq!(
            menu_ids(COMPUTERS_MENU_ID),
            vec![
                "acornatom",
                "apple-ii",
                "amiga",
                "atari800",
                "zx81",
                "trs-80",
                "dos",
                "msx",
                "amstrad",
            ]
        );
        assert_eq!(taxonomy.collection(MENU_ARCADE_SYSTEM_ID).unwrap().count, 2);
        assert_eq!(taxonomy.menu(SNK_NEOGEO_MENU_ID).unwrap().count, 1);
    }
}
