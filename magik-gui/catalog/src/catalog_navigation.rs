//! Runtime navigation projection for fast launcher catalog hydration.

use crate::arcade_catalog::{
    ArcadeCatalog, ArcadeFilter, ArcadeGameEntry, ArcadeFilterOption, GameSystemEntry,
    LaunchTarget, StructuredLaunchPlan,
};
use crate::catalog_config::{CATALOG_BUILD_VERSION, SCHEMA_VERSION};
use crate::catalog_load_metrics;
use crate::catalog_stamp::CatalogStamp;
use crate::sqlite_catalog;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CATALOG_NAVIGATION_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogNavigationProjection {
    pub schema: u32,
    pub catalog_schema_version: u32,
    pub catalog_build_version: u32,
    pub catalog_generation: String,
    pub catalog_stamp_fingerprint: String,
    pub catalog_stamp_lines: Vec<String>,
    pub systems: Vec<NavigationSystem>,
    pub games: Vec<NavigationGame>,
    pub launch_plans: Vec<NavigationLaunchPlan>,
    pub filter_options: Vec<NavigationFilterOption>,
    pub filter_memberships: Vec<NavigationFilterMembership>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationSystem {
    pub id: String,
    pub title: String,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationGame {
    pub title: String,
    pub launch_ref: String,
    pub preview_archive_path: String,
    pub preview_asset_key: String,
    pub has_preview: bool,
    pub system_id: String,
    pub year: Option<u16>,
    pub manufacturer: String,
    pub category: String,
    pub is_new: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationLaunchPlan {
    pub launch_ref: String,
    pub title: String,
    pub system_id: String,
    pub core_path: String,
    pub payload_path: String,
    pub mount_kind: String,
    pub mount_index: u8,
    pub delay_secs: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationFilterOption {
    pub system_id: String,
    pub kind: String,
    pub label: String,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationFilterMembership {
    pub system_id: String,
    pub kind: String,
    pub label: String,
    pub game_indexes: Vec<usize>,
}

pub fn navigation_path_for_sqlite(sqlite_path: &Path) -> PathBuf {
    sqlite_path.with_extension("nav.lz4b")
}

pub(crate) fn write_catalog_navigation_projection_for_catalog(
    sqlite_path: &Path,
    catalog: &ArcadeCatalog,
    stamp: &CatalogStamp,
) -> Result<(), String> {
    let projection = CatalogNavigationProjection::from_catalog(catalog, stamp);
    write_catalog_navigation_projection(&navigation_path_for_sqlite(sqlite_path), &projection)
}

pub fn read_catalog_navigation_projection(
    path: &Path,
    expected_stamp: &CatalogStamp,
) -> Result<Option<CatalogNavigationProjection>, String> {
    catalog_load_metrics::record_nav_projection_read();
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read catalog navigation {}: {e}", path.display())),
    };
    let decoded = lz4_flex::decompress_size_prepended(&bytes)
        .map_err(|e| format!("decompress catalog navigation {}: {e}", path.display()))?;
    let projection: CatalogNavigationProjection = serde_json::from_slice(&decoded)
        .map_err(|e| format!("parse catalog navigation {}: {e}", path.display()))?;
    if !projection.matches(expected_stamp) {
        return Ok(None);
    }
    Ok(Some(projection))
}

impl CatalogNavigationProjection {
    pub fn from_catalog(catalog: &ArcadeCatalog, stamp: &CatalogStamp) -> Self {
        let catalog_stamp_fingerprint = stamp.fingerprint_hex();
        let game_index_by_ref = catalog
            .games
            .iter()
            .enumerate()
            .map(|(index, game)| (game.mra_path.to_string(), index))
            .collect::<HashMap<_, _>>();
        Self {
            schema: CATALOG_NAVIGATION_SCHEMA_VERSION,
            catalog_schema_version: SCHEMA_VERSION,
            catalog_build_version: CATALOG_BUILD_VERSION,
            catalog_generation: catalog_stamp_fingerprint.clone(),
            catalog_stamp_fingerprint,
            catalog_stamp_lines: stamp.lines().to_vec(),
            systems: catalog.systems.iter().map(NavigationSystem::from).collect(),
            games: catalog.games.iter().map(NavigationGame::from).collect(),
            launch_plans: structured_launch_plans(catalog),
            filter_options: navigation_filter_options(catalog),
            filter_memberships: navigation_filter_memberships(catalog, &game_index_by_ref),
        }
    }

    pub fn matches(&self, expected_stamp: &CatalogStamp) -> bool {
        self.schema == CATALOG_NAVIGATION_SCHEMA_VERSION
            && self.catalog_schema_version == SCHEMA_VERSION
            && self.catalog_build_version == CATALOG_BUILD_VERSION
            && self.catalog_stamp_fingerprint == expected_stamp.fingerprint_hex()
            && self.catalog_stamp_lines == expected_stamp.lines()
    }
}

impl From<&GameSystemEntry> for NavigationSystem {
    fn from(system: &GameSystemEntry) -> Self {
        Self {
            id: system.id.clone(),
            title: system.title.clone(),
            count: system.count,
        }
    }
}

impl From<NavigationSystem> for GameSystemEntry {
    fn from(system: NavigationSystem) -> Self {
        Self {
            id: system.id,
            title: system.title,
            count: system.count,
        }
    }
}

impl From<&ArcadeGameEntry> for NavigationGame {
    fn from(game: &ArcadeGameEntry) -> Self {
        Self {
            title: game.title.to_string(),
            launch_ref: game.mra_path.to_string(),
            preview_archive_path: game.preview_archive_path.to_string(),
            preview_asset_key: game.preview_asset_key.to_string(),
            has_preview: game.has_preview,
            system_id: game.system_id.to_string(),
            year: game.year,
            manufacturer: game.manufacturer.to_string(),
            category: game.category.to_string(),
            is_new: game.is_new,
        }
    }
}

impl From<NavigationGame> for ArcadeGameEntry {
    fn from(game: NavigationGame) -> Self {
        Self {
            title: game.title.into(),
            mra_path: game.launch_ref.into(),
            preview_archive_path: game.preview_archive_path.into(),
            preview_asset_key: game.preview_asset_key.into(),
            has_preview: game.has_preview,
            system_id: game.system_id.into(),
            year: game.year,
            manufacturer: game.manufacturer.into(),
            category: game.category.into(),
            is_new: game.is_new,
        }
    }
}

impl From<&StructuredLaunchPlan> for NavigationLaunchPlan {
    fn from(plan: &StructuredLaunchPlan) -> Self {
        Self {
            launch_ref: plan.launch_ref.to_string(),
            title: plan.title.to_string(),
            system_id: plan.system_id.to_string(),
            core_path: plan.core_path.to_string(),
            payload_path: plan.payload_path.to_string(),
            mount_kind: plan.mount_kind.to_string(),
            mount_index: plan.mount_index,
            delay_secs: plan.delay_secs,
        }
    }
}

impl From<NavigationLaunchPlan> for StructuredLaunchPlan {
    fn from(plan: NavigationLaunchPlan) -> Self {
        Self {
            launch_ref: plan.launch_ref.into(),
            title: plan.title.into(),
            system_id: plan.system_id.into(),
            core_path: plan.core_path.into(),
            payload_path: plan.payload_path.into(),
            mount_kind: plan.mount_kind.into(),
            mount_index: plan.mount_index,
            delay_secs: plan.delay_secs,
        }
    }
}

fn structured_launch_plans(catalog: &ArcadeCatalog) -> Vec<NavigationLaunchPlan> {
    let mut seen = HashSet::new();
    let mut plans = Vec::new();
    for game in &catalog.games {
        if !seen.insert(game.mra_path.to_string()) {
            continue;
        }
        if let LaunchTarget::Structured(plan) = catalog.launch_target_for_ref(game.mra_path.as_ref()) {
            plans.push(NavigationLaunchPlan::from(&plan));
        }
    }
    plans
}

fn navigation_filter_options(catalog: &ArcadeCatalog) -> Vec<NavigationFilterOption> {
    let mut options = Vec::new();
    for system in &catalog.systems {
        append_filter_options(
            &mut options,
            &system.id,
            "decade",
            catalog.decade_options(&system.id),
        );
        append_filter_options(
            &mut options,
            &system.id,
            "manufacturer",
            catalog.manufacturer_options(&system.id),
        );
        append_filter_options(
            &mut options,
            &system.id,
            "category",
            catalog.category_options(&system.id),
        );
    }
    options
}

fn append_filter_options(
    out: &mut Vec<NavigationFilterOption>,
    system_id: &str,
    kind: &str,
    options: Vec<ArcadeFilterOption>,
) {
    out.extend(options.into_iter().map(|option| NavigationFilterOption {
        system_id: system_id.to_string(),
        kind: kind.to_string(),
        label: option.label,
        count: option.count,
    }));
}

fn navigation_filter_memberships(
    catalog: &ArcadeCatalog,
    game_index_by_ref: &HashMap<String, usize>,
) -> Vec<NavigationFilterMembership> {
    let mut memberships = Vec::new();
    for option in navigation_filter_options(catalog) {
        let Some(filter) = filter_from_option(&option) else {
            continue;
        };
        let game_indexes = catalog
            .filtered_game_slice(&option.system_id, &filter)
            .iter()
            .filter_map(|game| game_index_by_ref.get(game.mra_path.as_ref()).copied())
            .collect();
        memberships.push(NavigationFilterMembership {
            system_id: option.system_id,
            kind: option.kind,
            label: option.label,
            game_indexes,
        });
    }
    memberships
}

fn filter_from_option(option: &NavigationFilterOption) -> Option<ArcadeFilter> {
    match option.kind.as_str() {
        "decade" => option
            .label
            .strip_suffix("'s")
            .and_then(|label| label.parse::<u16>().ok())
            .map(ArcadeFilter::Decade),
        "manufacturer" => Some(ArcadeFilter::Manufacturer(option.label.clone())),
        "category" => Some(ArcadeFilter::Category(option.label.clone())),
        _ => None,
    }
}

fn write_catalog_navigation_projection(
    navigation_path: &Path,
    projection: &CatalogNavigationProjection,
) -> Result<(), String> {
    let encoded = serde_json::to_vec(projection)
        .map_err(|e| format!("serialize catalog navigation: {e}"))?;
    let compressed = lz4_flex::compress_prepend_size(&encoded);
    write_bytes_atomically(navigation_path, &compressed)
}

fn write_bytes_atomically(final_path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create catalog navigation dir {}: {e}", parent.display()))?;
    }
    let temp_path = navigation_temp_path_for(final_path);
    let result = (|| -> Result<(), String> {
        let mut file = File::create(&temp_path).map_err(|e| {
            format!(
                "create catalog navigation temp {}: {e}",
                temp_path.display()
            )
        })?;
        file.write_all(bytes).map_err(|e| {
            format!(
                "write catalog navigation temp {}: {e}",
                temp_path.display()
            )
        })?;
        file.sync_all().map_err(|e| {
            format!("sync catalog navigation temp {}: {e}", temp_path.display())
        })?;
        drop(file);
        std::fs::rename(&temp_path, final_path).map_err(|e| {
            format!(
                "replace catalog navigation {} from {}: {e}",
                final_path.display(),
                temp_path.display()
            )
        })?;
        sqlite_catalog::sync_parent_dir(final_path);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn navigation_temp_path_for(final_path: &Path) -> PathBuf {
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("catalog.nav.lz4b");
    final_path.with_file_name(format!(".{file_name}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn stamp(lines: &[&str]) -> CatalogStamp {
        CatalogStamp::from_lines(lines.iter().map(|line| line.to_string()).collect())
    }

    fn game(title: &str, launch_ref: &str, system_id: &str) -> ArcadeGameEntry {
        ArcadeGameEntry {
            title: Arc::from(title),
            mra_path: Arc::from(launch_ref),
            preview_archive_path: Arc::from("/media/fat/mister-magik/assets/arcade.mmlz4b"),
            preview_asset_key: Arc::from(title.to_ascii_lowercase()),
            has_preview: true,
            system_id: Arc::from(system_id),
            year: Some(1984),
            manufacturer: Arc::from("Capcom"),
            category: Arc::from("Shooter"),
            is_new: false,
        }
    }

    fn projection_catalog() -> ArcadeCatalog {
        let games = vec![
            game("1942", "/media/fat/_Arcade/1942.mra", "arcade"),
            game("Nights", "magik-plan:saturn:nights", "saturn"),
        ];
        let systems = vec![
            GameSystemEntry {
                id: "arcade".to_string(),
                title: "Arcade".to_string(),
                count: 1,
            },
            GameSystemEntry {
                id: "saturn".to_string(),
                title: "Saturn".to_string(),
                count: 1,
            },
        ];
        let plans = vec![StructuredLaunchPlan {
            launch_ref: Arc::from("magik-plan:saturn:nights"),
            title: Arc::from("Nights"),
            system_id: Arc::from("saturn"),
            core_path: Arc::from("_Console/Saturn"),
            payload_path: Arc::from("/media/fat/games/Saturn/Nights.chd"),
            mount_kind: Arc::from("mount-image"),
            mount_index: 0,
            delay_secs: 1,
        }];
        ArcadeCatalog::new_with_launch_plans(PathBuf::from("/media/fat/_Arcade"), games, systems, plans)
    }

    #[test]
    fn navigation_projection_round_trips_catalog_rows_and_plans() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-navigation-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temp dir");
        let db = root.join("library.sqlite3");
        let stamp = stamp(&["root /media/fat/games"]);
        let catalog = projection_catalog();

        write_catalog_navigation_projection_for_catalog(&db, &catalog, &stamp)
            .expect("write projection");
        let loaded = read_catalog_navigation_projection(&navigation_path_for_sqlite(&db), &stamp)
            .expect("read projection")
            .expect("current projection");
        let hydrated =
            ArcadeCatalog::from_navigation_projection("/media/fat/_Arcade", loaded.clone());

        assert_eq!(loaded.games.len(), catalog.games.len());
        assert_eq!(loaded.systems.len(), catalog.systems.len());
        assert_eq!(loaded.launch_plans.len(), 1);
        assert_eq!(loaded.filter_options.len(), 6);
        assert_eq!(loaded.filter_memberships.len(), 6);
        assert_eq!(hydrated.games.len(), catalog.games.len());
        assert_eq!(hydrated.systems, catalog.systems);
        assert!(matches!(
            hydrated.launch_target_for_ref("magik-plan:saturn:nights"),
            LaunchTarget::Structured(_)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn navigation_projection_rejects_stale_stamp_and_corrupt_file() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-navigation-stale-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create temp dir");
        let db = root.join("library.sqlite3");
        let path = navigation_path_for_sqlite(&db);
        let current_stamp = stamp(&["root current"]);
        let stale_stamp = stamp(&["root stale"]);

        write_catalog_navigation_projection_for_catalog(&db, &projection_catalog(), &current_stamp)
            .expect("write projection");
        assert!(
            read_catalog_navigation_projection(&path, &stale_stamp)
                .expect("read stale projection")
                .is_none()
        );
        std::fs::write(&path, b"not-lz4").expect("write corrupt projection");
        assert!(read_catalog_navigation_projection(&path, &current_stamp).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
