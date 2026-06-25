//! Whole-MiSTer library database scanning and loading.
//!
//! This is deliberately TOC/header-only for archives. Indexing must never
//! decompress full game libraries just to make the launcher searchable.

use crate::arcade_catalog::ArcadeCatalog;
use crate::catalog_build::CatalogRefreshPipeline;
use crate::catalog_config;
use crate::catalog_config::DEFAULT_SQLITE_PATH;
pub use crate::catalog_config::{
    default_hbmame_sqlite_path, default_mame_sqlite_path, default_sqlite_path,
};
pub(crate) use crate::catalog_progress::ProgressCallback;
pub use crate::catalog_progress::{
    catalog_progress_percent_from_display, CatalogProgress, CatalogProgressPhase,
};
pub(crate) use crate::catalog_projection::canonical_variant_title;
use crate::catalog_projection::{
    self, CatalogProjectionRow, CatalogProjectionSource, LauncherPreviewAsset,
};
use crate::catalog_stamp;
use crate::game_discovery::{
    catalog_system_id_for_discovery, covered_payload_paths, is_launcher_launch_ref,
    launch_kind_for_discovery, launch_ref_for_discovery, preferred_playable_discoveries_by_key,
    GameDiscovery,
};
use crate::launch_profiles::{CollectionListing, PayloadRule};
use crate::library_indexer::LibraryIndexer;
use crate::software_identity::{
    load_arcade_machine_metadata, mame_identity_for_discovery, mame_identity_projection,
    write_simple_mame_metadata_db, ArcadeMachineMetadata, MachineMetadataRows,
};
use crate::sqlite_catalog;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) const MRA_PREFIX_BYTES: usize = 160 * 1024;
pub type ScanEventCallback<'a> = Option<&'a mut dyn FnMut(LibraryScanEvent)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryScanEvent {
    SystemDiscovered { system_id: String },
}

pub(crate) const AMIGAVISION_GAME_LAUNCH_PREFIX: &str = "magik-amigavision:";
pub(crate) const AMIGAVISION_LAUNCHER_REF: &str = "magik-amigavision-launcher";

pub(crate) const AMIGAVISION_INSTALLED_LISTINGS: &[CollectionListing] = &[
    CollectionListing {
        entry_path: "listings/games.txt",
        genre: "AmigaVision",
    },
    CollectionListing {
        entry_path: "listings/demos.txt",
        genre: "AmigaVision demos",
    },
];
#[derive(Clone, Debug)]
pub struct LibraryContainer {
    pub file_path: String,
    pub format: ArchiveFormat,
    pub size: u64,
    pub mtime_secs: i64,
    pub entry_count: u32,
    pub scan_status: ArchiveScanStatus,
    pub scan_us: u64,
}

#[derive(Clone, Debug)]
pub struct LibraryContainerEntry {
    pub file_path: String,
    pub entry_path: String,
    pub normalized_title: String,
    pub profile_id: String,
    pub rule: PayloadRule,
    pub compressed_size: Option<u64>,
    pub uncompressed_size: Option<u64>,
    pub crc32: Option<u32>,
    pub launchable: bool,
    pub launch_ref: String,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ArchiveFormat {
    Zip,
    SevenZip,
    Lha,
    Lzh,
    Rar,
    Chd,
}

impl ArchiveFormat {
    pub(crate) fn from_ext(ext: &str) -> Option<Self> {
        match ext {
            "zip" => Some(Self::Zip),
            "7z" => Some(Self::SevenZip),
            "lha" => Some(Self::Lha),
            "lzh" => Some(Self::Lzh),
            "rar" => Some(Self::Rar),
            "chd" => Some(Self::Chd),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ArchiveScanStatus {
    Ok,
    HeaderOnly,
    Unsupported,
    Error(String),
}

#[derive(Clone, Debug)]
pub(crate) struct LibraryScan {
    pub(crate) version: u32,
    pub(crate) scanned_at_unix: i64,
    pub(crate) normal_files: Vec<LibraryPayloadFile>,
    pub(crate) containers: Vec<LibraryContainer>,
    pub(crate) entries: Vec<LibraryContainerEntry>,
    pub(crate) ignored_files: usize,
    pub(crate) discoveries: Vec<GameDiscovery>,
    pub(crate) discover_us: u64,
    pub(crate) classify_us: u64,
}

pub struct LibraryCatalogLoad {
    pub catalog: ArcadeCatalog,
    pub us: u64,
    pub open_us: u64,
    pub query_us: u64,
    pub systems_us: u64,
    pub catalog_us: u64,
    pub rows: usize,
}

#[derive(Clone, Debug)]
pub struct LibraryScanStats {
    pub scan_us: u64,
    pub discover_us: u64,
    pub classify_us: u64,
    pub normal_files: usize,
    pub containers: usize,
    pub entries: usize,
    pub discoveries: usize,
}

pub struct LibraryScanArtifact {
    pub(crate) scan: LibraryScan,
    pub(crate) stats: LibraryScanStats,
    pub(crate) stamp: catalog_stamp::CatalogStamp,
}

#[derive(Clone, Debug, Default)]
pub struct LibraryBootstrapSummary {
    pub launchers: usize,
    pub scan_us: u64,
}

impl LibraryScanArtifact {
    pub fn stats(&self) -> &LibraryScanStats {
        &self.stats
    }

    pub fn arcade_catalog(&self, root: impl AsRef<Path>) -> ArcadeCatalog {
        build_arcade_catalog_from_scan(root, &self.scan)
    }

    pub fn save_default_sqlite(self) -> Result<LibraryRefreshSummary, String> {
        self.save_default_sqlite_with_progress(None)
    }

    pub fn save_default_sqlite_with_progress(
        self,
        progress: ProgressCallback<'_>,
    ) -> Result<LibraryRefreshSummary, String> {
        let cfg = BenchConfig::production();
        save_scan_artifact_to_sqlite(&cfg, self, progress)
    }

    pub fn save_default_sqlite_with_catalog(
        self,
        root: impl AsRef<Path>,
        progress: ProgressCallback<'_>,
    ) -> Result<LibraryRefreshCatalog, String> {
        let cfg = BenchConfig::production();
        save_scan_artifact_to_sqlite_with_catalog(&cfg, self, root, progress)
    }
}

#[derive(Clone, Debug)]
pub struct LibraryRefreshSummary {
    pub skipped: bool,
    pub scan_us: u64,
    pub discover_us: u64,
    pub classify_us: u64,
    pub import_us: u64,
    pub bytes: u64,
    pub normal_files: usize,
    pub containers: usize,
    pub entries: usize,
    pub discoveries: usize,
}

pub struct LibraryRefreshCatalog {
    pub summary: LibraryRefreshSummary,
    pub catalog: LibraryCatalogLoad,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogStampCheckSummary {
    pub unchanged: bool,
    pub check_us: u64,
    pub compute_us: u64,
    pub open_us: u64,
    pub read_us: u64,
    pub compare_us: u64,
    pub stored_fingerprint: Option<String>,
    pub current_fingerprint: String,
    pub stored_lines: usize,
    pub current_lines: usize,
}

pub struct HbmameMetadataSummary {
    pub path: PathBuf,
    pub rows: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtualLaunchPlan {
    pub launch_ref: String,
    pub title: String,
    pub system_id: String,
    pub core_path: String,
    pub payload_path: String,
    pub mount_kind: String,
    pub mount_index: u8,
    pub mount_delay_secs: u8,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct FileSignature {
    pub(crate) size: u64,
    pub(crate) mtime_secs: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct LibraryPayloadFile {
    pub(crate) path: String,
    pub(crate) profile_id: String,
    pub(crate) size: u64,
    pub(crate) mtime_secs: i64,
    pub(crate) rule: PayloadRule,
}

pub fn run_scan_bench() {
    crate::library_cli::run_scan_bench();
}

pub fn run_sqlite_inspect_cli(args: &[String]) -> Result<String, String> {
    crate::library_cli::run_sqlite_inspect_cli(args)
}

pub fn remove_default_sqlite_database() -> Result<(), String> {
    sqlite_catalog::remove_default_sqlite_database()
}

pub fn load_virtual_launch_plans_for_system(
    system_id: &str,
    limit: usize,
) -> Result<Vec<VirtualLaunchPlan>, String> {
    sqlite_catalog::load_virtual_launch_plans_for_system(system_id, limit)
}

pub fn load_amigavision_launch_refs(limit: usize) -> Result<Vec<String>, String> {
    sqlite_catalog::load_amigavision_launch_refs(limit)
}

pub fn load_arcade_catalog_from_sqlite(
    root: impl AsRef<Path>,
) -> Result<LibraryCatalogLoad, String> {
    sqlite_catalog::load_arcade_catalog_from_sqlite(root)
}

#[cfg(test)]
fn load_arcade_catalog_from_sqlite_at(
    root: impl AsRef<Path>,
    path: &Path,
) -> Result<LibraryCatalogLoad, String> {
    sqlite_catalog::load_arcade_catalog_from_sqlite_at(root, path)
}

pub(crate) fn sqlite_table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    sqlite_catalog::sqlite_table_exists(conn, table)
}

pub(crate) fn open_sqlite_read_only(path: &Path) -> rusqlite::Result<Connection> {
    sqlite_catalog::open_sqlite_read_only(path)
}

pub(crate) fn sqlite_temp_path(path: &Path) -> PathBuf {
    sqlite_catalog::sqlite_temp_path(path)
}

pub(crate) fn sync_parent_dir(path: &Path) {
    sqlite_catalog::sync_parent_dir(path)
}

pub(crate) fn file_signature(path: &Path) -> FileSignature {
    sqlite_catalog::file_signature(path)
}

pub(crate) fn sqlite_catalog_stamp_check(
    cfg: &BenchConfig,
) -> Result<CatalogStampCheckSummary, String> {
    sqlite_catalog::sqlite_catalog_stamp_check(cfg)
}

#[cfg(test)]
fn save_sqlite_scan(path: &Path, scan: &LibraryScan) -> Result<u64, String> {
    sqlite_catalog::save_sqlite_scan(path, scan)
}

fn sqlite_cached_summary(path: &Path, scan_us: u64) -> Result<LibraryRefreshSummary, String> {
    sqlite_catalog::sqlite_cached_summary(path, scan_us)
}

#[cfg(test)]
fn write_sqlite_scan_with_mame(
    path: &Path,
    scan: &LibraryScan,
    mame_sqlite_path: &Path,
) -> Result<(), String> {
    sqlite_catalog::write_sqlite_scan_with_mame(path, scan, mame_sqlite_path)
}

pub fn rebuild_default_sqlite_database(
    progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    rebuild_default_sqlite_database_with_events(progress, None)
}

pub fn rebuild_default_sqlite_database_with_events(
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    let cfg = BenchConfig::production();
    rebuild_sqlite_database_with_events(&cfg, progress, scan_events)
}

pub fn rebuild_default_sqlite_database_with_catalog(
    root: impl AsRef<Path>,
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> Result<LibraryRefreshCatalog, String> {
    let cfg = BenchConfig::production();
    rebuild_sqlite_database_with_catalog(&cfg, root, progress, scan_events)
}

pub fn scan_default_library(progress: ProgressCallback<'_>) -> Result<LibraryScanArtifact, String> {
    scan_default_library_with_events(progress, None)
}

pub fn scan_default_library_with_events(
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> Result<LibraryScanArtifact, String> {
    let cfg = BenchConfig::production();
    Ok(scan_library_artifact_with_events(
        &cfg,
        progress,
        scan_events,
    ))
}

pub fn bootstrap_default_library_progress(
    progress: ProgressCallback<'_>,
) -> LibraryBootstrapSummary {
    let cfg = BenchConfig::production();
    bootstrap_library_progress(&cfg, progress)
}

pub fn write_default_hbmame_metadata_from_library() -> Result<HbmameMetadataSummary, String> {
    write_hbmame_metadata_from_library(&default_sqlite_path(), &default_hbmame_sqlite_path())
}

pub(crate) fn write_hbmame_metadata_from_library(
    library_path: &Path,
    hbmame_path: &Path,
) -> Result<HbmameMetadataSummary, String> {
    let conn = open_sqlite_read_only(library_path)
        .map_err(|e| format!("open library db {}: {e}", library_path.display()))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    let mut stmt = conn
        .prepare(
            "SELECT
                COALESCE(p.setname, ''),
                COALESCE(p.parent, ''),
                g.title,
                g.year,
                g.manufacturer
             FROM launch_plans p
             JOIN games g ON g.game_id = p.game_id
             WHERE p.launch_kind = 'mra'
               AND COALESCE(p.setname, '') != ''
               AND COALESCE(p.parent, '') != ''",
        )
        .map_err(|e| format!("prepare hbmame metadata query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|e| format!("query hbmame metadata rows: {e}"))?;
    let mut machines: MachineMetadataRows = BTreeMap::new();
    for row in rows {
        let (setname, parent, title, year, manufacturer) =
            row.map_err(|e| format!("read hbmame metadata row: {e}"))?;
        let identity_id = normalize_id(&setname);
        let family_id = normalize_id(&parent);
        if identity_id.is_empty() || family_id.is_empty() || identity_id == family_id {
            continue;
        }
        machines.entry(identity_id).or_insert_with(|| {
            (
                family_id,
                title,
                year.map(|value| value.to_string()),
                manufacturer,
            )
        });
    }
    write_simple_mame_metadata_db(hbmame_path, &machines)?;
    Ok(HbmameMetadataSummary {
        path: hbmame_path.to_path_buf(),
        rows: machines.len(),
    })
}

pub fn default_sqlite_catalog_stamp_check() -> Result<CatalogStampCheckSummary, String> {
    let cfg = BenchConfig::production();
    sqlite_catalog_stamp_check(&cfg)
}

pub fn default_sqlite_cached_summary(scan_us: u64) -> Result<LibraryRefreshSummary, String> {
    sqlite_cached_summary(&default_sqlite_path(), scan_us)
}

pub(crate) fn rebuild_sqlite_database(
    cfg: &BenchConfig,
    progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    rebuild_sqlite_database_with_events(cfg, progress, None)
}

pub(crate) fn rebuild_sqlite_database_with_events(
    cfg: &BenchConfig,
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    CatalogRefreshPipeline::new(cfg).rebuild_with_events(progress, scan_events)
}

pub(crate) fn rebuild_sqlite_database_with_catalog(
    cfg: &BenchConfig,
    root: impl AsRef<Path>,
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> Result<LibraryRefreshCatalog, String> {
    CatalogRefreshPipeline::new(cfg).rebuild_with_catalog(root, progress, scan_events)
}

pub(crate) struct BenchConfig {
    pub(crate) roots: Vec<String>,
    pub(crate) sqlite_path: PathBuf,
}

impl BenchConfig {
    pub(crate) fn from_env() -> Self {
        let roots = catalog_config::library_roots_from_env();
        let sqlite_path = std::env::var("MISTER_LIBRARY_BENCH_SQLITE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_SQLITE_PATH));
        Self { roots, sqlite_path }
    }

    fn production() -> Self {
        let mut cfg = Self::from_env();
        cfg.sqlite_path = default_sqlite_path();
        cfg
    }
}

pub(crate) fn env_bool(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub(crate) fn scan_library(cfg: &BenchConfig) -> LibraryScan {
    LibraryIndexer::new(cfg).scan()
}

pub(crate) fn scan_library_artifact(
    cfg: &BenchConfig,
    progress: ProgressCallback<'_>,
) -> LibraryScanArtifact {
    CatalogRefreshPipeline::new(cfg).scan_artifact(progress)
}

pub(crate) fn scan_library_artifact_with_events(
    cfg: &BenchConfig,
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> LibraryScanArtifact {
    CatalogRefreshPipeline::new(cfg).scan_artifact_with_events(progress, scan_events)
}

pub(crate) fn save_scan_artifact_to_sqlite(
    cfg: &BenchConfig,
    artifact: LibraryScanArtifact,
    progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    CatalogRefreshPipeline::new(cfg).save_artifact(artifact, progress)
}

pub(crate) fn save_scan_artifact_to_sqlite_with_catalog(
    cfg: &BenchConfig,
    artifact: LibraryScanArtifact,
    root: impl AsRef<Path>,
    progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshCatalog, String> {
    CatalogRefreshPipeline::new(cfg).save_artifact_with_catalog(artifact, root, progress)
}

fn build_arcade_catalog_from_scan(root: impl AsRef<Path>, scan: &LibraryScan) -> ArcadeCatalog {
    let arcade_metadata =
        load_arcade_machine_metadata(&default_mame_sqlite_path(), &default_hbmame_sqlite_path());
    build_arcade_catalog_from_scan_with_metadata(root, scan, &arcade_metadata)
}

fn build_arcade_catalog_from_scan_with_metadata(
    root: impl AsRef<Path>,
    scan: &LibraryScan,
    arcade_metadata: &ArcadeMachineMetadata,
) -> ArcadeCatalog {
    let covered_payloads = covered_payload_paths(&scan.discoveries);
    let discoveries = preferred_playable_discoveries_by_key(&scan.discoveries, &covered_payloads);
    let mut rows = Vec::<CatalogProjectionRow>::new();
    for (key, discovery) in discoveries {
        let system_id = catalog_system_id_for_discovery(discovery);
        let plan_launch_ref = launch_ref_for_discovery(&key, discovery);
        if !is_launcher_launch_ref(&plan_launch_ref) {
            continue;
        }
        let (setname, parent) = catalog_family_fields_for_discovery(discovery, arcade_metadata);
        rows.push(CatalogProjectionRow::new(
            discovery.title.clone(),
            plan_launch_ref,
            system_id,
            LauncherPreviewAsset::none(),
            false,
            CatalogProjectionSource {
                discovered_at_unix: None,
                source_kind: launch_kind_for_discovery(discovery).to_string(),
                setname,
                parent,
                family_key: None,
            },
        ));
    }
    catalog_projection::catalog_from_projection_rows(root, rows)
}

fn catalog_family_fields_for_discovery(
    discovery: &GameDiscovery,
    arcade_metadata: &ArcadeMachineMetadata,
) -> (String, String) {
    if let Some(identity_id) = mame_identity_for_discovery(discovery) {
        let (family_id, _, _, _, _) =
            mame_identity_projection(&identity_id, arcade_metadata, discovery.parent.as_deref());
        let parent = if family_id == identity_id {
            String::new()
        } else {
            family_id
        };
        return (identity_id, parent);
    }
    (
        discovery.setname.clone().unwrap_or_default(),
        discovery.parent.clone().unwrap_or_default(),
    )
}

#[cfg(test)]
fn scan_library_with_progress(cfg: &BenchConfig, progress: ProgressCallback<'_>) -> LibraryScan {
    LibraryIndexer::new(cfg).scan_with_progress_and_events(progress, None)
}

#[cfg(test)]
fn scan_library_with_progress_and_events(
    cfg: &BenchConfig,
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> LibraryScan {
    LibraryIndexer::new(cfg).scan_with_progress_and_events(progress, scan_events)
}

fn bootstrap_library_progress(
    cfg: &BenchConfig,
    progress: ProgressCallback<'_>,
) -> LibraryBootstrapSummary {
    LibraryIndexer::new(cfg).bootstrap_progress(progress)
}

pub(crate) fn normalize_id(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.trim().chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

pub(crate) fn report_library_scan_timing(stage: &str, us: u64, detail: impl std::fmt::Display) {
    println!("library_scan_timing\t{stage}\t{us}\t{detail}");
}

pub(crate) fn normalize_title(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_ascii_lowercase()
}

pub(crate) fn title_from_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .trim()
        .to_string()
}

pub(crate) fn path_ext(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

pub(crate) fn find_eocd(buf: &[u8]) -> Option<usize> {
    if buf.len() < 22 {
        return None;
    }
    (0..=buf.len() - 22)
        .rev()
        .find(|&idx| buf[idx..idx + 4] == [0x50, 0x4b, 0x05, 0x06])
}

pub(crate) fn le_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

pub(crate) fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

pub(crate) fn be_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

pub(crate) fn le_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub(crate) fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

pub(crate) fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog_store;
    use crate::game_discovery::unique_discovery_count;
    use crate::test_support::*;

    #[test]
    fn ram_catalog_from_scan_matches_sqlite_catalog_for_simple_mra_fixture() {
        let scan = sqlite_scan_with_discoveries(vec![
            mra_discovery(1, "Alpha Mission"),
            mra_discovery(2, "Beta Fighter"),
        ]);
        let ram_catalog = build_arcade_catalog_from_scan("/media/fat/_Arcade", &scan);
        let root = unique_temp_dir("ram-catalog-projection");
        std::fs::create_dir_all(&root).expect("create temp root");
        let db = root.join("library.sqlite3");
        save_sqlite_scan(&db, &scan).expect("save sqlite");
        let sqlite_catalog =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load sqlite");

        assert_eq!(ram_catalog.len(), sqlite_catalog.catalog.len());
        assert_eq!(
            ram_catalog
                .games
                .iter()
                .map(|game| game.mra_path.as_ref())
                .collect::<Vec<_>>(),
            sqlite_catalog
                .catalog
                .games
                .iter()
                .map(|game| game.mra_path.as_ref())
                .collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ram_catalog_uses_mame_metadata_families_like_sqlite_catalog() {
        let root = unique_temp_dir("ram-catalog-mame-families");
        std::fs::create_dir_all(&root).expect("create temp root");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[
                ("1942", None, "1942", Some("1984"), Some("Capcom")),
                (
                    "1942b",
                    Some("1942"),
                    "1942 (First Version)",
                    Some("1984"),
                    Some("Capcom"),
                ),
            ],
        );
        let mut parent = mra_discovery(1, "1942");
        parent.setname = Some("1942".to_string());
        let mut clone = mra_discovery(2, "1942 (First Version)");
        clone.setname = Some("1942b".to_string());
        clone.parent = None;
        let scan = sqlite_scan_with_discoveries(vec![parent, clone]);
        let metadata = load_arcade_machine_metadata(&mame_db, &PathBuf::new());
        let ram_catalog =
            build_arcade_catalog_from_scan_with_metadata("/media/fat/_Arcade", &scan, &metadata);

        write_sqlite_scan_with_mame(&db, &scan, &mame_db).expect("save sqlite");
        let sqlite_catalog =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load sqlite");

        assert_eq!(ram_catalog.system_game_count("arcade"), 1);
        assert_eq!(
            ram_catalog.system_game_count("arcade"),
            sqlite_catalog.catalog.system_game_count("arcade")
        );
        assert_eq!(
            ram_catalog.games[0].title.as_ref(),
            sqlite_catalog.catalog.games[0].title.as_ref()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn scan_artifact_save_writes_catalog_stamp() {
        let root = unique_temp_dir("sqlite-catalog-stamp");
        let db = root.join("library.sqlite3");
        let games = root.join("games");
        std::fs::create_dir_all(&games).expect("create games dir");
        let cfg = BenchConfig {
            roots: vec![games.display().to_string()],
            sqlite_path: db.clone(),
        };
        let artifact = scan_library_artifact(&cfg, None);
        let expected_stamp = artifact.stamp.clone();

        save_scan_artifact_to_sqlite(&cfg, artifact, None).expect("save artifact");

        let conn = Connection::open(&db).expect("open sqlite");
        let stored = catalog_store::read_catalog_stamp(&conn)
            .expect("read catalog stamp")
            .expect("catalog stamp");
        assert_eq!(stored, expected_stamp);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scan_progress_waits_for_batch_before_reporting_found_games() {
        let root = unique_temp_dir("scan-progress-first-batch");
        let games = root.join("games");
        let nes = games.join("NES");
        std::fs::create_dir_all(&nes).expect("create NES dir");
        std::fs::write(nes.join("Super Mario Bros.nes"), b"nes").expect("write NES game");
        let cfg = BenchConfig {
            roots: vec![games.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };
        let mut messages = Vec::<(String, String)>::new();
        let mut progress = |title: &str, detail: &str| {
            messages.push((title.to_string(), detail.to_string()));
        };

        let scan = scan_library_with_progress(&cfg, Some(&mut progress));

        assert_eq!(unique_discovery_count(&scan.discoveries), 1);
        assert!(!messages
            .iter()
            .any(|(title, detail)| title == "Classifying library"
                && detail.starts_with("Games found: ")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scan_events_report_supported_system_once() {
        let root = unique_temp_dir("scan-events-supported-systems");
        let games = root.join("games");
        let nes = games.join("NES");
        let snes = games.join("SNES");
        std::fs::create_dir_all(&nes).expect("create NES dir");
        std::fs::create_dir_all(&snes).expect("create SNES dir");
        std::fs::write(nes.join("Super Mario Bros.nes"), b"nes").expect("write NES game");
        std::fs::write(nes.join("Legend of Zelda.nes"), b"nes").expect("write second NES game");
        std::fs::write(snes.join("ActRaiser.sfc"), b"snes").expect("write SNES game");
        let cfg = BenchConfig {
            roots: vec![games.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };
        let mut systems = Vec::new();
        let mut scan_events = |event: LibraryScanEvent| match event {
            LibraryScanEvent::SystemDiscovered { system_id } => systems.push(system_id),
        };

        let scan = scan_library_with_progress_and_events(&cfg, None, Some(&mut scan_events));

        assert_eq!(unique_discovery_count(&scan.discoveries), 3);
        systems.sort();
        assert_eq!(systems, ["nes", "snes"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scan_events_ignore_systems_without_screenshot_packs() {
        let root = unique_temp_dir("scan-events-unsupported-systems");
        let games = root.join("games");
        let gba = games.join("GBA");
        let nes = games.join("NES");
        std::fs::create_dir_all(&gba).expect("create GBA dir");
        std::fs::create_dir_all(&nes).expect("create NES dir");
        std::fs::write(gba.join("Advance Wars.gba"), b"gba").expect("write GBA game");
        std::fs::write(nes.join("Metroid.nes"), b"nes").expect("write NES game");
        let cfg = BenchConfig {
            roots: vec![games.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };
        let mut systems = Vec::new();
        let mut scan_events = |event: LibraryScanEvent| match event {
            LibraryScanEvent::SystemDiscovered { system_id } => systems.push(system_id),
        };

        let scan = scan_library_with_progress_and_events(&cfg, None, Some(&mut scan_events));

        assert_eq!(unique_discovery_count(&scan.discoveries), 2);
        assert_eq!(systems, ["nes"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bootstrap_progress_reports_after_50_real_launchers() {
        let root = unique_temp_dir("bootstrap-progress");
        let arcade = root.join("_Arcade");
        std::fs::create_dir_all(&arcade).expect("create arcade dir");
        for idx in 0..55 {
            std::fs::write(arcade.join(format!("Game {idx:02}.mra")), b"<mra/>")
                .expect("write mra");
        }
        std::fs::write(arcade.join("not-a-launcher.txt"), b"ignore").expect("write ignored file");
        let cfg = BenchConfig {
            roots: vec![arcade.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };
        let mut messages = Vec::<(String, String)>::new();
        let mut progress = |title: &str, detail: &str| {
            messages.push((title.to_string(), detail.to_string()));
        };

        let summary = bootstrap_library_progress(&cfg, Some(&mut progress));

        assert_eq!(summary.launchers, 55);
        assert!(messages
            .iter()
            .any(|(title, detail)| title == "Finding games" && detail == "Games found: 50"));
        assert!(!messages
            .iter()
            .any(|(title, detail)| title == "Finding games" && detail == "Games found: 1"));
        let _ = std::fs::remove_dir_all(root);
    }
}
