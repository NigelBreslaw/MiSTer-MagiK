//! Arcade catalog helpers.
//!
//! The runtime launcher catalog is SQLite-backed. This module keeps the shared
//! in-memory catalog types and presentation helpers used by the SQLite loader.

use crate::launch_profiles::{self, MountKind, PayloadRule};
use crate::catalog_navigation::CatalogNavigationProjection;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

pub const DEFAULT_ARCADE_ROOT: &str = "/media/fat/_Arcade";

/// Logical row height for the Rust-painted arcade list viewport.
pub const ARCADE_ROW_HEIGHT: i32 = 48;
/// Visible list height: 8 exact arcade rows (matches `arcade_list.slint` left pane).
pub const ARCADE_LIST_VISIBLE_H: i32 = 384;
pub const HOME_TILE_WIDTH: i32 = 220;
pub const HOME_TILE_GAP: i32 = 16;
pub const HOME_LIST_VISIBLE_W: i32 = 912;

#[derive(Clone, Debug)]
pub struct ArcadeGameEntry {
    pub title: Arc<str>,
    pub mra_path: Arc<str>,
    pub preview_archive_path: Arc<str>,
    pub preview_asset_key: Arc<str>,
    pub has_preview: bool,
    pub system_id: Arc<str>,
    pub year: Option<u16>,
    pub manufacturer: Arc<str>,
    pub category: Arc<str>,
    pub is_new: bool,
}

impl ArcadeGameEntry {
    pub fn metadata_key(&self) -> ArcadeGameMetadataKey {
        ArcadeGameMetadataKey {
            year: self.year,
            manufacturer: self.manufacturer.to_string(),
            category: self.category.to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArcadeGameMetadataKey {
    pub year: Option<u16>,
    pub manufacturer: String,
    pub category: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameSystemEntry {
    pub id: String,
    pub title: String,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ArcadeFilter {
    All,
    Decade(u16),
    Manufacturer(String),
    Category(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArcadeFilterOption {
    pub label: String,
    pub count: usize,
}

#[derive(Clone, Debug)]
pub struct ArcadeCatalog {
    pub root: PathBuf,
    pub games: Vec<ArcadeGameEntry>,
    pub systems: Vec<GameSystemEntry>,
    games_by_system: HashMap<String, Vec<ArcadeGameEntry>>,
    games_by_filter: HashMap<String, Vec<ArcadeGameEntry>>,
    filter_options_by_system: HashMap<String, ArcadeSystemFilterOptions>,
    preview_games_by_system: HashMap<String, Vec<ArcadeGameEntry>>,
    games_by_ref: HashMap<Arc<str>, usize>,
    launch_plans_by_ref: HashMap<Arc<str>, StructuredLaunchPlan>,
}

#[derive(Clone, Debug, Default)]
struct ArcadeSystemFilterOptions {
    decades: Vec<ArcadeFilterOption>,
    manufacturers: Vec<ArcadeFilterOption>,
    categories: Vec<ArcadeFilterOption>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructuredLaunchPlan {
    pub launch_ref: Arc<str>,
    pub title: Arc<str>,
    pub system_id: Arc<str>,
    pub core_path: Arc<str>,
    pub payload_path: Arc<str>,
    pub mount_kind: Arc<str>,
    pub mount_index: u8,
    pub delay_secs: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchTarget {
    Path(Arc<str>),
    Structured(StructuredLaunchPlan),
    MissingStructured(Arc<str>),
}

impl ArcadeCatalog {
    pub fn new(root: PathBuf, games: Vec<ArcadeGameEntry>, systems: Vec<GameSystemEntry>) -> Self {
        Self::new_with_launch_plans(root, games, systems, Vec::new())
    }

    pub fn new_with_launch_plans(
        root: PathBuf,
        games: Vec<ArcadeGameEntry>,
        systems: Vec<GameSystemEntry>,
        launch_plans: Vec<StructuredLaunchPlan>,
    ) -> Self {
        let games_by_system = games_by_system(&games);
        let games_by_filter = games_by_filter(&games);
        let filter_options_by_system = filter_options_by_system(&games);
        let preview_games_by_system = preview_games_by_system(&games);
        let games_by_ref: HashMap<Arc<str>, usize> = games
            .iter()
            .enumerate()
            .map(|(idx, game)| (game.mra_path.clone(), idx))
            .collect();
        let launch_plans_by_ref: HashMap<Arc<str>, StructuredLaunchPlan> = launch_plans
            .into_iter()
            .map(|plan| (plan.launch_ref.clone(), plan))
            .collect();
        Self {
            root,
            games,
            systems,
            games_by_system,
            games_by_filter,
            filter_options_by_system,
            preview_games_by_system,
            games_by_ref,
            launch_plans_by_ref,
        }
    }

    pub fn from_navigation_projection(
        root: impl Into<PathBuf>,
        projection: CatalogNavigationProjection,
    ) -> Self {
        let games = projection.games.into_iter().map(ArcadeGameEntry::from).collect();
        let systems = projection
            .systems
            .into_iter()
            .map(GameSystemEntry::from)
            .collect();
        let launch_plans = projection
            .launch_plans
            .into_iter()
            .map(StructuredLaunchPlan::from)
            .collect();
        Self::new_with_launch_plans(root.into(), games, systems, launch_plans)
    }

    pub fn len(&self) -> usize {
        self.games.len()
    }

    pub fn is_empty(&self) -> bool {
        self.games.is_empty()
    }

    pub fn title_for_path(&self, mra_path: &str) -> &str {
        self.games
            .iter()
            .find(|g| g.mra_path.as_ref() == mra_path)
            .map(|g| g.title.as_ref())
            .unwrap_or("Game")
    }

    pub fn launch_target_for_ref(&self, launch_ref: &str) -> LaunchTarget {
        self.launch_plans_by_ref
            .get(launch_ref)
            .cloned()
            .map(LaunchTarget::Structured)
            .unwrap_or_else(|| {
                if launch_ref.starts_with("magik-plan:") {
                    self.games_by_ref
                        .get(launch_ref)
                        .and_then(|idx| self.games.get(*idx))
                        .and_then(|game| derive_structured_launch_plan(game, profiles_by_system()))
                        .map(LaunchTarget::Structured)
                        .unwrap_or_else(|| LaunchTarget::MissingStructured(Arc::from(launch_ref)))
                } else {
                    LaunchTarget::Path(Arc::from(launch_ref))
                }
            })
    }

    pub fn system_games(&self, system_id: &str) -> Vec<ArcadeGameEntry> {
        self.system_game_slice(system_id).to_vec()
    }

    pub fn system_game_count(&self, system_id: &str) -> usize {
        self.system_game_slice(system_id).len()
    }

    pub fn system_game_at(&self, system_id: &str, index: usize) -> Option<&ArcadeGameEntry> {
        self.system_game_slice(system_id).get(index)
    }

    pub fn system_game_slice(&self, system_id: &str) -> &[ArcadeGameEntry] {
        self.games_by_system
            .get(system_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn filtered_game_count(&self, system_id: &str, filter: &ArcadeFilter) -> usize {
        self.filtered_game_slice(system_id, filter).len()
    }

    pub fn filtered_game_at(
        &self,
        system_id: &str,
        filter: &ArcadeFilter,
        index: usize,
    ) -> Option<&ArcadeGameEntry> {
        self.filtered_game_slice(system_id, filter).get(index)
    }

    pub fn filtered_game_slice(
        &self,
        system_id: &str,
        filter: &ArcadeFilter,
    ) -> &[ArcadeGameEntry] {
        match filter {
            ArcadeFilter::All => self.system_game_slice(system_id),
            ArcadeFilter::Decade(decade) => self
                .games_by_filter
                .get(&filter_key(system_id, "decade", &decade.to_string()))
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            ArcadeFilter::Manufacturer(manufacturer) => self
                .games_by_filter
                .get(&filter_key(system_id, "manufacturer", manufacturer))
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            ArcadeFilter::Category(category) => self
                .games_by_filter
                .get(&filter_key(system_id, "category", category))
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        }
    }

    pub fn decade_options(&self, system_id: &str) -> Vec<ArcadeFilterOption> {
        self.filter_options(system_id).decades.clone()
    }

    pub fn decade_option_count(&self, system_id: &str) -> usize {
        self.filter_options(system_id).decades.len()
    }

    pub fn manufacturer_options(&self, system_id: &str) -> Vec<ArcadeFilterOption> {
        self.filter_options(system_id).manufacturers.clone()
    }

    pub fn manufacturer_option_count(&self, system_id: &str) -> usize {
        self.filter_options(system_id).manufacturers.len()
    }

    pub fn category_options(&self, system_id: &str) -> Vec<ArcadeFilterOption> {
        self.filter_options(system_id).categories.clone()
    }

    pub fn category_option_count(&self, system_id: &str) -> usize {
        self.filter_options(system_id).categories.len()
    }

    pub fn system_preview_games(&self, system_id: &str) -> Vec<ArcadeGameEntry> {
        self.system_preview_game_slice(system_id).to_vec()
    }

    pub fn system_preview_game_count(&self, system_id: &str) -> usize {
        self.system_preview_game_slice(system_id).len()
    }

    pub fn system_preview_game_at(&self, system_id: &str, index: usize) -> Option<ArcadeGameEntry> {
        self.system_preview_game_slice(system_id)
            .get(index)
            .cloned()
    }

    pub fn system_preview_game_slice(&self, system_id: &str) -> &[ArcadeGameEntry] {
        self.preview_games_by_system
            .get(system_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn filter_options(&self, system_id: &str) -> &ArcadeSystemFilterOptions {
        static EMPTY: OnceLock<ArcadeSystemFilterOptions> = OnceLock::new();
        self.filter_options_by_system
            .get(system_id)
            .unwrap_or_else(|| EMPTY.get_or_init(ArcadeSystemFilterOptions::default))
    }
}

fn derive_structured_launch_plan(
    game: &ArcadeGameEntry,
    profiles_by_system: &HashMap<&'static str, launch_profiles::LaunchProfile>,
) -> Option<StructuredLaunchPlan> {
    let encoded_payload = game.mra_path.strip_prefix("magik-plan:")?;
    let (archive_entry, payload_path) = encoded_payload
        .strip_prefix("archive:")
        .map(|payload| (true, payload))
        .unwrap_or_else(|| {
            encoded_payload
                .strip_prefix("payload:")
                .map(|payload| (false, payload))
                .unwrap_or((false, encoded_payload))
        });
    let profile = profiles_by_system.get(game.system_id.as_ref())?;
    let core_path = profile.core_path?;
    let payload_rule = if archive_entry {
        profile.classify_archive_entry(Path::new(payload_path))
    } else {
        payload_rule_for_path(profile, payload_path)
    }?;
    Some(StructuredLaunchPlan {
        launch_ref: game.mra_path.clone(),
        title: game.title.clone(),
        system_id: game.system_id.clone(),
        core_path: core_path.into(),
        payload_path: payload_path.into(),
        mount_kind: mount_kind_label(payload_rule.mount.kind).into(),
        mount_index: payload_rule.mount.index,
        delay_secs: payload_rule.mount.delay_secs,
    })
}

fn profiles_by_system() -> &'static HashMap<&'static str, launch_profiles::LaunchProfile> {
    static PROFILES_BY_SYSTEM: OnceLock<HashMap<&'static str, launch_profiles::LaunchProfile>> =
        OnceLock::new();
    PROFILES_BY_SYSTEM.get_or_init(|| {
        launch_profiles::builtin_profiles()
            .into_iter()
            .map(|profile| (profile.system_id, profile))
            .collect()
    })
}

fn payload_rule_for_path(
    profile: &launch_profiles::LaunchProfile,
    payload_path: &str,
) -> Option<PayloadRule> {
    match profile.classify_path(Path::new(payload_path)) {
        launch_profiles::ProfilePathClass::Payload { rule } => Some(rule),
        _ => None,
    }
}

fn mount_kind_label(kind: MountKind) -> &'static str {
    match kind {
        MountKind::Launcher => "launcher",
        MountKind::LoadFile => "load-file",
        MountKind::MountImage => "mount-image",
        MountKind::Core => "core",
    }
}

fn games_by_system(games: &[ArcadeGameEntry]) -> HashMap<String, Vec<ArcadeGameEntry>> {
    let mut by_system: HashMap<String, Vec<ArcadeGameEntry>> = HashMap::new();
    for game in games {
        by_system
            .entry(game.system_id.to_string())
            .or_default()
            .push(game.clone());
    }
    by_system
}

fn games_by_filter(games: &[ArcadeGameEntry]) -> HashMap<String, Vec<ArcadeGameEntry>> {
    let mut by_filter: HashMap<String, Vec<ArcadeGameEntry>> = HashMap::new();
    for game in games {
        let system_id = game.system_id.as_ref();
        if let Some(year) = game.year {
            let decade = (year / 10) * 10;
            by_filter
                .entry(filter_key(system_id, "decade", &decade.to_string()))
                .or_default()
                .push(game.clone());
        }
        if !game.manufacturer.is_empty() {
            by_filter
                .entry(filter_key(system_id, "manufacturer", &game.manufacturer))
                .or_default()
                .push(game.clone());
        }
        if !game.category.is_empty() {
            by_filter
                .entry(filter_key(system_id, "category", &game.category))
                .or_default()
                .push(game.clone());
        }
    }
    by_filter
}

fn filter_key(system_id: &str, kind: &str, value: &str) -> String {
    format!("{system_id}\n{kind}\n{value}")
}

#[derive(Default)]
struct FilterOptionCounts {
    decades: BTreeMap<u16, usize>,
    manufacturers: BTreeMap<String, usize>,
    categories: BTreeMap<String, usize>,
}

fn filter_options_by_system(
    games: &[ArcadeGameEntry],
) -> HashMap<String, ArcadeSystemFilterOptions> {
    let mut counts_by_system = HashMap::<String, FilterOptionCounts>::new();
    for game in games {
        let counts = counts_by_system
            .entry(game.system_id.to_string())
            .or_default();
        if let Some(year) = game.year {
            let decade = (year / 10) * 10;
            *counts.decades.entry(decade).or_default() += 1;
        }
        let manufacturer = game.manufacturer.trim();
        if !manufacturer.is_empty() {
            *counts
                .manufacturers
                .entry(manufacturer.to_string())
                .or_default() += 1;
        }
        let category = game.category.trim();
        if !category.is_empty() {
            *counts.categories.entry(category.to_string()).or_default() += 1;
        }
    }
    counts_by_system
        .into_iter()
        .map(|(system_id, counts)| {
            (
                system_id,
                ArcadeSystemFilterOptions {
                    decades: counts
                        .decades
                        .into_iter()
                        .map(|(decade, count)| ArcadeFilterOption {
                            label: format!("{decade}'s"),
                            count,
                        })
                        .collect(),
                    manufacturers: string_filter_options_from_counts(counts.manufacturers),
                    categories: string_filter_options_from_counts(counts.categories),
                },
            )
        })
        .collect()
}

fn string_filter_options_from_counts(counts: BTreeMap<String, usize>) -> Vec<ArcadeFilterOption> {
    counts
        .into_iter()
        .map(|(label, count)| ArcadeFilterOption { label, count })
        .collect()
}

fn preview_games_by_system(games: &[ArcadeGameEntry]) -> HashMap<String, Vec<ArcadeGameEntry>> {
    let mut by_system: HashMap<String, Vec<&ArcadeGameEntry>> = HashMap::new();
    for game in games {
        by_system
            .entry(game.system_id.to_string())
            .or_default()
            .push(game);
    }
    by_system
        .into_iter()
        .map(|(system_id, games)| (system_id, preview_games(games.into_iter())))
        .collect()
}

fn preview_games<'a>(games: impl Iterator<Item = &'a ArcadeGameEntry>) -> Vec<ArcadeGameEntry> {
    let mut best_idx: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<ArcadeGameEntry> = Vec::new();

    for game in games {
        if !has_preview_image(game) {
            continue;
        }
        let key = preview_dedupe_key(&game.title);
        if let Some(&idx) = best_idx.get(&key) {
            if prefer_preview_game(game, &out[idx]) {
                out[idx] = game.clone();
            }
        } else {
            best_idx.insert(key, out.len());
            out.push(game.clone());
        }
    }

    out
}

fn preview_dedupe_key(title: &str) -> String {
    let base = title
        .split_once('(')
        .map(|(before, _)| before.trim())
        .filter(|before| !before.is_empty())
        .unwrap_or(title);
    base.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn prefer_preview_game(a: &ArcadeGameEntry, b: &ArcadeGameEntry) -> bool {
    let a_exact = !a.title.contains('(');
    let b_exact = !b.title.contains('(');
    if a_exact != b_exact {
        return a_exact;
    }
    if a.title.len() != b.title.len() {
        return a.title.len() < b.title.len();
    }
    a.mra_path < b.mra_path
}

fn has_preview_image(game: &ArcadeGameEntry) -> bool {
    game.has_preview && !game.preview_archive_path.is_empty() && !game.preview_asset_key.is_empty()
}

pub fn systems_from_games(games: &[ArcadeGameEntry]) -> Vec<GameSystemEntry> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for game in games {
        *counts.entry(game.system_id.to_string()).or_default() += 1;
    }
    let mut systems: Vec<GameSystemEntry> = counts
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(id, count)| GameSystemEntry {
            title: system_title(&id),
            id,
            count,
        })
        .collect();
    systems.sort_by_cached_key(system_sort_key);
    systems
}

fn system_sort_key(system: &GameSystemEntry) -> String {
    let rank = match system.id.as_str() {
        "arcade" => 0,
        "amiga" => 1,
        "neogeo" => 2,
        "nes" => 3,
        "snes" => 4,
        "saturn" => 5,
        "megadrive" => 6,
        "gba" => 7,
        "gbc" => 8,
        "n64" => 9,
        "gamegear" => 10,
        "vectrex" => 11,
        "ao486" => 12,
        "dos" => 13,
        "unknown" => 999,
        _ => 100,
    };
    format!("{rank:03}-{}", system.title.to_lowercase())
}

pub fn system_title(id: &str) -> String {
    match id {
        "arcade" => "Arcade".to_string(),
        "neogeo" | "neo-geo" | "snk-neo-geo" => "NeoGeo".to_string(),
        "cps1" | "capcom-cps1" => "CPS1".to_string(),
        "cps2" | "capcom-cps2" => "CPS2".to_string(),
        "cps3" | "capcom-cps3" => "CPS3".to_string(),
        "system16" | "sega-system16" => "System 16".to_string(),
        "system18" | "sega-system18" => "System 18".to_string(),
        "m72" | "irem-m72" => "Irem M72".to_string(),
        "m92" | "irem-m92" => "Irem M92".to_string(),
        "gba" => "GBA".to_string(),
        "gbc" => "GBC".to_string(),
        "gb" => "GB".to_string(),
        "nes" => "NES".to_string(),
        "snes" => "SNES".to_string(),
        "n64" => "N64".to_string(),
        "sms" => "SMS".to_string(),
        "psx" => "PSX".to_string(),
        "ao486" => "ao486".to_string(),
        "dos" => "DOS Games".to_string(),
        "megadrive" => "Mega Drive".to_string(),
        "megacd" => "Mega CD".to_string(),
        "gamegear" => "Game Gear".to_string(),
        "unknown" => "Unknown".to_string(),
        other => other
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(
        title: &str,
        mra_path: &str,
        preview_asset_key: &str,
        system_id: &str,
    ) -> ArcadeGameEntry {
        let has_preview = !preview_asset_key.is_empty();
        ArcadeGameEntry {
            title: title.into(),
            mra_path: mra_path.into(),
            preview_archive_path: if has_preview {
                "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b".into()
            } else {
                "".into()
            },
            preview_asset_key: preview_asset_key.into(),
            has_preview,
            system_id: system_id.into(),
            year: None,
            manufacturer: "".into(),
            category: "".into(),
            is_new: false,
        }
    }

    #[test]
    fn preview_games_require_images_and_collapse_parenthetical_clones() {
        let root = PathBuf::from("/media/fat/_Arcade");
        let systems = vec![GameSystemEntry {
            id: "arcade".into(),
            title: "Arcade".into(),
            count: 5,
        }];
        let games = vec![
            ArcadeGameEntry {
                title: "1941: Counter Attack (Japan)".into(),
                mra_path: "/media/fat/_Arcade/1941 Japan.mra".into(),
                preview_archive_path: "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b"
                    .into(),
                preview_asset_key: "1941u".into(),
                has_preview: true,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                is_new: false,
            },
            ArcadeGameEntry {
                title: "1941: Counter Attack (World)".into(),
                mra_path: "/media/fat/_Arcade/1941 World.mra".into(),
                preview_archive_path: "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b"
                    .into(),
                preview_asset_key: "1941u".into(),
                has_preview: true,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                is_new: false,
            },
            ArcadeGameEntry {
                title: "1942".into(),
                mra_path: "/media/fat/_Arcade/1942.mra".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                is_new: false,
            },
            ArcadeGameEntry {
                title: "1943".into(),
                mra_path: "/media/fat/_Arcade/1943.mra".into(),
                preview_archive_path: "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b"
                    .into(),
                preview_asset_key: "1943".into(),
                has_preview: true,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                is_new: false,
            },
            ArcadeGameEntry {
                title: "Astra SuperStars".into(),
                mra_path: "/media/fat/_Arcade/Astra SuperStars.mra".into(),
                preview_archive_path: "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b"
                    .into(),
                preview_asset_key: "astrass".into(),
                has_preview: true,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                is_new: false,
            },
        ];
        let catalog = ArcadeCatalog::new(root, games, systems);

        let games = catalog.system_preview_games("arcade");
        assert_eq!(games.len(), 3);
        assert_eq!(catalog.system_preview_game_count("arcade"), 3);
        assert_eq!(games[0].title.as_ref(), "1941: Counter Attack (Japan)");
        assert_eq!(games[1].title.as_ref(), "1943");
        assert_eq!(games[2].title.as_ref(), "Astra SuperStars");
        assert_eq!(
            catalog
                .system_preview_game_at("arcade", 1)
                .map(|game| game.title.to_string()),
            Some("1943".to_string())
        );
    }

    #[test]
    fn system_game_count_includes_games_without_preview_assets() {
        let root = PathBuf::from("/media/fat/_Arcade");
        let systems = vec![GameSystemEntry {
            id: "amiga".into(),
            title: "Amiga".into(),
            count: 1,
        }];
        let games = vec![ArcadeGameEntry {
            title: "Agony".into(),
            mra_path: "magik-plan:amiga-agony".into(),
            preview_archive_path: "".into(),
            preview_asset_key: "".into(),
            has_preview: false,
            system_id: "amiga".into(),
            year: None,
            manufacturer: "".into(),
            category: "".into(),
            is_new: false,
        }];
        let catalog = ArcadeCatalog::new(root, games, systems);

        assert_eq!(catalog.system_game_count("amiga"), 1);
        assert_eq!(catalog.system_game_slice("amiga").len(), 1);
        assert_eq!(catalog.system_preview_game_count("amiga"), 0);
    }

    #[test]
    fn catalog_lookup_falls_back_cleanly_for_missing_paths_and_systems() {
        let catalog = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            vec![ArcadeGameEntry {
                title: "1942".into(),
                mra_path: "/media/fat/_Arcade/1942.mra".into(),
                preview_archive_path: "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b"
                    .into(),
                preview_asset_key: "1942".into(),
                has_preview: true,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                is_new: false,
            }],
            vec![GameSystemEntry {
                id: "arcade".into(),
                title: "Arcade".into(),
                count: 1,
            }],
        );

        assert_eq!(catalog.len(), 1);
        assert!(!catalog.is_empty());
        assert_eq!(catalog.title_for_path("/missing.mra"), "Game");
        assert!(catalog.system_games("missing").is_empty());
        assert_eq!(catalog.system_game_count("missing"), 0);
        assert!(catalog.system_game_at("missing", 0).is_none());
        assert!(catalog.system_preview_games("missing").is_empty());
        assert_eq!(catalog.system_preview_game_count("missing"), 0);
        assert!(catalog.system_preview_game_at("missing", 0).is_none());
    }

    #[test]
    fn filter_options_are_precomputed_by_system() {
        let mut first = game("1942", "/games/1942.mra", "", "arcade");
        first.year = Some(1984);
        first.manufacturer = "Capcom".into();
        first.category = "Shooter".into();
        let mut second = game("1943", "/games/1943.mra", "", "arcade");
        second.year = Some(1987);
        second.manufacturer = "Capcom".into();
        second.category = "Shooter".into();
        let mut third = game("Out Run", "/games/outrun.mra", "", "arcade");
        third.year = Some(1986);
        third.manufacturer = "Sega".into();
        third.category = "Driving".into();
        let mut other_system = game("Agony", "/games/agony.mgl", "", "amiga");
        other_system.manufacturer = "Psygnosis".into();

        let catalog = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            vec![first, second, third, other_system],
            Vec::new(),
        );

        assert_eq!(
            catalog.decade_options("arcade"),
            vec![ArcadeFilterOption {
                label: "1980's".into(),
                count: 3
            }]
        );
        assert_eq!(catalog.decade_option_count("arcade"), 1);
        assert_eq!(
            catalog.manufacturer_options("arcade"),
            vec![
                ArcadeFilterOption {
                    label: "Capcom".into(),
                    count: 2
                },
                ArcadeFilterOption {
                    label: "Sega".into(),
                    count: 1
                },
            ]
        );
        assert_eq!(catalog.manufacturer_option_count("arcade"), 2);
        assert_eq!(
            catalog.category_options("arcade"),
            vec![
                ArcadeFilterOption {
                    label: "Driving".into(),
                    count: 1
                },
                ArcadeFilterOption {
                    label: "Shooter".into(),
                    count: 2
                },
            ]
        );
        assert_eq!(catalog.category_option_count("arcade"), 2);
        assert_eq!(catalog.manufacturer_option_count("amiga"), 1);
        assert!(catalog.decade_options("missing").is_empty());
        assert_eq!(catalog.category_option_count("missing"), 0);
    }

    #[test]
    fn preview_games_prefer_exact_or_shorter_title_for_same_family() {
        let games = [
            game(
                "Puzzle Star (World)",
                "/games/puzzle-world.mra",
                "puzzle-world",
                "arcade",
            ),
            game("Puzzle Star", "/games/puzzle.mra", "puzzle", "arcade"),
            game(
                "Space   Duel Alpha",
                "/games/space-extended.mra",
                "space-extended",
                "arcade",
            ),
            game("Space Duel Alpha", "/games/space.mra", "space", "arcade"),
        ];

        let previews = preview_games(games.iter());

        assert_eq!(
            previews
                .iter()
                .map(|game| game.title.as_ref())
                .collect::<Vec<_>>(),
            vec!["Puzzle Star", "Space Duel Alpha"]
        );
    }

    #[test]
    fn preview_games_require_preview_archive_and_asset_key() {
        let games = [
            game("Still Image", "/games/still.mra", "", "arcade"),
            game("Photo", "/games/photo.mra", "photo", "arcade"),
        ];

        let previews = preview_games(games.iter());

        assert_eq!(previews.len(), 1);
        assert_eq!(previews[0].title.as_ref(), "Photo");
    }

    #[test]
    fn systems_from_games_uses_runtime_order_and_human_titles() {
        let games = vec![
            ArcadeGameEntry {
                title: "Unknown Thing".into(),
                mra_path: "/media/fat/_Arcade/Unknown.mra".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "unknown".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                is_new: false,
            },
            ArcadeGameEntry {
                title: "Sonic".into(),
                mra_path: "/media/fat/games/MegaDrive/Sonic.md".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "megadrive".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                is_new: false,
            },
            ArcadeGameEntry {
                title: "1942".into(),
                mra_path: "/media/fat/_Arcade/1942.mra".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "arcade".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                is_new: false,
            },
            ArcadeGameEntry {
                title: "Another Sonic".into(),
                mra_path: "/media/fat/games/MegaDrive/Another Sonic.md".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "megadrive".into(),
                year: None,
                manufacturer: "".into(),
                category: "".into(),
                is_new: false,
            },
        ];

        let systems = systems_from_games(&games);

        assert_eq!(
            systems,
            vec![
                GameSystemEntry {
                    id: "arcade".into(),
                    title: "Arcade".into(),
                    count: 1
                },
                GameSystemEntry {
                    id: "megadrive".into(),
                    title: "Mega Drive".into(),
                    count: 2
                },
                GameSystemEntry {
                    id: "unknown".into(),
                    title: "Unknown".into(),
                    count: 1
                }
            ]
        );
    }
}
