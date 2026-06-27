//! Runtime navigation projection for fast launcher catalog hydration.

use crate::arcade_catalog::{
    ArcadeCatalog, ArcadeGameEntry, GameSystemEntry, LaunchTarget, StructuredLaunchPlan,
};
use crate::catalog_config::{CATALOG_BUILD_VERSION, SCHEMA_VERSION};
use crate::catalog_load_metrics;
use crate::catalog_stamp::CatalogStamp;
use crate::sqlite_catalog;
use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CATALOG_NAVIGATION_SCHEMA_VERSION: u32 = 3;
const CATALOG_NAVIGATION_BINARY_MAGIC: &[u8; 8] = b"MMNAVB3\0";

#[derive(Clone, Debug, PartialEq, Eq)]
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationSystem {
    pub id: String,
    pub title: String,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
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

pub fn navigation_path_for_sqlite(sqlite_path: &Path) -> PathBuf {
    sqlite_path.with_extension("nav.lz4b")
}

pub fn write_catalog_navigation_projection_for_catalog(
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
    let projection = decode_navigation_projection(&decoded)
        .map_err(|e| format!("parse catalog navigation {}: {e}", path.display()))?;
    if !projection.matches(expected_stamp) {
        return Ok(None);
    }
    Ok(Some(projection))
}

impl CatalogNavigationProjection {
    pub fn from_catalog(catalog: &ArcadeCatalog, stamp: &CatalogStamp) -> Self {
        let catalog_stamp_fingerprint = stamp.fingerprint_hex();
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

fn write_catalog_navigation_projection(
    navigation_path: &Path,
    projection: &CatalogNavigationProjection,
) -> Result<(), String> {
    let encoded = encode_navigation_projection(projection)?;
    let compressed = lz4_flex::compress_prepend_size(&encoded);
    write_bytes_atomically(navigation_path, &compressed)
}

fn encode_navigation_projection(projection: &CatalogNavigationProjection) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend_from_slice(CATALOG_NAVIGATION_BINARY_MAGIC);
    write_u32(&mut out, projection.schema);
    write_u32(&mut out, projection.catalog_schema_version);
    write_u32(&mut out, projection.catalog_build_version);
    write_string(&mut out, &projection.catalog_generation)?;
    write_string(&mut out, &projection.catalog_stamp_fingerprint)?;
    write_len(&mut out, projection.catalog_stamp_lines.len())?;
    for line in &projection.catalog_stamp_lines {
        write_string(&mut out, line)?;
    }
    write_len(&mut out, projection.systems.len())?;
    for system in &projection.systems {
        write_string(&mut out, &system.id)?;
        write_string(&mut out, &system.title)?;
        write_u64(&mut out, system.count as u64);
    }
    write_len(&mut out, projection.games.len())?;
    for game in &projection.games {
        write_string(&mut out, &game.title)?;
        write_string(&mut out, &game.launch_ref)?;
        write_string(&mut out, &game.preview_archive_path)?;
        write_string(&mut out, &game.preview_asset_key)?;
        write_bool(&mut out, game.has_preview);
        write_string(&mut out, &game.system_id)?;
        match game.year {
            Some(year) => {
                write_bool(&mut out, true);
                write_u16(&mut out, year);
            }
            None => write_bool(&mut out, false),
        }
        write_string(&mut out, &game.manufacturer)?;
        write_string(&mut out, &game.category)?;
        write_bool(&mut out, game.is_new);
    }
    write_len(&mut out, projection.launch_plans.len())?;
    for plan in &projection.launch_plans {
        write_string(&mut out, &plan.launch_ref)?;
        write_string(&mut out, &plan.title)?;
        write_string(&mut out, &plan.system_id)?;
        write_string(&mut out, &plan.core_path)?;
        write_string(&mut out, &plan.payload_path)?;
        write_string(&mut out, &plan.mount_kind)?;
        out.push(plan.mount_index);
        out.push(plan.delay_secs);
    }
    Ok(out)
}

fn decode_navigation_projection(bytes: &[u8]) -> Result<CatalogNavigationProjection, String> {
    let mut reader = NavigationBinaryReader::new(bytes);
    reader.expect_magic(CATALOG_NAVIGATION_BINARY_MAGIC)?;
    let schema = reader.read_u32()?;
    let catalog_schema_version = reader.read_u32()?;
    let catalog_build_version = reader.read_u32()?;
    let catalog_generation = reader.read_string()?;
    let catalog_stamp_fingerprint = reader.read_string()?;
    let stamp_line_count = reader.read_len()?;
    let mut catalog_stamp_lines = Vec::with_capacity(stamp_line_count);
    for _ in 0..stamp_line_count {
        catalog_stamp_lines.push(reader.read_string()?);
    }
    let system_count = reader.read_len()?;
    let mut systems = Vec::with_capacity(system_count);
    for _ in 0..system_count {
        systems.push(NavigationSystem {
            id: reader.read_string()?,
            title: reader.read_string()?,
            count: reader.read_u64()?.try_into().map_err(|_| "system count too large".to_string())?,
        });
    }
    let game_count = reader.read_len()?;
    let mut games = Vec::with_capacity(game_count);
    for _ in 0..game_count {
        let title = reader.read_string()?;
        let launch_ref = reader.read_string()?;
        let preview_archive_path = reader.read_string()?;
        let preview_asset_key = reader.read_string()?;
        let has_preview = reader.read_bool()?;
        let system_id = reader.read_string()?;
        let year = if reader.read_bool()? {
            Some(reader.read_u16()?)
        } else {
            None
        };
        let manufacturer = reader.read_string()?;
        let category = reader.read_string()?;
        let is_new = reader.read_bool()?;
        games.push(NavigationGame {
            title,
            launch_ref,
            preview_archive_path,
            preview_asset_key,
            has_preview,
            system_id,
            year,
            manufacturer,
            category,
            is_new,
        });
    }
    let launch_plan_count = reader.read_len()?;
    let mut launch_plans = Vec::with_capacity(launch_plan_count);
    for _ in 0..launch_plan_count {
        launch_plans.push(NavigationLaunchPlan {
            launch_ref: reader.read_string()?,
            title: reader.read_string()?,
            system_id: reader.read_string()?,
            core_path: reader.read_string()?,
            payload_path: reader.read_string()?,
            mount_kind: reader.read_string()?,
            mount_index: reader.read_u8()?,
            delay_secs: reader.read_u8()?,
        });
    }
    reader.finish()?;
    Ok(CatalogNavigationProjection {
        schema,
        catalog_schema_version,
        catalog_build_version,
        catalog_generation,
        catalog_stamp_fingerprint,
        catalog_stamp_lines,
        systems,
        games,
        launch_plans,
    })
}

fn write_len(out: &mut Vec<u8>, value: usize) -> Result<(), String> {
    let value: u32 = value
        .try_into()
        .map_err(|_| "catalog navigation collection too large".to_string())?;
    write_u32(out, value);
    Ok(())
}

fn write_string(out: &mut Vec<u8>, value: &str) -> Result<(), String> {
    write_len(out, value.len())?;
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn write_bool(out: &mut Vec<u8>, value: bool) {
    out.push(u8::from(value));
}

fn write_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

struct NavigationBinaryReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> NavigationBinaryReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn expect_magic(&mut self, magic: &[u8]) -> Result<(), String> {
        let bytes = self.take(magic.len())?;
        if bytes == magic {
            Ok(())
        } else {
            Err("navigation projection magic mismatch".to_string())
        }
    }

    fn read_len(&mut self) -> Result<usize, String> {
        self.read_u32()?
            .try_into()
            .map_err(|_| "navigation projection length too large".to_string())
    }

    fn read_string(&mut self) -> Result<String, String> {
        let len = self.read_len()?;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|e| format!("navigation projection string is not utf-8: {e}"))
    }

    fn read_bool(&mut self) -> Result<bool, String> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(format!("navigation projection bool value {value} is invalid")),
        }
    }

    fn read_u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, String> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_u64(&mut self) -> Result<u64, String> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| "navigation projection offset overflow".to_string())?;
        if end > self.bytes.len() {
            return Err("navigation projection is truncated".to_string());
        }
        let bytes = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn finish(self) -> Result<(), String> {
        if self.pos == self.bytes.len() {
            Ok(())
        } else {
            Err("navigation projection has trailing bytes".to_string())
        }
    }
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
        assert_eq!(hydrated.games.len(), catalog.games.len());
        assert_eq!(hydrated.systems, catalog.systems);
        assert_eq!(hydrated.decade_option_count("arcade"), 1);
        assert_eq!(hydrated.manufacturer_option_count("arcade"), 1);
        assert_eq!(hydrated.category_option_count("arcade"), 1);
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
