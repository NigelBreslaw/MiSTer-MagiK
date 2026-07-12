//! Stable, catalog-derived hierarchy used by the launcher home screen.

use crate::arcade_catalog::{
    ArcadeCatalog, GameSystemEntry, PlatformKind, MENU_ARCADE_SYSTEM_ID, MENU_SNK_ARCADE_SYSTEM_ID,
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

const CONSOLE_ATARI: &[&str] = &["atari2600", "atari5200", "atari7800", "jaguar"];
const CONSOLE_SEGA: &[&str] = &["sg1000", "sms", "megadrive", "megacd", "s32x", "saturn"];
const CONSOLE_SONY: &[&str] = &["psx"];
const CONSOLE_NINTENDO: &[&str] = &["nes", "fds", "snes", "satellaview", "n64"];
const CONSOLE_NEC: &[&str] = &["tgfx16", "tgfx16-cd", "supergrafx"];

const HANDHELD_NINTENDO: &[&str] = &[
    "gb",
    "gameboy",
    "gameboy2p",
    "gbc",
    "gba",
    "gba2p",
    "sgb",
    "sgb2",
    "pokemonmini",
];
const HANDHELD_SEGA: &[&str] = &["gamegear"];
const HANDHELD_ATARI: &[&str] = &["atarilynx"];
const HANDHELD_SNK: &[&str] = &["neogeopocket", "ngpc"];
const HANDHELD_BANDAI: &[&str] = &["wonderswan", "wonderswancolor"];

const COMPUTER_ACORN: &[&str] = &["acornatom", "acornelectron", "bbcmicro", "archie"];
const COMPUTER_APPLE: &[&str] = &["apple-ii", "macplus", "maclc"];
const COMPUTER_COMMODORE: &[&str] = &["amiga", "c64", "c128", "c16", "vic20", "pet2001"];
const COMPUTER_ATARI: &[&str] = &["atari800", "atarist"];
const COMPUTER_SINCLAIR: &[&str] = &["zx81", "zx-spectrum", "ql"];
const COMPUTER_TANDY: &[&str] = &["trs-80", "coco2", "coco3"];
const COMPUTER_DOS_PC: &[&str] = &["ao486", "dos"];
const COMPUTER_JAPANESE: &[&str] = &[
    "msx", "msx2", "pc88", "pc98", "x68000", "x1", "sharp-x1", "fm7", "fmtowns",
];

const NEOGEO_IDS: &[&str] = &["neogeo", "neo-geo", "snk-neo-geo"];
const NEOGEO_CD_IDS: &[&str] = &["neogeo-cd"];

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
        TaxonomyBuilder::new(catalog).build()
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

fn normalize_system_id(id: &str) -> String {
    id.trim().to_ascii_lowercase()
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
    taxonomy: LauncherTaxonomy,
    buckets: HashMap<Bucket, Vec<ClassifiedSystem>>,
    snk_systems: Vec<ClassifiedSystem>,
    arcade_summary_count: usize,
}

impl<'a> TaxonomyBuilder<'a> {
    fn new(catalog: &'a ArcadeCatalog) -> Self {
        Self {
            catalog,
            taxonomy: LauncherTaxonomy {
                token: LauncherTaxonomyToken::from_catalog(catalog),
                ..LauncherTaxonomy::default()
            },
            buckets: HashMap::new(),
            snk_systems: Vec::new(),
            arcade_summary_count: 0,
        }
    }

    fn build(mut self) -> LauncherTaxonomy {
        self.classify_catalog_systems();
        self.build_exact_collection_menus();
        self.build_category_menus();
        self.build_snk_menu();
        self.build_root_menu();
        self.record_collection_destinations();
        self.taxonomy
    }

    fn classify_catalog_systems(&mut self) {
        for system in &self.catalog.systems {
            let id = normalize_system_id(&system.id);
            let count = system.count.max(self.catalog.system_game_count(&system.id));
            if count == 0 {
                continue;
            }
            let kind = self.catalog.platform_kind(&system.id);
            let (bucket, rank) = classify(&id, kind);
            if bucket == Bucket::ArcadeAggregate {
                self.arcade_summary_count = self.arcade_summary_count.saturating_add(count);
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
        let snk_arcade_count = self.catalog.system_game_count(MENU_SNK_ARCADE_SYSTEM_ID);
        if snk_arcade_count > 0 {
            self.taxonomy.collections.insert(
                MENU_SNK_ARCADE_SYSTEM_ID.to_string(),
                LauncherCollection {
                    id: MENU_SNK_ARCADE_SYSTEM_ID.to_string(),
                    title: "Arcade".to_string(),
                    count: snk_arcade_count,
                    system_id: None,
                    legacy_system_id: "arcade".to_string(),
                },
            );
            items.push(LauncherMenuItem {
                id: MENU_SNK_ARCADE_SYSTEM_ID.to_string(),
                title: "Arcade".to_string(),
                count: snk_arcade_count,
                kind: LauncherMenuItemKind::Collection,
            });
        }

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
                    menu_path: vec![ROOT_MENU_ID.to_string(), SNK_NEOGEO_MENU_ID.to_string()],
                    collection_id: system.entry.id.clone(),
                },
            );
        }

        // NeoGeo Pocket is intentionally reachable both through the dedicated
        // SNK menu and its primary Handhelds / SNK path.
        if let Some(handheld_snk) = self.taxonomy.menu(HANDHELDS_SNK_MENU_ID) {
            for item in &handheld_snk.items {
                if HANDHELD_SNK.contains(&normalize_system_id(&item.id).as_str()) {
                    items.push(item.clone());
                }
            }
        }
        self.insert_menu(SNK_NEOGEO_MENU_ID, "SNK NeoGeo", Some(ROOT_MENU_ID), items);
    }

    fn build_root_menu(&mut self) {
        let arcade_count = self
            .catalog
            .system_game_count(MENU_ARCADE_SYSTEM_ID)
            .max(self.arcade_summary_count);
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
        for menu_id in [
            SNK_NEOGEO_MENU_ID,
            CONSOLES_MENU_ID,
            HANDHELDS_MENU_ID,
            COMPUTERS_MENU_ID,
        ] {
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
        for (collection_id, menu_path) in [
            (MENU_ARCADE_SYSTEM_ID, vec![ROOT_MENU_ID.to_string()]),
            (
                MENU_SNK_ARCADE_SYSTEM_ID,
                vec![ROOT_MENU_ID.to_string(), SNK_NEOGEO_MENU_ID.to_string()],
            ),
        ] {
            if self.taxonomy.collection(collection_id).is_some() {
                self.taxonomy.primary_collection_destinations.insert(
                    collection_id.to_string(),
                    LauncherDestination {
                        menu_path,
                        collection_id: collection_id.to_string(),
                    },
                );
            }
        }
    }

    fn insert_parent_menu(&mut self, id: &str, title: &str, child_ids: &[&str]) {
        let items = child_ids
            .iter()
            .filter_map(|child_id| self.taxonomy.menu(child_id))
            .map(|menu| LauncherMenuItem {
                id: menu.id.clone(),
                title: menu.title.clone(),
                count: menu.count,
                kind: LauncherMenuItemKind::Menu,
            })
            .collect();
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
    if let Some(rank) = rank_in(id, NEOGEO_IDS) {
        return (Bucket::SnkOnly, rank);
    }
    if let Some(rank) = rank_in(id, NEOGEO_CD_IDS) {
        return (Bucket::SnkOnly, NEOGEO_IDS.len() + rank);
    }
    for (ids, bucket) in [
        (CONSOLE_ATARI, Bucket::ConsoleAtari),
        (CONSOLE_SEGA, Bucket::ConsoleSega),
        (CONSOLE_SONY, Bucket::ConsoleSony),
        (CONSOLE_NINTENDO, Bucket::ConsoleNintendo),
        (CONSOLE_NEC, Bucket::ConsoleNec),
        (HANDHELD_NINTENDO, Bucket::HandheldNintendo),
        (HANDHELD_SEGA, Bucket::HandheldSega),
        (HANDHELD_ATARI, Bucket::HandheldAtari),
        (HANDHELD_SNK, Bucket::HandheldSnk),
        (HANDHELD_BANDAI, Bucket::HandheldBandai),
        (COMPUTER_ACORN, Bucket::ComputerAcorn),
        (COMPUTER_APPLE, Bucket::ComputerApple),
        (COMPUTER_COMMODORE, Bucket::ComputerCommodore),
        (COMPUTER_ATARI, Bucket::ComputerAtari),
        (COMPUTER_SINCLAIR, Bucket::ComputerSinclair),
        (COMPUTER_TANDY, Bucket::ComputerTandy),
        (COMPUTER_DOS_PC, Bucket::ComputerDosPc),
        (COMPUTER_JAPANESE, Bucket::ComputerJapanese),
    ] {
        if let Some(rank) = rank_in(id, ids) {
            return (bucket, rank);
        }
    }
    match kind {
        PlatformKind::Arcade => (Bucket::ArcadeAggregate, usize::MAX),
        PlatformKind::Console => (Bucket::ConsoleOther, usize::MAX),
        PlatformKind::Handheld => (Bucket::HandheldOther, usize::MAX),
        PlatformKind::Computer => (Bucket::ComputerOther, usize::MAX),
        PlatformKind::Unknown => (Bucket::ConsoleOther, usize::MAX),
    }
}

fn rank_in(id: &str, ids: &[&str]) -> Option<usize> {
    ids.iter().position(|candidate| *candidate == id)
}

fn snk_title(id: &str, fallback: &str) -> String {
    match normalize_system_id(id).as_str() {
        "neogeo" | "neo-geo" | "snk-neo-geo" => "NeoGeo".to_string(),
        "neogeo-cd" => "NeoGeo CD".to_string(),
        "neogeopocket" | "ngpc" => "NeoGeo Pocket".to_string(),
        _ => fallback.to_string(),
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
    fn root_uses_fixed_five_item_order_when_every_branch_is_populated() {
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
                SNK_NEOGEO_MENU_ID,
                CONSOLES_MENU_ID,
                HANDHELDS_MENU_ID,
                COMPUTERS_MENU_ID,
            ]
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
    fn neogeo_pocket_has_handheld_primary_path_and_snk_shortcut() {
        let taxonomy =
            LauncherTaxonomy::from_catalog(&catalog(Vec::new(), vec![system("neogeopocket", 3)]));
        let primary = taxonomy
            .primary_destination_for_system("neogeopocket")
            .expect("primary destination");
        assert_eq!(
            primary.menu_path,
            vec![ROOT_MENU_ID, HANDHELDS_MENU_ID, HANDHELDS_SNK_MENU_ID]
        );
        assert!(taxonomy.menu_contains_item(SNK_NEOGEO_MENU_ID, "neogeopocket"));
    }

    #[test]
    fn snk_arcade_collection_uses_whole_token_matcher_from_catalog() {
        let catalog = catalog(
            vec![
                game("arcade", "One", "SNK (Rock-Ola license)"),
                game("arcade", "Two", "FunSNKWorks"),
            ],
            vec![system("arcade", 2)],
        );
        let taxonomy = LauncherTaxonomy::from_catalog(&catalog);
        assert_eq!(
            taxonomy
                .collection(MENU_SNK_ARCADE_SYSTEM_ID)
                .expect("SNK Arcade")
                .count,
            1
        );
    }

    #[test]
    fn unknown_system_falls_back_to_console_other_with_diagnostic() {
        let taxonomy =
            LauncherTaxonomy::from_catalog(&catalog(Vec::new(), vec![system("mystery-box", 2)]));
        assert!(taxonomy.menu_contains_item(CONSOLES_OTHER_MENU_ID, "mystery-box"));
        assert_eq!(taxonomy.diagnostics().len(), 1);
    }

    #[test]
    fn every_curated_system_id_has_its_approved_primary_vendor_path() {
        for (ids, parent, vendor) in [
            (CONSOLE_ATARI, CONSOLES_MENU_ID, CONSOLES_ATARI_MENU_ID),
            (CONSOLE_SEGA, CONSOLES_MENU_ID, CONSOLES_SEGA_MENU_ID),
            (CONSOLE_SONY, CONSOLES_MENU_ID, CONSOLES_SONY_MENU_ID),
            (
                CONSOLE_NINTENDO,
                CONSOLES_MENU_ID,
                CONSOLES_NINTENDO_MENU_ID,
            ),
            (CONSOLE_NEC, CONSOLES_MENU_ID, CONSOLES_NEC_MENU_ID),
            (
                HANDHELD_NINTENDO,
                HANDHELDS_MENU_ID,
                HANDHELDS_NINTENDO_MENU_ID,
            ),
            (HANDHELD_SEGA, HANDHELDS_MENU_ID, HANDHELDS_SEGA_MENU_ID),
            (HANDHELD_ATARI, HANDHELDS_MENU_ID, HANDHELDS_ATARI_MENU_ID),
            (HANDHELD_SNK, HANDHELDS_MENU_ID, HANDHELDS_SNK_MENU_ID),
            (HANDHELD_BANDAI, HANDHELDS_MENU_ID, HANDHELDS_BANDAI_MENU_ID),
            (COMPUTER_ACORN, COMPUTERS_MENU_ID, COMPUTERS_ACORN_MENU_ID),
            (COMPUTER_APPLE, COMPUTERS_MENU_ID, COMPUTERS_APPLE_MENU_ID),
            (
                COMPUTER_COMMODORE,
                COMPUTERS_MENU_ID,
                COMPUTERS_COMMODORE_MENU_ID,
            ),
            (COMPUTER_ATARI, COMPUTERS_MENU_ID, COMPUTERS_ATARI_MENU_ID),
            (
                COMPUTER_SINCLAIR,
                COMPUTERS_MENU_ID,
                COMPUTERS_SINCLAIR_MENU_ID,
            ),
            (COMPUTER_TANDY, COMPUTERS_MENU_ID, COMPUTERS_TANDY_MENU_ID),
            (COMPUTER_DOS_PC, COMPUTERS_MENU_ID, COMPUTERS_DOS_PC_MENU_ID),
            (
                COMPUTER_JAPANESE,
                COMPUTERS_MENU_ID,
                COMPUTERS_JAPANESE_MENU_ID,
            ),
        ] {
            for id in ids {
                let taxonomy =
                    LauncherTaxonomy::from_catalog(&catalog(Vec::new(), vec![system(id, 1)]));
                let destination = taxonomy
                    .primary_destination_for_system(id)
                    .unwrap_or_else(|| panic!("missing primary destination for {id}"));
                assert_eq!(
                    destination.menu_path,
                    vec![ROOT_MENU_ID, parent, vendor],
                    "wrong primary path for {id}"
                );
                assert_eq!(destination.collection_id, *id);
            }
        }

        for id in NEOGEO_IDS.iter().chain(NEOGEO_CD_IDS) {
            let taxonomy =
                LauncherTaxonomy::from_catalog(&catalog(Vec::new(), vec![system(id, 1)]));
            let destination = taxonomy
                .primary_destination_for_system(id)
                .expect("SNK primary destination");
            assert_eq!(
                destination.menu_path,
                vec![ROOT_MENU_ID, SNK_NEOGEO_MENU_ID]
            );
        }
    }

    #[test]
    fn category_fallbacks_are_complete_and_unknown_is_diagnostic() {
        for (kind, parent, vendor) in [
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
            assert_eq!(destination.menu_path, vec![ROOT_MENU_ID, parent, vendor]);
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
    fn vendor_groups_keep_fixed_order_and_aggregate_counts_include_overlaps() {
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
                CONSOLES_ATARI_MENU_ID,
                CONSOLES_SEGA_MENU_ID,
                CONSOLES_SONY_MENU_ID,
                CONSOLES_NINTENDO_MENU_ID,
                CONSOLES_NEC_MENU_ID,
                CONSOLES_OTHER_MENU_ID,
            ]
        );
        assert_eq!(
            menu_ids(HANDHELDS_MENU_ID),
            vec![
                HANDHELDS_NINTENDO_MENU_ID,
                HANDHELDS_SEGA_MENU_ID,
                HANDHELDS_ATARI_MENU_ID,
                HANDHELDS_SNK_MENU_ID,
                HANDHELDS_BANDAI_MENU_ID,
                HANDHELDS_OTHER_MENU_ID,
            ]
        );
        assert_eq!(
            menu_ids(COMPUTERS_MENU_ID),
            vec![
                COMPUTERS_ACORN_MENU_ID,
                COMPUTERS_APPLE_MENU_ID,
                COMPUTERS_COMMODORE_MENU_ID,
                COMPUTERS_ATARI_MENU_ID,
                COMPUTERS_SINCLAIR_MENU_ID,
                COMPUTERS_TANDY_MENU_ID,
                COMPUTERS_DOS_PC_MENU_ID,
                COMPUTERS_JAPANESE_MENU_ID,
                COMPUTERS_OTHER_MENU_ID,
            ]
        );
        assert_eq!(taxonomy.collection(MENU_ARCADE_SYSTEM_ID).unwrap().count, 2);
        assert_eq!(
            taxonomy
                .collection(MENU_SNK_ARCADE_SYSTEM_ID)
                .unwrap()
                .count,
            2
        );
        assert_eq!(taxonomy.menu(SNK_NEOGEO_MENU_ID).unwrap().count, 4);
    }
}
