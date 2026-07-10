//! Whole-MiSTer library database scanning and loading.
//!
//! This is deliberately TOC/header-only for archives. Indexing must never
//! decompress full game libraries just to make the launcher searchable.

use crate::arcade_catalog::{self, ArcadeCatalog, ArcadeGameMetadataKey, StructuredLaunchPlan};
use crate::catalog_build::CatalogRefreshPipeline;
use crate::catalog_checkpoint::CatalogDriftSummary;
use crate::catalog_config;
use crate::catalog_config::DEFAULT_SQLITE_PATH;
pub use crate::catalog_config::{
    default_hbmame_sqlite_path, default_mame_sqlite_path, default_sqlite_path,
};
use crate::catalog_discovery::{GameDirFact, InstalledCore};
use crate::catalog_load_metrics;
pub use crate::catalog_navigation::{
    navigation_path_for_sqlite, read_catalog_navigation_projection,
    write_catalog_navigation_projection_for_catalog, CatalogNavigationProjection,
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
use crate::core_audit::{self, CatalogAuditRow};
use crate::game_discovery::{
    catalog_system_id_for_discovery, covered_payload_paths, is_launcher_launch_ref,
    is_raw_arcade_zip_set_discovery, launch_kind_for_discovery, launch_ref_for_discovery,
    preferred_playable_discoveries_by_key, profile_id_for_discovery, DiscoverySourceKind,
    GameDiscovery,
};
use crate::launch_profiles::{self, CollectionListing, LaunchProfile, PayloadRule};
use crate::library_indexer::LibraryIndexer;
use crate::preview_worker;
use crate::software_identity::{
    console_preview_asset, load_arcade_machine_metadata_for_setnames, load_mame_software_metadata,
    mame_identity_for_discovery, mame_identity_projection, mame_software_identity_for_discovery,
    write_simple_mame_metadata_db, ArcadeMachineMetadata, MachineMetadataRows,
    MameSoftwareMetadata, PreviewArchivePaths, SoftwareHashCache,
};
use crate::sqlite_catalog;
use rusqlite::Connection;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

pub(crate) const MRA_PREFIX_BYTES: usize = 160 * 1024;
pub type ScanEventCallback<'a> = Option<&'a mut dyn FnMut(LibraryScanEvent)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryScanEvent {
    SystemDiscovered { system_id: String },
}

pub(crate) const AMIGAVISION_GAME_LAUNCH_PREFIX: &str = "magik-amigavision:";
pub(crate) const AMIGAVISION_LAUNCHER_REF: &str = "magik-amigavision-launcher";

pub(crate) fn amigavision_installed_listings() -> Vec<CollectionListing> {
    vec![
        CollectionListing {
            entry_path: "listings/games.txt".to_string(),
            genre: "AmigaVision".to_string(),
        },
        CollectionListing {
            entry_path: "listings/demos.txt".to_string(),
            genre: "AmigaVision demos".to_string(),
        },
    ]
}
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
    pub(crate) roots: Vec<String>,
    /// Filesystem facts collected by the scan plan.  They are deliberately
    /// retained so audit/validation never need to re-walk the SD card.
    pub(crate) installed_cores: Vec<InstalledCore>,
    pub(crate) game_dir_facts: Vec<GameDirFact>,
    pub(crate) profiles: Vec<LaunchProfile>,
    pub(crate) normal_files: Vec<LibraryPayloadFile>,
    pub(crate) containers: Vec<LibraryContainer>,
    pub(crate) entries: Vec<LibraryContainerEntry>,
    pub(crate) audit_rows: Vec<CatalogAuditRow>,
    pub(crate) ignored_files: usize,
    pub(crate) discoveries: Vec<GameDiscovery>,
    pub(crate) discover_us: u64,
    pub(crate) classify_us: u64,
}

pub struct LibraryCatalogLoad {
    pub catalog: ArcadeCatalog,
    pub stamp: Option<catalog_stamp::CatalogStamp>,
    pub us: u64,
    pub open_us: u64,
    pub schema_check_us: u64,
    pub query_us: u64,
    pub query_prepare_us: u64,
    pub query_first_row_us: u64,
    pub query_row_read_us: u64,
    pub query_row_hydrate_us: u64,
    pub launch_plans_us: u64,
    pub systems_us: u64,
    pub catalog_us: u64,
    pub rows: usize,
}

impl LibraryCatalogLoad {
    pub(crate) fn from_precomputed(catalog: ArcadeCatalog, us: u64) -> Self {
        let rows = catalog.len();
        Self {
            catalog,
            stamp: None,
            us,
            open_us: 0,
            schema_check_us: 0,
            query_us: 0,
            query_prepare_us: 0,
            query_first_row_us: 0,
            query_row_read_us: 0,
            query_row_hydrate_us: 0,
            launch_plans_us: 0,
            systems_us: 0,
            catalog_us: us,
            rows,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LibraryScanStats {
    pub scan_us: u64,
    pub discover_us: u64,
    pub classify_us: u64,
    pub normal_files: usize,
    pub containers: usize,
    pub entries: usize,
    pub audit_rows: usize,
    pub discoveries: usize,
}

pub struct LibraryScanArtifact {
    pub(crate) scan: LibraryScan,
    pub(crate) stats: LibraryScanStats,
    pub(crate) stamp: catalog_stamp::CatalogStamp,
}

pub struct LibraryRamScanArtifact {
    pub(crate) scan: LibraryScan,
    pub(crate) stats: LibraryScanStats,
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

    pub fn stamp(&self) -> &catalog_stamp::CatalogStamp {
        &self.stamp
    }

    pub fn catalog(&self, root: impl AsRef<Path>) -> ArcadeCatalog {
        build_catalog_from_scan(root, &self.scan)
    }

    pub fn arcade_catalog(&self, root: impl AsRef<Path>) -> ArcadeCatalog {
        self.catalog(root)
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

    pub fn save_default_sqlite_with_projections(
        self,
        root: impl AsRef<Path>,
        progress: ProgressCallback<'_>,
    ) -> Result<LibraryRefreshSummary, String> {
        let cfg = BenchConfig::production();
        save_scan_artifact_to_sqlite_with_projections(&cfg, self, root, progress)
    }
}

impl LibraryRamScanArtifact {
    pub fn stats(&self) -> &LibraryScanStats {
        &self.stats
    }

    pub fn catalog(&self, root: impl AsRef<Path>) -> ArcadeCatalog {
        build_catalog_from_scan(root, &self.scan)
    }

    pub fn save_default_sqlite_with_catalog_projection(
        self,
        catalog: &ArcadeCatalog,
        progress: ProgressCallback<'_>,
    ) -> Result<LibraryRefreshSummary, String> {
        let cfg = BenchConfig::production();
        let artifact = self.complete_coverage_audit();
        save_scan_artifact_to_sqlite_with_catalog_projection(&cfg, artifact, catalog, progress)
    }

    pub fn complete_coverage_audit(mut self) -> LibraryScanArtifact {
        let audit_t = std::time::Instant::now();
        self.scan.audit_rows = core_audit::audit_catalog_coverage_from_facts(
            &self.scan.profiles,
            &self.scan.installed_cores,
            &self.scan.game_dir_facts,
        );
        let audit_us = audit_t.elapsed().as_micros() as u64;
        report_library_scan_timing(
            "coverage_audit_deferred",
            audit_us,
            format!("rows={}", self.scan.audit_rows.len()),
        );
        let stamp = catalog_stamp::compute_default_catalog_stamp_with_audit(
            &self.scan.roots,
            &self.scan.audit_rows,
        );
        self.stats.scan_us = self.stats.scan_us.saturating_add(audit_us);
        self.stats.audit_rows = self.scan.audit_rows.len();
        LibraryScanArtifact {
            scan: self.scan,
            stats: self.stats,
            stamp,
        }
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
    pub audit_rows: usize,
    pub discoveries: usize,
}

pub struct LibraryRefreshCatalog {
    pub summary: LibraryRefreshSummary,
    pub catalog: LibraryCatalogLoad,
}

pub use catalog_load_metrics::CatalogLoadCounters;

pub fn reset_catalog_load_counters() {
    catalog_load_metrics::reset();
}

pub fn catalog_load_counters() -> CatalogLoadCounters {
    catalog_load_metrics::snapshot()
}

pub fn catalog_load_counter_detail() -> String {
    catalog_load_metrics::format_snapshot(catalog_load_metrics::snapshot())
}

pub fn record_catalog_worker_cache_load() {
    catalog_load_metrics::record_worker_cache_load();
}

pub fn record_catalog_ui_load() {
    catalog_load_metrics::record_ui_catalog_load();
}

pub fn record_catalog_nav_projection_load() {
    catalog_load_metrics::record_nav_projection_read();
}

pub fn load_arcade_catalog_from_navigation_projection(
    root: impl AsRef<Path>,
    sqlite_path: &Path,
    expected_stamp: &catalog_stamp::CatalogStamp,
) -> Result<Option<LibraryCatalogLoad>, String> {
    let started = std::time::Instant::now();
    let read_t = std::time::Instant::now();
    let Some(projection) = read_catalog_navigation_projection(
        &navigation_path_for_sqlite(sqlite_path),
        expected_stamp,
    )?
    else {
        return Ok(None);
    };
    let read_us = read_t.elapsed().as_micros() as u64;
    let rows = projection.games.len();
    let catalog_t = std::time::Instant::now();
    let catalog =
        ArcadeCatalog::from_navigation_projection(root.as_ref().to_path_buf(), projection);
    let catalog_us = catalog_t.elapsed().as_micros() as u64;
    Ok(Some(LibraryCatalogLoad {
        catalog,
        stamp: Some(expected_stamp.clone()),
        us: started.elapsed().as_micros() as u64,
        open_us: read_us,
        schema_check_us: 0,
        query_us: 0,
        query_prepare_us: 0,
        query_first_row_us: 0,
        query_row_read_us: 0,
        query_row_hydrate_us: 0,
        launch_plans_us: 0,
        systems_us: 0,
        catalog_us,
        rows,
    }))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogStampCheckSummary {
    pub unchanged: bool,
    pub check_us: u64,
    pub compute_us: u64,
    pub open_us: u64,
    pub read_us: u64,
    pub checkpoint_read_us: u64,
    pub checkpoint_compare_us: u64,
    pub compare_us: u64,
    pub stored_fingerprint: Option<String>,
    pub current_fingerprint: String,
    pub stored_checkpoint_fingerprint: Option<String>,
    pub current_checkpoint_fingerprint: String,
    pub stored_lines: usize,
    pub current_lines: usize,
    pub stored_checkpoint_lines: usize,
    pub current_checkpoint_lines: usize,
    pub drift: CatalogDriftSummary,
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
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) path: String,
}

pub fn run_scan_bench() {
    crate::library_cli::run_scan_bench();
}

pub fn run_sqlite_inspect_cli(args: &[String]) -> Result<String, String> {
    crate::library_cli::run_sqlite_inspect_cli(args)
}

pub use sqlite_catalog::{PreviewIndexRefreshRow, PREVIEW_INDEX_REFRESH_TSV_HEADER};

pub fn refresh_default_preview_index_flags(
    label: &str,
) -> Result<Vec<PreviewIndexRefreshRow>, String> {
    sqlite_catalog::refresh_preview_index_flags(label)
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

pub fn repair_catalog_projections_for_catalog(
    sqlite_path: &Path,
    catalog: &ArcadeCatalog,
    stamp: &catalog_stamp::CatalogStamp,
) -> Result<(), String> {
    sqlite_catalog::repair_catalog_projections_for_catalog(sqlite_path, catalog, stamp)
}

#[derive(Clone, Debug)]
pub struct CatalogProjectionRewriteSummary {
    pub games: usize,
    pub load_us: u64,
    pub repair_us: u64,
    pub summary_bytes: u64,
    pub navigation_bytes: u64,
}

pub fn rewrite_default_catalog_projections(
    root: impl AsRef<Path>,
) -> Result<CatalogProjectionRewriteSummary, String> {
    let sqlite_path = default_sqlite_path();
    let loaded = load_arcade_catalog_from_sqlite(root)?;
    let stamp = loaded
        .stamp
        .as_ref()
        .ok_or_else(|| "sqlite catalog has no stamp".to_string())?;
    let repair_t = std::time::Instant::now();
    sqlite_catalog::rewrite_catalog_projections_for_catalog(&sqlite_path, &loaded.catalog, stamp)?;
    let repair_us = repair_t.elapsed().as_micros() as u64;
    let summary_bytes = std::fs::metadata(crate::catalog_summary::summary_path_for_sqlite(
        &sqlite_path,
    ))
    .map(|metadata| metadata.len())
    .unwrap_or(0);
    let navigation_bytes = std::fs::metadata(navigation_path_for_sqlite(&sqlite_path))
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    Ok(CatalogProjectionRewriteSummary {
        games: loaded.catalog.len(),
        load_us: loaded.us,
        repair_us,
        summary_bytes,
        navigation_bytes,
    })
}

pub fn catalog_projection_pair_current(
    sqlite_path: &Path,
    stamp: &catalog_stamp::CatalogStamp,
) -> Result<bool, String> {
    sqlite_catalog::catalog_projection_pair_current(sqlite_path, stamp)
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

pub(crate) fn sqlite_column_exists(
    conn: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, String> {
    sqlite_catalog::sqlite_column_exists(conn, table, column)
}

pub(crate) fn open_sqlite_read_only(path: &Path) -> rusqlite::Result<Connection> {
    sqlite_catalog::open_sqlite_read_only(path)
}

pub fn read_sqlite_catalog_stamp(
    path: &Path,
) -> Result<Option<catalog_stamp::CatalogStamp>, String> {
    sqlite_catalog::read_sqlite_catalog_stamp(path)
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

pub fn scan_default_library_foreground_with_events(
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> Result<LibraryScanArtifact, String> {
    let cfg = BenchConfig::production();
    Ok(CatalogRefreshPipeline::new(&cfg)
        .scan_artifact_foreground_with_events(progress, scan_events))
}

pub fn scan_default_library_ram_foreground_with_events(
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> Result<LibraryRamScanArtifact, String> {
    let cfg = BenchConfig::production();
    Ok(CatalogRefreshPipeline::new(&cfg)
        .scan_ram_artifact_foreground_with_events(progress, scan_events))
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
                None,
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

#[cfg(test)]
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

pub(crate) fn save_scan_artifact_to_sqlite_with_projections(
    cfg: &BenchConfig,
    artifact: LibraryScanArtifact,
    root: impl AsRef<Path>,
    progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    let import_t = std::time::Instant::now();
    let bytes = sqlite_catalog::save_sqlite_scan_with_progress_and_stamp_and_projections(
        &cfg.sqlite_path,
        &artifact.scan,
        &artifact.stamp,
        root.as_ref(),
        progress,
    )?;
    let import_us = import_t.elapsed().as_micros() as u64;
    Ok(LibraryRefreshSummary {
        skipped: false,
        scan_us: artifact.stats.scan_us,
        discover_us: artifact.stats.discover_us,
        classify_us: artifact.stats.classify_us,
        import_us,
        bytes,
        normal_files: artifact.stats.normal_files,
        containers: artifact.stats.containers,
        entries: artifact.stats.entries,
        audit_rows: artifact.stats.audit_rows,
        discoveries: artifact.stats.discoveries,
    })
}

pub(crate) fn save_scan_artifact_to_sqlite_with_catalog_projection(
    cfg: &BenchConfig,
    artifact: LibraryScanArtifact,
    catalog: &ArcadeCatalog,
    progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    let import_t = std::time::Instant::now();
    let bytes = sqlite_catalog::save_sqlite_scan_with_progress_and_stamp_and_catalog_projection(
        &cfg.sqlite_path,
        &artifact.scan,
        &artifact.stamp,
        catalog,
        progress,
    )?;
    let import_us = import_t.elapsed().as_micros() as u64;
    Ok(LibraryRefreshSummary {
        skipped: false,
        scan_us: artifact.stats.scan_us,
        discover_us: artifact.stats.discover_us,
        classify_us: artifact.stats.classify_us,
        import_us,
        bytes,
        normal_files: artifact.stats.normal_files,
        containers: artifact.stats.containers,
        entries: artifact.stats.entries,
        audit_rows: artifact.stats.audit_rows,
        discoveries: artifact.stats.discoveries,
    })
}

pub(crate) fn apply_library_path_map(mut artifact: LibraryScanArtifact) -> LibraryScanArtifact {
    let rules = catalog_config::library_path_map_from_env();
    if rules.is_empty() {
        return artifact;
    }
    remap_library_scan_paths(&mut artifact.scan, &rules);
    artifact.stamp = catalog_stamp::compute_default_catalog_stamp_with_audit(
        &artifact.scan.roots,
        &artifact.scan.audit_rows,
    );
    artifact
}

pub(crate) fn apply_library_path_map_to_ram_artifact(
    mut artifact: LibraryRamScanArtifact,
) -> LibraryRamScanArtifact {
    let rules = catalog_config::library_path_map_from_env();
    if rules.is_empty() {
        return artifact;
    }
    remap_library_scan_paths(&mut artifact.scan, &rules);
    artifact
}

fn remap_library_scan_paths(scan: &mut LibraryScan, rules: &[catalog_config::PathMapRule]) {
    for root in &mut scan.roots {
        *root = catalog_config::map_library_path(root, rules);
    }
    for file in &mut scan.normal_files {
        file.path = catalog_config::map_library_path(&file.path, rules);
    }
    for container in &mut scan.containers {
        container.file_path = catalog_config::map_library_path(&container.file_path, rules);
    }
    for entry in &mut scan.entries {
        entry.file_path = catalog_config::map_library_path(&entry.file_path, rules);
        entry.launch_ref = catalog_config::map_library_path(&entry.launch_ref, rules);
    }
    for row in &mut scan.audit_rows {
        row.core_path = catalog_config::map_library_path(&row.core_path, rules);
        row.expected_game_dir = catalog_config::map_library_path(&row.expected_game_dir, rules);
    }
    for discovery in &mut scan.discoveries {
        discovery.source_path = catalog_config::map_library_path(&discovery.source_path, rules);
        discovery.launch_ref = catalog_config::map_library_path(&discovery.launch_ref, rules);
    }
}

#[cfg(test)]
fn build_arcade_catalog_from_scan(root: impl AsRef<Path>, scan: &LibraryScan) -> ArcadeCatalog {
    let arcade_metadata = crate::software_identity::load_arcade_machine_metadata(
        &default_mame_sqlite_path(),
        &default_hbmame_sqlite_path(),
    );
    build_arcade_catalog_from_scan_with_metadata(root, scan, &arcade_metadata)
}

fn build_catalog_from_scan(root: impl AsRef<Path>, scan: &LibraryScan) -> ArcadeCatalog {
    let mame_sqlite_path = default_mame_sqlite_path();
    let hbmame_sqlite_path = default_hbmame_sqlite_path();
    let preview_paths = PreviewArchivePaths::from_paths(
        preview_worker::preview_archive_paths_for_catalog_projection(),
    );
    build_catalog_from_scan_with_sources(
        root,
        scan,
        &mame_sqlite_path,
        &hbmame_sqlite_path,
        &preview_paths,
        SoftwareHashCache::load(&default_sqlite_path()),
        sqlite_catalog::DiscoveryHistory::load(&default_sqlite_path()),
    )
}

fn build_catalog_from_scan_with_sources(
    root: impl AsRef<Path>,
    scan: &LibraryScan,
    mame_sqlite_path: &Path,
    hbmame_sqlite_path: &Path,
    preview_paths: &PreviewArchivePaths,
    mut software_hash_cache: SoftwareHashCache,
    discovery_history: Option<sqlite_catalog::DiscoveryHistory>,
) -> ArcadeCatalog {
    let covered_payloads = covered_payload_paths(&scan.discoveries);
    let discoveries = preferred_playable_discoveries_by_key(&scan.discoveries, &covered_payloads);
    let arcade_setnames = arcade_metadata_setnames(discoveries.values().copied());
    let software_metadata = load_mame_software_metadata(mame_sqlite_path);
    let arcade_metadata = load_arcade_machine_metadata_for_setnames(
        mame_sqlite_path,
        hbmame_sqlite_path,
        &arcade_setnames,
    );
    let now = unix_now_secs();
    let mut arcade_rows = Vec::<CatalogProjectionRow>::new();
    let mut launcher_rows = Vec::<CatalogProjectionRow>::new();
    let mut launch_plans = Vec::<StructuredLaunchPlan>::new();
    let mut projection_context = CatalogProjectionBuildContext {
        scan,
        software_metadata: &software_metadata,
        arcade_metadata: &arcade_metadata,
        preview_paths,
        software_hash_cache: &mut software_hash_cache,
        discovery_history: discovery_history.as_ref(),
        now,
    };

    for (key, discovery) in discoveries {
        if is_raw_arcade_zip_set_discovery(discovery) {
            continue;
        }
        let Some(projection) = projection_context.projection_for_discovery(&key, discovery) else {
            continue;
        };
        if let Some(plan) = projection.launch_plan {
            launch_plans.push(plan);
        }
        if projection.is_arcade {
            arcade_rows.push(projection.row);
        } else {
            launcher_rows.push(projection.row);
        }
    }

    catalog_from_sqlite_launcher_projection_order(root, arcade_rows, launcher_rows, launch_plans)
}

struct CatalogProjectionBuildContext<'a> {
    scan: &'a LibraryScan,
    software_metadata: &'a MameSoftwareMetadata,
    arcade_metadata: &'a ArcadeMachineMetadata,
    preview_paths: &'a PreviewArchivePaths,
    software_hash_cache: &'a mut SoftwareHashCache,
    discovery_history: Option<&'a sqlite_catalog::DiscoveryHistory>,
    now: i64,
}

struct CatalogProjectionForDiscovery {
    row: CatalogProjectionRow,
    is_arcade: bool,
    launch_plan: Option<StructuredLaunchPlan>,
}

impl CatalogProjectionBuildContext<'_> {
    fn projection_for_discovery(
        &mut self,
        key: &str,
        discovery: &GameDiscovery,
    ) -> Option<CatalogProjectionForDiscovery> {
        let system_id = catalog_system_id_for_discovery(discovery);
        let launch_ref = launch_ref_for_discovery(key, discovery);
        if !is_launcher_launch_ref(&launch_ref) {
            return None;
        }
        let is_arcade = system_id == "arcade" || system_id == "neogeo";
        let launch_plan =
            structured_launch_plan_for_discovery(discovery, &launch_ref, &self.scan.profiles);
        let discovered_at_unix = self
            .discovery_history
            .and_then(|history| history.discovered_at_for(key, self.scan));
        let software_identity = mame_software_identity_for_discovery(
            discovery,
            self.software_metadata,
            self.software_hash_cache,
        );
        let (preview, setname, parent, family_key, metadata) = if is_arcade {
            let (identity_id, family_id, arcade_family_key, metadata) =
                catalog_arcade_projection_fields_for_discovery(
                    key,
                    discovery,
                    self.arcade_metadata,
                );
            let setname = discovery.setname.clone().unwrap_or_default();
            let preview_key = if system_id == "neogeo" {
                setname.clone()
            } else if family_id.is_empty() {
                identity_id.clone()
            } else {
                family_id.clone()
            };
            let preview = if preview_key.is_empty() {
                LauncherPreviewAsset::none()
            } else {
                LauncherPreviewAsset::new(
                    preview_worker::preview_archive_path_for_system(&system_id),
                    preview_key,
                )
            };
            (
                preview,
                if system_id == "neogeo" {
                    setname
                } else {
                    identity_id
                },
                family_id,
                Some(arcade_family_key),
                metadata,
            )
        } else {
            let preview = software_identity
                .as_ref()
                .and_then(|identity| console_preview_asset(identity, self.preview_paths));
            let preview = preview
                .as_ref()
                .map(|asset| {
                    LauncherPreviewAsset::new(
                        preview_worker::preview_archive_path_for_system(&system_id),
                        asset.asset_key.to_string(),
                    )
                })
                .unwrap_or_else(LauncherPreviewAsset::none);
            let family_key = software_identity
                .as_ref()
                .map(|identity| format!("mame-software:{}", identity.family_id));
            (
                preview,
                discovery.setname.clone().unwrap_or_default(),
                discovery.parent.clone().unwrap_or_default(),
                family_key,
                ArcadeGameMetadataKey {
                    year: discovery.year,
                    manufacturer: discovery.manufacturer.clone().unwrap_or_default(),
                    category: discovery.genre.clone().unwrap_or_default(),
                },
            )
        };
        let mut row = CatalogProjectionRow::new(
            discovery.title.clone(),
            launch_ref,
            system_id.clone(),
            preview,
            metadata,
            sqlite_catalog::is_new_discovery(discovered_at_unix, self.now),
            CatalogProjectionSource {
                source_kind: launch_kind_for_discovery(discovery).to_string(),
                setname,
                parent,
                family_key,
            },
        );
        if is_arcade {
            row.game.has_preview = row.game.has_preview
                && self
                    .preview_paths
                    .archive_for_platform(&system_id)
                    .is_some();
        }
        Some(CatalogProjectionForDiscovery {
            row,
            is_arcade,
            launch_plan,
        })
    }
}

fn catalog_from_sqlite_launcher_projection_order(
    root: impl AsRef<Path>,
    mut arcade_rows: Vec<CatalogProjectionRow>,
    mut launcher_rows: Vec<CatalogProjectionRow>,
    mut launch_plans: Vec<StructuredLaunchPlan>,
) -> ArcadeCatalog {
    arcade_rows.sort_by_cached_key(|row| row.game.title.to_ascii_lowercase());
    launcher_rows.sort_by_cached_key(|row| row.game.title.to_ascii_lowercase());
    let mut arcade_rows = catalog_projection::collapse_catalog_variant_rows(arcade_rows);
    catalog_projection::sort_catalog_projection_rows(&mut arcade_rows);
    let mut launcher_rows = catalog_projection::collapse_catalog_variant_rows(launcher_rows);
    catalog_projection::sort_catalog_projection_rows(&mut launcher_rows);
    let mut games: Vec<_> = arcade_rows.into_iter().map(|row| row.game).collect();
    games.extend(launcher_rows.into_iter().map(|row| row.game));
    let visible_refs = games
        .iter()
        .map(|game| game.mra_path.to_string())
        .collect::<HashSet<_>>();
    launch_plans.retain(|plan| visible_refs.contains(plan.launch_ref.as_ref()));
    let systems = arcade_catalog::systems_from_games(&games);
    ArcadeCatalog::new_with_deferred_text_indexes(
        root.as_ref().to_path_buf(),
        games,
        systems,
        launch_plans,
    )
}

fn structured_launch_plan_for_discovery(
    discovery: &GameDiscovery,
    launch_ref: &str,
    profiles: &[LaunchProfile],
) -> Option<StructuredLaunchPlan> {
    if launch_kind_for_discovery(discovery) != "virtual-mgl" {
        return None;
    }
    let profile_id = profile_id_for_discovery(discovery)?;
    let profile = launch_profiles::profile_for_launch_target_id(profiles, profile_id)?;
    let payload_path = discovery.launch_ref.as_str();
    let payload_rule = match discovery.source_kind {
        DiscoverySourceKind::ArchiveEntry => {
            profile.classify_archive_entry(Path::new(payload_path))
        }
        DiscoverySourceKind::PayloadFile => match profile.classify_path(Path::new(payload_path)) {
            launch_profiles::ProfilePathClass::Payload { rule } => Some(rule),
            _ => None,
        },
        _ => None,
    };
    let mount = payload_rule
        .as_ref()
        .map(|rule| rule.mount)
        .unwrap_or_else(|| launch_profiles::MountSpec::mount_image(0));
    Some(StructuredLaunchPlan {
        launch_ref: launch_ref.into(),
        title: discovery.title.clone().into(),
        system_id: catalog_system_id_for_discovery(discovery).into(),
        core_path: profile
            .core_path
            .as_deref()
            .unwrap_or(discovery.core_id.as_str())
            .into(),
        payload_path: payload_path.into(),
        mount_kind: sqlite_catalog::mount_kind_str(mount.kind).into(),
        mount_index: mount.index,
        delay_secs: mount.delay_secs,
    })
}

fn arcade_metadata_setnames<'a>(
    discoveries: impl Iterator<Item = &'a GameDiscovery>,
) -> HashSet<String> {
    discoveries
        .filter_map(mame_identity_for_discovery)
        .collect()
}

fn catalog_arcade_projection_fields_for_discovery(
    game_id: &str,
    discovery: &GameDiscovery,
    arcade_metadata: &ArcadeMachineMetadata,
) -> (String, String, String, ArcadeGameMetadataKey) {
    if let Some(identity_id) = mame_identity_for_discovery(discovery) {
        let (family_id, _, year, manufacturer, category, _) =
            mame_identity_projection(&identity_id, arcade_metadata, discovery.parent.as_deref());
        let parent = if family_id == identity_id {
            String::new()
        } else {
            family_id.clone()
        };
        let family_key = if family_id.is_empty() {
            identity_id.clone()
        } else {
            family_id
        };
        return (
            identity_id,
            parent,
            family_key,
            ArcadeGameMetadataKey {
                year: optional_year_from_metadata(year),
                manufacturer: manufacturer.unwrap_or_default().to_string(),
                category: category.unwrap_or_default().to_string(),
            },
        );
    }
    (
        discovery.setname.clone().unwrap_or_default(),
        discovery.parent.clone().unwrap_or_default(),
        game_id.to_string(),
        ArcadeGameMetadataKey {
            year: discovery.year,
            manufacturer: discovery.manufacturer.clone().unwrap_or_default(),
            category: discovery.genre.clone().unwrap_or_default(),
        },
    )
}

fn optional_year_from_metadata(value: Option<&str>) -> Option<u16> {
    value.and_then(|value| value.parse::<u16>().ok())
}

#[cfg(test)]
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
        let (setname, parent, family_key) =
            catalog_family_fields_for_discovery(&key, discovery, arcade_metadata);
        rows.push(CatalogProjectionRow::new(
            discovery.title.clone(),
            plan_launch_ref,
            system_id,
            LauncherPreviewAsset::none(),
            crate::arcade_catalog::ArcadeGameMetadataKey {
                year: discovery.year,
                manufacturer: discovery.manufacturer.clone().unwrap_or_default(),
                category: discovery.genre.clone().unwrap_or_default(),
            },
            false,
            CatalogProjectionSource {
                source_kind: launch_kind_for_discovery(discovery).to_string(),
                setname,
                parent,
                family_key: Some(family_key),
            },
        ));
    }
    catalog_projection::catalog_from_projection_rows(root, rows)
}

#[cfg(test)]
fn catalog_family_fields_for_discovery(
    game_id: &str,
    discovery: &GameDiscovery,
    arcade_metadata: &ArcadeMachineMetadata,
) -> (String, String, String) {
    if let Some(identity_id) = mame_identity_for_discovery(discovery) {
        let (family_id, _, _, _, _, _) =
            mame_identity_projection(&identity_id, arcade_metadata, discovery.parent.as_deref());
        let parent = if family_id == identity_id {
            String::new()
        } else {
            family_id.clone()
        };
        let family_key = if family_id.is_empty() {
            identity_id.clone()
        } else {
            family_id
        };
        return (identity_id, parent, family_key);
    }
    (
        discovery.setname.clone().unwrap_or_default(),
        discovery.parent.clone().unwrap_or_default(),
        game_id.to_string(),
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
    crate::catalog_logln!("library_scan_timing\t{stage}\t{us}\t{detail}");
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
    use crate::game_discovery::{DiscoveryConfidence, DiscoverySourceKind};
    use crate::media_metadata;
    use crate::software_identity::software_asset_key;
    use crate::sqlite_catalog::{
        write_sqlite_scan_with_mame_and_hbmame, write_sqlite_scan_with_mame_and_preview_pack,
    };
    use crate::test_support::*;

    #[derive(Debug, PartialEq, Eq)]
    struct CatalogGameSnapshot {
        title: String,
        launch_ref: String,
        preview_archive_path: String,
        preview_asset_key: String,
        has_preview: bool,
        system_id: String,
        year: Option<u16>,
        manufacturer: String,
        category: String,
        is_new: bool,
    }

    fn catalog_game_snapshots(catalog: &ArcadeCatalog) -> Vec<CatalogGameSnapshot> {
        catalog
            .games
            .iter()
            .map(|game| CatalogGameSnapshot {
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
            })
            .collect()
    }

    #[test]
    fn sqlite_launcher_projection_order_sorts_after_variant_collapse() {
        let launcher_rows = vec![
            CatalogProjectionRow {
                family_key: Some("mame-software:megadrive:another-world".to_string()),
                ..catalog_entry_row("Another World", "/z/Another World.md")
            },
            catalog_entry_row("Aq Renkan Awa", "/m/Aq Renkan Awa.md"),
            CatalogProjectionRow {
                family_key: Some("mame-software:megadrive:another-world".to_string()),
                ..catalog_entry_row("Out of This World", "/a/Out of This World.md")
            },
        ];

        let catalog = catalog_from_sqlite_launcher_projection_order(
            "/media/fat/_Arcade",
            Vec::new(),
            launcher_rows,
            Vec::new(),
        );
        let titles = catalog
            .system_game_view("amiga")
            .iter()
            .map(|game| game.title.to_string())
            .collect::<Vec<_>>();

        assert_eq!(titles, ["Aq Renkan Awa", "Out of This World"]);
    }

    fn raw_arcade_zip_set(path: &str, setname: &str) -> GameDiscovery {
        GameDiscovery {
            source_path: path.to_string(),
            launch_ref: path.to_string(),
            source_kind: DiscoverySourceKind::PayloadFile,
            title: title_from_path(path),
            category: "Arcade".to_string(),
            platform_id: "arcade".to_string(),
            core_id: "Arcade".to_string(),
            hardware_id: "arcade".to_string(),
            manufacturer: None,
            genre: None,
            year: None,
            setname: Some(setname.to_string()),
            parent: None,
            covered_payload_path: None,
            confidence: crate::game_discovery::DiscoveryConfidence::PayloadPath,
        }
    }

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
        let metadata =
            crate::software_identity::load_arcade_machine_metadata(&mame_db, &PathBuf::new());
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

    #[test]
    fn raw_mame_zip_sets_do_not_create_dead_launcher_rows() {
        let root = unique_temp_dir("raw-mame-zip-fold");
        std::fs::create_dir_all(&root).expect("create temp root");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[("puckman", None, "Puck Man", Some("1980"), Some("Namco"))],
        );
        let mut mra = mra_discovery(1, "Puck Man");
        mra.launch_ref = "/media/fat/_Arcade/Puck Man.mra".to_string();
        mra.source_path = mra.launch_ref.clone();
        mra.setname = Some("puckman".to_string());
        let known_zip = raw_arcade_zip_set("/media/fat/games/mame/puckman.zip", "puckman");
        let unknown_zip =
            raw_arcade_zip_set("/media/fat/games/hbmame/notarealset.zip", "notarealset");
        let scan = sqlite_scan_with_discoveries(vec![mra, known_zip, unknown_zip]);

        assert_eq!(unique_discovery_count(&scan.discoveries), 1);

        let ram_catalog = build_catalog_from_scan_with_sources(
            "/media/fat/_Arcade",
            &scan,
            &mame_db,
            &PathBuf::new(),
            &PreviewArchivePaths::from_paths(Vec::<String>::new()),
            SoftwareHashCache::load(&db),
            None,
        );
        write_sqlite_scan_with_mame(&db, &scan, &mame_db).expect("save sqlite");
        let sqlite_catalog =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load sqlite");
        let conn = open_sqlite_read_only(&db).expect("open sqlite");
        let stored_games: i64 = conn
            .query_row("SELECT count(*) FROM games", [], |row| row.get(0))
            .expect("count games");

        assert_eq!(ram_catalog.system_game_count("arcade"), 1);
        assert_eq!(sqlite_catalog.catalog.system_game_count("arcade"), 1);
        assert_eq!(ram_catalog.games[0].title.as_ref(), "Puck Man");
        assert_eq!(sqlite_catalog.catalog.games[0].title.as_ref(), "Puck Man");
        assert!(!ram_catalog
            .games
            .iter()
            .any(|game| game.mra_path.contains("/games/mame/")
                || game.mra_path.contains("/games/hbmame/")));
        assert_eq!(stored_games, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn survivability_raw_mame_and_hbmame_zip_sets_fold_only_when_launchable() {
        let root = unique_temp_dir("survivability-raw-zip-fold");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        let hbmame_db = root.join("hbmame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[("puckman", None, "Puck Man", Some("1980"), Some("Namco"))],
        );
        write_mame_fixture_db(
            &hbmame_db,
            &[(
                "homebrew",
                None,
                "Homebrew Demo",
                Some("2024"),
                Some("HBMAME"),
            )],
        );
        let mut puckman_mra = mra_discovery(1, "Puck Man");
        puckman_mra.launch_ref = "/media/fat/_Arcade/Puck Man.mra".to_string();
        puckman_mra.source_path = puckman_mra.launch_ref.clone();
        puckman_mra.setname = Some("puckman".to_string());
        let mut homebrew_mra = mra_discovery(2, "Homebrew Demo");
        homebrew_mra.launch_ref = "/media/fat/_Arcade/Homebrew Demo.mra".to_string();
        homebrew_mra.source_path = homebrew_mra.launch_ref.clone();
        homebrew_mra.setname = Some("homebrew".to_string());
        let known_mame_zip = raw_arcade_zip_set("/media/fat/games/mame/puckman.zip", "puckman");
        let known_hbmame_zip =
            raw_arcade_zip_set("/media/fat/games/hbmame/homebrew.zip", "homebrew");
        let unknown_zip =
            raw_arcade_zip_set("/media/fat/games/hbmame/notarealset.zip", "notarealset");
        let scan = sqlite_scan_with_discoveries(vec![
            puckman_mra,
            homebrew_mra,
            known_mame_zip,
            known_hbmame_zip,
            unknown_zip,
        ]);

        write_sqlite_scan_with_mame_and_hbmame(&db, &scan, &mame_db, &hbmame_db)
            .expect("save sqlite");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load sqlite");
        let conn = open_sqlite_read_only(&db).expect("open sqlite");
        let stored_games: i64 = conn
            .query_row("SELECT count(*) FROM games", [], |row| row.get(0))
            .expect("count games");

        assert_eq!(unique_discovery_count(&scan.discoveries), 2);
        assert_eq!(loaded.catalog.system_game_count("arcade"), 2);
        assert_eq!(stored_games, 2);
        assert!(loaded
            .catalog
            .games
            .iter()
            .any(|game| game.title.as_ref() == "Puck Man"));
        assert!(loaded
            .catalog
            .games
            .iter()
            .any(|game| game.title.as_ref() == "Homebrew Demo"));
        assert!(!loaded.catalog.games.iter().any(|game| {
            game.mra_path.contains("/games/mame/") || game.mra_path.contains("/games/hbmame/")
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ram_scan_artifact_completes_coverage_audit_before_save_artifact() {
        let root = unique_temp_dir("ram-scan-deferred-audit");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::write(root.join("_Console/UnknownCore_20260701.rbf"), b"rbf").expect("write core");
        let mut scan = sqlite_scan_with_discoveries(vec![mra_discovery(1, "1942")]);
        scan.roots = vec![root.display().to_string()];
        scan.installed_cores = crate::catalog_discovery::installed_cores_for_roots(&scan.roots);
        scan.game_dir_facts = crate::catalog_discovery::top_level_game_dirs_for_roots(&scan.roots);
        let ram_artifact = LibraryRamScanArtifact {
            stats: LibraryScanStats {
                scan_us: 42,
                discover_us: scan.discover_us,
                classify_us: scan.classify_us,
                normal_files: scan.normal_files.len(),
                containers: scan.containers.len(),
                entries: scan.entries.len(),
                audit_rows: 0,
                discoveries: unique_discovery_count(&scan.discoveries),
            },
            scan,
        };

        assert_eq!(ram_artifact.stats().audit_rows, 0);
        assert_eq!(ram_artifact.catalog("/media/fat/_Arcade").len(), 1);

        let artifact = ram_artifact.complete_coverage_audit();
        assert!(!artifact.scan.audit_rows.is_empty());
        assert_eq!(artifact.stats.audit_rows, artifact.scan.audit_rows.len());
        assert!(artifact.stats.scan_us >= 42);
        assert_eq!(
            artifact.stamp,
            catalog_stamp::compute_default_catalog_stamp_with_audit(
                &artifact.scan.roots,
                &artifact.scan.audit_rows,
            )
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scan_artifact_catalog_matches_sqlite_catalog_for_mixed_fixtures() {
        let root = unique_temp_dir("ram-catalog-mixed-fixtures");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_software_fixture_db(
            &mame_db,
            &[(
                "nes",
                "smb",
                None,
                "Super Mario Bros. (USA)",
                Some("1985"),
                Some("Nintendo"),
                Some("usa"),
            )],
            &[],
        );
        {
            let conn = Connection::open(&mame_db).expect("open mame fixture");
            conn.execute(
                "INSERT INTO mame_machines(setname,parent_setname,title,year,manufacturer)
                 VALUES ('mslug',NULL,'Metal Slug','1996','SNK')",
                [],
            )
            .expect("insert neogeo parent metadata");
            conn.execute(
                "INSERT INTO mame_machines(setname,parent_setname,title,year,manufacturer)
                 VALUES ('mslugh','mslug','Metal Slug (Hack)','1996','SNK')",
                [],
            )
            .expect("insert neogeo clone metadata");
        }
        let pack = preview_worker::PreviewArchiveIndex {
            path: root.join("nes-screenshots.mmlz4b").display().to_string(),
            codec: "mmlz4b",
            entries: vec![software_asset_key("nes", "smb")],
        };
        let preview_paths = PreviewArchivePaths::from_paths(vec![pack.path.clone()]);

        let mut arcade_parent = mra_discovery(1, "Moon Patrol");
        arcade_parent.setname = Some("mpatrol".to_string());
        let mut arcade_clone = mra_discovery(2, "Moon Patrol (Japan)");
        arcade_clone.setname = Some("mpatrolj".to_string());
        arcade_clone.parent = Some("mpatrol".to_string());

        let mut nes_payload = payload("/media/fat/games/NES/Super Mario Bros.nes");
        nes_payload.title = "Super Mario Bros. (USA)".to_string();
        nes_payload.category = "Console".to_string();
        nes_payload.platform_id = "nes".to_string();
        nes_payload.core_id = "NES".to_string();
        nes_payload.hardware_id = "nes".to_string();

        let mut dos_launcher = mgl(
            "/media/fat/_DOS Games/Commander Keen.mgl",
            "/media/fat/_DOS Games/Commander Keen.mgl",
        );
        dos_launcher.title = "Commander Keen".to_string();
        dos_launcher.category = "Computer".to_string();
        dos_launcher.platform_id = "dos".to_string();
        dos_launcher.core_id = "AO486".to_string();
        dos_launcher.hardware_id = "ao486".to_string();

        let mut neogeo_parent = mgl(
            "/media/fat/_Games/_Neo Geo MVS & AES/Metal Slug.mgl",
            "/media/fat/_Games/_Neo Geo MVS & AES/Metal Slug.mgl",
        );
        neogeo_parent.title = "Metal Slug".to_string();
        neogeo_parent.category = "Arcade".to_string();
        neogeo_parent.platform_id = "neogeo".to_string();
        neogeo_parent.core_id = "NeoGeo".to_string();
        neogeo_parent.hardware_id = "neogeo".to_string();
        neogeo_parent.setname = Some("mslug".to_string());

        let mut neogeo_clone = mgl(
            "/media/fat/_Games/_Neo Geo MVS & AES/Metal Slug Hack.mgl",
            "/media/fat/_Games/_Neo Geo MVS & AES/Metal Slug Hack.mgl",
        );
        neogeo_clone.title = "Metal Slug (Hack)".to_string();
        neogeo_clone.category = "Arcade".to_string();
        neogeo_clone.platform_id = "neogeo".to_string();
        neogeo_clone.core_id = "NeoGeo".to_string();
        neogeo_clone.hardware_id = "neogeo".to_string();
        neogeo_clone.setname = Some("mslugh".to_string());

        let mut neogeo_without_setname = mgl(
            "/media/fat/games/NEOGEO/Neo Geo Mister FGPA Ultra Pack/Homebrew/Demos/Bad Apple Demo.neo",
            "/media/fat/games/NEOGEO/Neo Geo Mister FGPA Ultra Pack/Homebrew/Demos/Bad Apple Demo.neo",
        );
        neogeo_without_setname.title = "Bad Apple Demo".to_string();
        neogeo_without_setname.category = "Arcade".to_string();
        neogeo_without_setname.platform_id = "neogeo".to_string();
        neogeo_without_setname.core_id = "NeoGeo".to_string();
        neogeo_without_setname.hardware_id = "neogeo".to_string();
        neogeo_without_setname.source_kind = DiscoverySourceKind::PayloadFile;
        neogeo_without_setname.setname = None;

        let archive_launch_ref =
            "/media/fat/games/NES/Collection.zip/Collection/Legend of Zelda.nes";
        let archive_entry = GameDiscovery {
            source_path: "/media/fat/games/NES/Collection.zip::Collection/Legend of Zelda.nes"
                .to_string(),
            launch_ref: archive_launch_ref.to_string(),
            source_kind: DiscoverySourceKind::ArchiveEntry,
            title: "Legend of Zelda".to_string(),
            category: "Console".to_string(),
            platform_id: "nes".to_string(),
            core_id: "NES".to_string(),
            hardware_id: "nes".to_string(),
            manufacturer: None,
            genre: None,
            year: None,
            setname: None,
            parent: None,
            covered_payload_path: None,
            confidence: DiscoveryConfidence::ArchiveToc,
        };

        let amigavision_game = GameDiscovery {
            source_path: "/media/fat/games/Amiga/AmigaVision.hdf::Alien Breed".to_string(),
            launch_ref: media_metadata::amigavision_game_launch_ref("Alien Breed"),
            source_kind: DiscoverySourceKind::CatalogEntry,
            title: "Alien Breed".to_string(),
            category: "Computer".to_string(),
            platform_id: "amiga".to_string(),
            core_id: "Minimig".to_string(),
            hardware_id: "amiga".to_string(),
            manufacturer: None,
            genre: Some("AmigaVision".to_string()),
            year: None,
            setname: None,
            parent: None,
            covered_payload_path: None,
            confidence: DiscoveryConfidence::CatalogMetadata,
        };

        let scan = sqlite_scan_with_discoveries(vec![
            arcade_parent,
            arcade_clone,
            nes_payload,
            dos_launcher,
            neogeo_parent,
            neogeo_clone,
            neogeo_without_setname,
            archive_entry,
            amigavision_game,
        ]);
        let ram_catalog = build_catalog_from_scan_with_sources(
            "/media/fat/_Arcade",
            &scan,
            &mame_db,
            &PathBuf::new(),
            &preview_paths,
            SoftwareHashCache::load(&db),
            None,
        );
        write_sqlite_scan_with_mame_and_preview_pack(&db, &scan, &mame_db, &pack)
            .expect("save sqlite");
        let sqlite_catalog =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load sqlite");

        assert_eq!(
            catalog_game_snapshots(&ram_catalog),
            catalog_game_snapshots(&sqlite_catalog.catalog)
        );
        assert_eq!(ram_catalog.system_game_count("arcade"), 1);
        assert_eq!(ram_catalog.system_game_count("neogeo"), 2);
        let neogeo_game = ram_catalog
            .games
            .iter()
            .find(|game| game.title.as_ref() == "Metal Slug")
            .expect("neogeo game");
        assert_eq!(neogeo_game.preview_asset_key.as_ref(), "mslug");
        let neogeo_without_preview = ram_catalog
            .games
            .iter()
            .find(|game| game.title.as_ref() == "Bad Apple Demo")
            .expect("neogeo game without setname");
        assert!(!neogeo_without_preview.has_preview);
        assert_eq!(neogeo_without_preview.preview_asset_key.as_ref(), "");
        assert!(ram_catalog
            .games
            .iter()
            .any(|game| game.mra_path.starts_with("magik-amigavision:")));
        let virtual_ref = ram_catalog
            .games
            .iter()
            .find(|game| game.title.as_ref() == "Legend of Zelda")
            .expect("archive entry")
            .mra_path
            .to_string();
        assert!(matches!(
            ram_catalog.launch_target_for_ref(&virtual_ref),
            crate::arcade_catalog::LaunchTarget::Structured(_)
        ));
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
        install_test_console_core(&root, "NES");
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
        install_test_console_core(&root, "NES");
        install_test_console_core(&root, "SNES");
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
        install_test_console_core(&root, "GBA");
        install_test_console_core(&root, "NES");
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
