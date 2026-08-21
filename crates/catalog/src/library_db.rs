// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whole-MiSTer library database scanning and loading.
//!
//! This is deliberately TOC/header-only for archives. Indexing must never
//! decompress full game libraries just to make the launcher searchable.

use crate::arcade_catalog::{
    self, ArcadeCatalog, ArcadeGameMetadataKey, PlatformKind, StructuredLaunchPlan,
    SystemProjectionStats,
};
use crate::catalog_build::CatalogRefreshPipeline;
use crate::catalog_checkpoint::CatalogDriftSummary;
use crate::catalog_config;
pub use crate::catalog_config::{
    default_hbmame_sqlite_path, default_mame_sqlite_path, default_sqlite_path,
};
use crate::catalog_discovery::{GameDirFact, InstalledCore};
use crate::catalog_load_metrics;
pub use crate::catalog_navigation::{
    CatalogNavigationProjection, navigation_path_for_sqlite, read_catalog_navigation_projection,
    read_catalog_navigation_projection_with_timing,
    write_catalog_navigation_projection_for_catalog,
};
pub(crate) use crate::catalog_progress::ProgressCallback;
pub use crate::catalog_progress::{
    CatalogProgress, CatalogProgressPhase, catalog_progress_percent_from_display,
};
pub(crate) use crate::catalog_projection::canonical_variant_title;
use crate::catalog_projection::{
    self, CatalogProjectionRow, CatalogProjectionSource, LauncherPreviewAsset,
};
use crate::catalog_stamp;
use crate::core_audit::{self, CatalogAuditRow};
#[cfg(test)]
use crate::game_discovery::preferred_playable_discoveries_by_key;
use crate::game_discovery::{
    DiscoverySourceKind, GameDiscovery, catalog_system_id_for_discovery, covered_payload_paths,
    is_launcher_launch_ref, is_raw_arcade_zip_set_discovery, launch_kind_for_discovery,
    launch_ref_for_discovery, preferred_playable_discovery_indices_by_key,
    profile_id_for_discovery,
};
use crate::launch_profiles::{self, CollectionListing, LaunchProfile, PayloadRule};
use crate::library_indexer::LibraryIndexer;
use crate::prepared_collections::PreparedCollectionId;
use crate::preview_worker;
use crate::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use crate::software_identity::{
    ArcadeMachineMetadata, MachineMetadataRows, MameSoftwareMetadata, PreviewArchivePaths,
    SoftwareHashCache, console_preview_asset, load_arcade_machine_metadata_for_setnames,
    load_mame_machine_metadata_for_setnames, load_mame_software_metadata,
    mame_identity_for_discovery, mame_identity_projection, mame_software_identity_for_discovery,
    mister_arcade_metadata_for_discovery, software_list_for_platform,
    write_simple_mame_metadata_db,
};
use crate::sqlite_catalog;
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(crate) const MRA_PREFIX_BYTES: usize = 160 * 1024;
pub type ScanEventCallback<'a> = Option<&'a mut dyn FnMut(LibraryScanEvent)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryScanEvent {
    ReconciliationPlanReady {
        system_ids: Vec<String>,
        all_published_systems: bool,
    },
    SystemDiscovered {
        system_id: String,
    },
    SystemScanning {
        system_id: String,
    },
    TargetProgress {
        ordinal: usize,
        total: usize,
        path: String,
        target_kind: String,
        state: String,
        completed_targets: usize,
        discoveries: usize,
        execution_mode: String,
        cooperative_policy: String,
    },
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
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LibraryContainer {
    pub file_path: String,
    pub format: ArchiveFormat,
    pub size: u64,
    pub mtime_secs: i64,
    pub entry_count: u32,
    pub scan_status: ArchiveScanStatus,
    pub scan_us: u64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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

#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize,
)]
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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
    pub(crate) attribution: crate::library_indexer::CatalogScanAttribution,
}

pub struct LibraryCatalogLoad {
    pub catalog: ArcadeCatalog,
    pub stamp: Option<catalog_stamp::CatalogStamp>,
    /// True when this catalog contains enough canonical projection data to
    /// recreate the adjacent summary/navigation pair without semantic loss.
    pub projection_repair_safe: bool,
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
    pub navigation_file_read_us: u64,
    pub navigation_decompress_us: u64,
    pub navigation_decode_us: u64,
    pub rows: usize,
}

impl LibraryCatalogLoad {
    pub fn from_precomputed(catalog: ArcadeCatalog, us: u64) -> Self {
        let rows = catalog.len();
        Self {
            catalog,
            stamp: None,
            projection_repair_safe: true,
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
            navigation_file_read_us: 0,
            navigation_decompress_us: 0,
            navigation_decode_us: 0,
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

pub struct LibraryPreparedState {
    pub(crate) catalog_state: crate::catalog_state::CatalogState,
    pub(crate) stats: LibraryScanStats,
}

impl LibraryPreparedState {
    pub fn stats(&self) -> &LibraryScanStats {
        &self.stats
    }

    pub fn stamp(&self) -> &catalog_stamp::CatalogStamp {
        &self.catalog_state.stamp
    }

    pub fn into_parts(self) -> (crate::catalog_state::CatalogState, LibraryScanStats) {
        (self.catalog_state, self.stats)
    }
}

#[derive(Clone)]
pub struct LibraryRamScanArtifact {
    pub(crate) scan: LibraryScan,
    pub(crate) stats: LibraryScanStats,
    pub(crate) preferred_discoveries: BTreeMap<String, usize>,
}

pub struct LibraryAuditedScanArtifact {
    scan: LibraryScan,
    stats: LibraryScanStats,
    preferred_discoveries: BTreeMap<String, usize>,
    catalog_state: crate::catalog_state::CatalogState,
    audit_us: u64,
    stamp_us: u64,
    audit_stamp_worker_us: u64,
}

impl LibraryAuditedScanArtifact {
    pub fn catalog_state(&self) -> &crate::catalog_state::CatalogState {
        &self.catalog_state
    }

    pub fn stats(&self) -> &LibraryScanStats {
        &self.stats
    }

    pub fn complete_catalog_background_with_progress(
        mut self,
        root: impl AsRef<Path>,
        progress: &mut dyn FnMut(&str, &str),
    ) -> Result<
        (
            LibraryPreparedState,
            ArcadeCatalog,
            CatalogPrepareTiming,
            crate::scanner_cache::ScannerCacheState,
        ),
        String,
    > {
        apply_runtime_thread_policy(RuntimeThreadRole::CatalogWorker);
        let wall_t = std::time::Instant::now();
        release_non_projection_scan_facts(&mut self.scan);
        let (catalog, timing, scanner_cache) = build_catalog_from_scan_with_preferred_and_progress(
            root.as_ref(),
            &self.scan,
            &self.preferred_discoveries,
            progress,
        );
        Ok((
            LibraryPreparedState {
                catalog_state: self.catalog_state,
                stats: self.stats,
            },
            catalog,
            CatalogPrepareTiming {
                audit_us: self.audit_us,
                stamp_us: self.stamp_us,
                audit_stamp_worker_us: self.audit_stamp_worker_us,
                catalog_us: timing.total_us,
                metadata_us: timing.metadata_us,
                projection_rows_us: timing.projection_rows_us,
                indexes_us: timing.indexes_us,
                wall_us: wall_t.elapsed().as_micros() as u64,
                overlapped_us: 0,
            },
            scanner_cache,
        ))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CatalogPrepareTiming {
    pub audit_us: u64,
    pub stamp_us: u64,
    pub audit_stamp_worker_us: u64,
    pub catalog_us: u64,
    pub metadata_us: u64,
    pub projection_rows_us: u64,
    pub indexes_us: u64,
    pub wall_us: u64,
    pub overlapped_us: u64,
}

const CATALOG_PREPARE_WORKER_ROLE: RuntimeThreadRole = RuntimeThreadRole::CatalogForeground;

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

    pub fn catalog_state(&self) -> crate::catalog_state::CatalogState {
        catalog_state_from_scan(&self.scan, &self.stats, self.stamp.clone())
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

    pub fn save_default_sqlite_with_catalog_projection(
        self,
        catalog: &ArcadeCatalog,
        progress: ProgressCallback<'_>,
    ) -> Result<LibraryRefreshSummary, String> {
        let cfg = BenchConfig::production();
        save_scan_artifact_to_sqlite_with_catalog_projection(&cfg, self, catalog, progress)
    }
}

impl LibraryRamScanArtifact {
    pub fn stats(&self) -> &LibraryScanStats {
        &self.stats
    }

    pub fn scan_attribution_detail(&self) -> String {
        self.scan.attribution.compact_detail()
    }

    pub fn catalog(&self, root: impl AsRef<Path>) -> ArcadeCatalog {
        build_catalog_from_scan(root, &self.scan)
    }

    pub fn complete_coverage_audit_for_decision(mut self) -> LibraryAuditedScanArtifact {
        apply_runtime_thread_policy(RuntimeThreadRole::CatalogWorker);
        let worker_t = std::time::Instant::now();
        let (audit_rows, stamp, audit_us, stamp_us) = coverage_audit_and_stamp(&self.scan);
        let audit_stamp_worker_us = worker_t.elapsed().as_micros() as u64;
        self.scan.audit_rows = audit_rows;
        report_library_scan_timing(
            "coverage_audit_deferred",
            audit_us,
            format!("rows={}", self.scan.audit_rows.len()),
        );
        self.stats.scan_us = self.stats.scan_us.saturating_add(audit_us);
        self.stats.audit_rows = self.scan.audit_rows.len();
        let catalog_state = catalog_state_from_scan(&self.scan, &self.stats, stamp);
        LibraryAuditedScanArtifact {
            scan: self.scan,
            stats: self.stats,
            preferred_discoveries: self.preferred_discoveries,
            catalog_state,
            audit_us,
            stamp_us,
            audit_stamp_worker_us,
        }
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
        let (audit_rows, stamp, audit_us, _stamp_us) = coverage_audit_and_stamp(&self.scan);
        self.scan.audit_rows = audit_rows;
        report_library_scan_timing(
            "coverage_audit_deferred",
            audit_us,
            format!("rows={}", self.scan.audit_rows.len()),
        );
        self.stats.scan_us = self.stats.scan_us.saturating_add(audit_us);
        self.stats.audit_rows = self.scan.audit_rows.len();
        LibraryScanArtifact {
            scan: self.scan,
            stats: self.stats,
            stamp,
        }
    }

    /// Completes the exact deferred audit/stamp generation while the already
    /// foreground coordinator builds the RAM catalog on the other Cortex-A9.
    /// The scoped worker is joined before either result can escape, so the
    /// returned catalog and stamp retain the same generation boundary as the
    /// sequential path.
    pub fn complete_coverage_audit_and_catalog_foreground(
        self,
        root: impl AsRef<Path>,
    ) -> Result<
        (
            LibraryPreparedState,
            ArcadeCatalog,
            CatalogPrepareTiming,
            crate::scanner_cache::ScannerCacheState,
        ),
        String,
    > {
        self.complete_coverage_audit_and_catalog_foreground_with_progress(root, &mut |_, _| {})
    }

    pub fn complete_coverage_audit_and_catalog_foreground_with_progress(
        mut self,
        root: impl AsRef<Path>,
        progress: &mut dyn FnMut(&str, &str),
    ) -> Result<
        (
            LibraryPreparedState,
            ArcadeCatalog,
            CatalogPrepareTiming,
            crate::scanner_cache::ScannerCacheState,
        ),
        String,
    > {
        let root = root.as_ref().to_path_buf();
        apply_runtime_thread_policy(CATALOG_PREPARE_WORKER_ROLE);
        let wall_t = std::time::Instant::now();
        let (audit_rows, stamp, audit_us, stamp_us, audit_stamp_worker_us) =
            std::thread::scope(|scope| {
                let scan = &self.scan;
                let audit_worker = std::thread::Builder::new()
                    .name("catalog-audit".to_string())
                    .spawn_scoped(scope, move || {
                        let worker_t = std::time::Instant::now();
                        apply_runtime_thread_policy(CATALOG_PREPARE_WORKER_ROLE);
                        let (audit_rows, stamp, audit_us, stamp_us) =
                            coverage_audit_and_stamp(scan);
                        (
                            audit_rows,
                            stamp,
                            audit_us,
                            stamp_us,
                            worker_t.elapsed().as_micros() as u64,
                        )
                    })
                    .map_err(|error| format!("spawn catalog audit/stamp worker: {error}"))?;

                audit_worker
                    .join()
                    .map_err(|_| "catalog audit/stamp worker panicked".to_string())
            })?;
        self.scan.audit_rows = audit_rows;
        report_library_scan_timing(
            "coverage_audit_deferred",
            audit_us,
            format!("rows={}", self.scan.audit_rows.len()),
        );
        self.stats.scan_us = self.stats.scan_us.saturating_add(audit_us);
        self.stats.audit_rows = self.scan.audit_rows.len();
        let catalog_state = catalog_state_from_scan(&self.scan, &self.stats, stamp);
        release_non_projection_scan_facts(&mut self.scan);
        let (catalog, timing, scanner_cache) = build_catalog_from_scan_with_preferred_and_progress(
            &root,
            &self.scan,
            &self.preferred_discoveries,
            progress,
        );
        let wall_us = wall_t.elapsed().as_micros() as u64;
        Ok((
            LibraryPreparedState {
                catalog_state,
                stats: self.stats,
            },
            catalog,
            CatalogPrepareTiming {
                audit_us,
                stamp_us,
                audit_stamp_worker_us,
                catalog_us: timing.total_us,
                metadata_us: timing.metadata_us,
                projection_rows_us: timing.projection_rows_us,
                indexes_us: timing.indexes_us,
                wall_us,
                overlapped_us: 0,
            },
            scanner_cache,
        ))
    }

    /// Prepare a replacement catalog while a usable generation remains live.
    /// Work stays on the background catalog policy and runs sequentially so it
    /// cannot occupy both Cortex-A9 cores during interactive UI rendering.
    pub fn complete_coverage_audit_and_catalog_background_with_progress(
        mut self,
        root: impl AsRef<Path>,
        progress: &mut dyn FnMut(&str, &str),
    ) -> Result<
        (
            LibraryPreparedState,
            ArcadeCatalog,
            CatalogPrepareTiming,
            crate::scanner_cache::ScannerCacheState,
        ),
        String,
    > {
        apply_runtime_thread_policy(RuntimeThreadRole::CatalogWorker);
        let wall_t = std::time::Instant::now();
        let audit_worker_t = std::time::Instant::now();
        let (audit_rows, stamp, audit_us, stamp_us) = coverage_audit_and_stamp(&self.scan);
        let audit_stamp_worker_us = audit_worker_t.elapsed().as_micros() as u64;
        self.scan.audit_rows = audit_rows;
        report_library_scan_timing(
            "coverage_audit_deferred",
            audit_us,
            format!("rows={}", self.scan.audit_rows.len()),
        );
        self.stats.scan_us = self.stats.scan_us.saturating_add(audit_us);
        self.stats.audit_rows = self.scan.audit_rows.len();
        let catalog_state = catalog_state_from_scan(&self.scan, &self.stats, stamp);
        release_non_projection_scan_facts(&mut self.scan);
        let (catalog, timing, scanner_cache) = build_catalog_from_scan_with_preferred_and_progress(
            root.as_ref(),
            &self.scan,
            &self.preferred_discoveries,
            progress,
        );
        let wall_us = wall_t.elapsed().as_micros() as u64;
        Ok((
            LibraryPreparedState {
                catalog_state,
                stats: self.stats,
            },
            catalog,
            CatalogPrepareTiming {
                audit_us,
                stamp_us,
                audit_stamp_worker_us,
                catalog_us: timing.total_us,
                metadata_us: timing.metadata_us,
                projection_rows_us: timing.projection_rows_us,
                indexes_us: timing.indexes_us,
                wall_us,
                overlapped_us: 0,
            },
            scanner_cache,
        ))
    }
}

fn catalog_state_from_scan(
    scan: &LibraryScan,
    stats: &LibraryScanStats,
    stamp: catalog_stamp::CatalogStamp,
) -> crate::catalog_state::CatalogState {
    crate::catalog_state::CatalogState {
        stamp,
        checkpoint: crate::catalog_checkpoint::compute_catalog_discovery_checkpoint_from_facts(
            &scan.roots,
            &default_mame_sqlite_path(),
            &default_hbmame_sqlite_path(),
            &scan.audit_rows,
            &scan.installed_cores,
            &scan.game_dir_facts,
        ),
        stats: crate::catalog_state::CatalogStateStats {
            normal_files: stats.normal_files,
            containers: stats.containers,
            entries: stats.entries,
            audit_rows: stats.audit_rows,
            discoveries: stats.discoveries,
        },
    }
}

fn release_non_projection_scan_facts(scan: &mut LibraryScan) {
    scan.roots = Vec::new();
    scan.installed_cores = Vec::new();
    scan.game_dir_facts = Vec::new();
    scan.normal_files = Vec::new();
    scan.containers = Vec::new();
    scan.entries = Vec::new();
    scan.audit_rows = Vec::new();
}

fn coverage_audit_and_stamp(
    scan: &LibraryScan,
) -> (Vec<CatalogAuditRow>, catalog_stamp::CatalogStamp, u64, u64) {
    crate::cooperative_work::checkpoint();
    let audit_t = std::time::Instant::now();
    let audit_rows = core_audit::audit_catalog_coverage_from_facts(
        &scan.roots,
        &scan.profiles,
        &scan.installed_cores,
        &scan.game_dir_facts,
    );
    crate::cooperative_work::checkpoint();
    let audit_us = audit_t.elapsed().as_micros() as u64;
    let stamp_t = std::time::Instant::now();
    let stamp = catalog_stamp::compute_default_catalog_stamp_with_audit(&scan.roots, &audit_rows);
    let stamp_us = stamp_t.elapsed().as_micros() as u64;
    (audit_rows, stamp, audit_us, stamp_us)
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

pub fn is_catalog_schema_mismatch_error(error: &str) -> bool {
    error.starts_with("catalog schema mismatch:")
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
    let Some(loaded_projection) = read_catalog_navigation_projection_with_timing(
        &navigation_path_for_sqlite(sqlite_path),
        expected_stamp,
    )?
    else {
        return Ok(None);
    };
    let read_us = read_t.elapsed().as_micros() as u64;
    let rows = loaded_projection.projection.games.len();
    let catalog_t = std::time::Instant::now();
    let catalog = ArcadeCatalog::from_navigation_projection(
        root.as_ref().to_path_buf(),
        loaded_projection.projection,
    );
    let catalog_us = catalog_t.elapsed().as_micros() as u64;
    Ok(Some(LibraryCatalogLoad {
        catalog,
        stamp: Some(expected_stamp.clone()),
        projection_repair_safe: true,
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
        navigation_file_read_us: loaded_projection.file_read_us,
        navigation_decompress_us: loaded_projection.decompress_us,
        navigation_decode_us: loaded_projection.decode_us,
        rows,
    }))
}

pub fn load_arcade_catalog_from_snapshot(
    root: impl AsRef<Path>,
    path: &Path,
) -> Result<LibraryCatalogLoad, String> {
    let started = std::time::Instant::now();
    let projection = crate::catalog_navigation::read_catalog_navigation_snapshot(path)?;
    let catalog =
        ArcadeCatalog::from_navigation_projection(root.as_ref().to_path_buf(), projection);
    Ok(LibraryCatalogLoad::from_precomputed(
        catalog,
        started.elapsed().as_micros() as u64,
    ))
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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct LibraryPayloadFile {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) path: String,
}

pub fn run_scan_bench_with_config(
    paths: &crate::device_layout::CatalogPaths,
    archive_cache: &crate::catalog_config::ArchiveCacheConfig,
) {
    crate::library_cli::run_scan_bench_with_config(paths, archive_cache);
}

pub fn run_sqlite_inspect_cli(args: &[String]) -> Result<String, String> {
    crate::library_cli::run_sqlite_inspect_cli(args)
}

pub fn run_sqlite_inspect_cli_with_paths(
    args: &[String],
    paths: &crate::device_layout::CatalogPaths,
) -> Result<String, String> {
    crate::library_cli::run_sqlite_inspect_cli_with_default(
        args,
        paths.library_sqlite().to_path_buf(),
    )
}

pub use sqlite_catalog::{PREVIEW_INDEX_REFRESH_TSV_HEADER, PreviewIndexRefreshRow};

pub fn refresh_default_preview_index_flags(
    label: &str,
) -> Result<Vec<PreviewIndexRefreshRow>, String> {
    sqlite_catalog::refresh_preview_index_flags(label)
}

pub fn remove_default_sqlite_database() -> Result<(), String> {
    sqlite_catalog::remove_default_sqlite_database()
}

pub fn remove_default_catalog_artifacts() -> Result<usize, String> {
    sqlite_catalog::remove_default_catalog_artifacts()
}

#[cfg(feature = "builder")]
pub(crate) fn remove_catalog_artifacts_with_config(
    paths: &crate::device_layout::CatalogPaths,
    archive_cache: &crate::catalog_config::ArchiveCacheConfig,
) -> Result<usize, String> {
    sqlite_catalog::remove_catalog_artifacts_with_cache_paths(
        paths.library_sqlite(),
        archive_cache.sqlite_build_dir(),
    )
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

/// Hydrate from the retained relational/materialized rows, deliberately
/// bypassing the embedded navigation recovery cache.
pub fn load_arcade_catalog_from_materialized_sqlite(
    root: impl AsRef<Path>,
) -> Result<LibraryCatalogLoad, String> {
    sqlite_catalog::load_arcade_catalog_from_materialized_sqlite(root)
}

pub fn load_arcade_catalog_from_materialized_sqlite_at(
    root: impl AsRef<Path>,
    sqlite_path: &Path,
) -> Result<LibraryCatalogLoad, String> {
    sqlite_catalog::load_arcade_catalog_from_materialized_sqlite_at(root, sqlite_path)
}

pub fn repair_catalog_projections_for_catalog(
    sqlite_path: &Path,
    catalog: &ArcadeCatalog,
    stamp: &catalog_stamp::CatalogStamp,
) -> Result<(), String> {
    sqlite_catalog::repair_catalog_projections_for_catalog(sqlite_path, catalog, stamp)
}

pub fn catalog_projection_filter_mismatches(
    sqlite_path: &Path,
    catalog: &ArcadeCatalog,
    stamp: &catalog_stamp::CatalogStamp,
) -> Result<Vec<String>, String> {
    sqlite_catalog::catalog_projection_filter_mismatches(sqlite_path, catalog, stamp)
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
    rewrite_catalog_projections_from_sqlite(root, &sqlite_path)
}

pub(crate) fn rewrite_catalog_projections_from_sqlite(
    root: impl AsRef<Path>,
    sqlite_path: &Path,
) -> Result<CatalogProjectionRewriteSummary, String> {
    let loaded =
        sqlite_catalog::load_arcade_catalog_from_materialized_sqlite_at(root, sqlite_path)?;
    if !loaded.projection_repair_safe {
        return Err(
            "refusing to rewrite catalog projections from degraded joined-SQL fallback".to_string(),
        );
    }
    let stamp = loaded
        .stamp
        .as_ref()
        .ok_or_else(|| "sqlite catalog has no stamp".to_string())?;
    let repair_t = std::time::Instant::now();
    sqlite_catalog::rewrite_catalog_projections_for_catalog(sqlite_path, &loaded.catalog, stamp)?;
    let repair_us = repair_t.elapsed().as_micros() as u64;
    let summary_bytes =
        std::fs::metadata(crate::catalog_summary::summary_path_for_sqlite(sqlite_path))
            .map(|metadata| metadata.len())
            .unwrap_or(0);
    let navigation_bytes = std::fs::metadata(navigation_path_for_sqlite(sqlite_path))
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
    durable_resume: bool,
) -> Result<LibraryRamScanArtifact, String> {
    let cfg = BenchConfig::production();
    Ok(CatalogRefreshPipeline::new(&cfg)
        .scan_ram_artifact_foreground_with_events_and_durable_resume(
            progress,
            scan_events,
            durable_resume,
        ))
}

#[cfg(feature = "builder")]
pub(crate) fn scan_library_ram_foreground_with_paths(
    paths: &crate::device_layout::CatalogPaths,
    archive_cache: &crate::catalog_config::ArchiveCacheConfig,
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
    durable_resume: bool,
) -> Result<LibraryRamScanArtifact, String> {
    let cfg = BenchConfig::production_with_paths(paths);
    Ok(
        CatalogRefreshPipeline::with_archive_cache(&cfg, archive_cache)
            .scan_ram_artifact_foreground_with_events_and_durable_resume(
                progress,
                scan_events,
                durable_resume,
            ),
    )
}

pub fn scan_library_ram_foreground_with_roots(
    roots: Vec<PathBuf>,
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
    durable_resume: bool,
) -> Result<LibraryRamScanArtifact, String> {
    let cfg = BenchConfig {
        roots: roots
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        sqlite_path: PathBuf::new(),
    };
    Ok(CatalogRefreshPipeline::new(&cfg)
        .scan_ram_artifact_foreground_with_events_and_durable_resume(
            progress,
            scan_events,
            durable_resume,
        ))
}

/// Scan only the first visible Arcade collection for cold-start publication.
/// The authoritative full scan follows after the launcher acknowledges this
/// compact generation, so this artifact is never persisted as the final
/// catalog database.
pub fn scan_arcade_bootstrap_ram_foreground_with_events(
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> Result<LibraryRamScanArtifact, String> {
    let cfg = BenchConfig {
        roots: vec![crate::arcade_catalog::DEFAULT_ARCADE_ROOT.to_string()],
        sqlite_path: default_sqlite_path(),
    };
    Ok(CatalogRefreshPipeline::new(&cfg)
        .scan_ram_artifact_foreground_with_events(progress, scan_events))
}

#[cfg(feature = "builder")]
pub(crate) fn scan_arcade_bootstrap_ram_foreground_with_paths(
    paths: &crate::device_layout::CatalogPaths,
    archive_cache: &crate::catalog_config::ArchiveCacheConfig,
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> Result<LibraryRamScanArtifact, String> {
    let cfg = BenchConfig {
        roots: vec![crate::arcade_catalog::DEFAULT_ARCADE_ROOT.to_string()],
        sqlite_path: paths.library_sqlite().to_path_buf(),
    };
    Ok(
        CatalogRefreshPipeline::with_archive_cache(&cfg, archive_cache)
            .with_arcade_updater_index(paths.arcade_updater_index())
            .scan_ram_artifact_foreground_with_events(progress, scan_events),
    )
}

/// CPU0-confined variant used while the first-run animation owns CPU1.
pub fn scan_arcade_bootstrap_ram_background_with_events(
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> Result<LibraryRamScanArtifact, String> {
    let cfg = BenchConfig {
        roots: vec![crate::arcade_catalog::DEFAULT_ARCADE_ROOT.to_string()],
        sqlite_path: default_sqlite_path(),
    };
    Ok(
        CatalogRefreshPipeline::new(&cfg).scan_ram_artifact_with_events_and_durable_resume(
            progress,
            scan_events,
            false,
        ),
    )
}

#[cfg(feature = "builder")]
pub(crate) fn scan_arcade_bootstrap_ram_background_with_paths(
    paths: &crate::device_layout::CatalogPaths,
    archive_cache: &crate::catalog_config::ArchiveCacheConfig,
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> Result<LibraryRamScanArtifact, String> {
    let cfg = BenchConfig {
        roots: vec![crate::arcade_catalog::DEFAULT_ARCADE_ROOT.to_string()],
        sqlite_path: paths.library_sqlite().to_path_buf(),
    };
    Ok(
        CatalogRefreshPipeline::with_archive_cache(&cfg, archive_cache)
            .with_arcade_updater_index(paths.arcade_updater_index())
            .scan_ram_artifact_with_events_and_durable_resume(progress, scan_events, false),
    )
}

pub fn scan_default_library_ram_background_with_events(
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
    durable_resume: bool,
) -> Result<LibraryRamScanArtifact, String> {
    let cfg = BenchConfig::production();
    Ok(
        CatalogRefreshPipeline::new(&cfg).scan_ram_artifact_with_events_and_durable_resume(
            progress,
            scan_events,
            durable_resume,
        ),
    )
}

#[cfg(feature = "builder")]
pub(crate) fn scan_library_ram_background_with_paths(
    paths: &crate::device_layout::CatalogPaths,
    archive_cache: &crate::catalog_config::ArchiveCacheConfig,
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
    durable_resume: bool,
) -> Result<LibraryRamScanArtifact, String> {
    let cfg = BenchConfig::production_with_paths(paths);
    Ok(
        CatalogRefreshPipeline::with_archive_cache(&cfg, archive_cache)
            .scan_ram_artifact_with_events_and_durable_resume(
                progress,
                scan_events,
                durable_resume,
            ),
    )
}

pub fn scan_default_library_ram_background_reusing_arcade_with_events(
    arcade: LibraryRamScanArtifact,
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> Result<LibraryRamScanArtifact, String> {
    let cfg = BenchConfig::production();
    Ok(
        CatalogRefreshPipeline::new(&cfg).scan_ram_artifact_with_reused_prefix(
            arcade,
            vec![PathBuf::from(crate::arcade_catalog::DEFAULT_ARCADE_ROOT)],
            progress,
            scan_events,
        ),
    )
}

pub fn scan_default_library_ram_foreground_reusing_arcade_with_events(
    arcade: LibraryRamScanArtifact,
    progress: ProgressCallback<'_>,
    scan_events: ScanEventCallback<'_>,
) -> Result<LibraryRamScanArtifact, String> {
    let cfg = BenchConfig::production();
    Ok(
        CatalogRefreshPipeline::new(&cfg).scan_ram_artifact_foreground_with_reused_prefix(
            arcade,
            vec![PathBuf::from(crate::arcade_catalog::DEFAULT_ARCADE_ROOT)],
            progress,
            scan_events,
        ),
    )
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
    let rows = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("read hbmame metadata rows: {e}"))?;
    let setnames = rows
        .iter()
        .flat_map(|(setname, parent, ..)| [normalize_id(setname), normalize_id(parent)])
        .filter(|setname| !setname.is_empty())
        .collect::<HashSet<_>>();
    let mame_metadata = load_mame_machine_metadata_for_setnames(
        &hbmame_path.with_file_name("mame.sqlite3"),
        &setnames,
    );
    let mut machines: MachineMetadataRows = BTreeMap::new();
    for row in rows {
        let (setname, parent, title, year, manufacturer) = row;
        let identity_id = normalize_id(&setname);
        let family_id = normalize_id(&parent);
        if identity_id.is_empty() || family_id.is_empty() || identity_id == family_id {
            continue;
        }
        let controls = mame_metadata
            .get(&identity_id)
            .or_else(|| mame_metadata.get(&family_id));
        machines.entry(identity_id).or_insert_with(|| {
            (
                family_id,
                title,
                year.map(|value| value.to_string()),
                manufacturer,
                controls.and_then(|metadata| metadata.players),
                controls.and_then(|metadata| metadata.control.clone()),
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
    let state_path = crate::catalog_state::default_path();
    sqlite_catalog::catalog_state_stamp_check(&cfg, &state_path)
}

#[cfg(feature = "builder")]
pub(crate) fn sqlite_catalog_stamp_check_with_paths(
    paths: &crate::device_layout::CatalogPaths,
) -> Result<CatalogStampCheckSummary, String> {
    let cfg = BenchConfig::production_with_paths(paths);
    let state_path = crate::catalog_state::path_for_root(paths.sharded_catalog_dir());
    sqlite_catalog::catalog_state_stamp_check(&cfg, &state_path)
}

pub fn default_sqlite_cached_summary(scan_us: u64) -> Result<LibraryRefreshSummary, String> {
    sqlite_cached_summary(&default_sqlite_path(), scan_us)
}

pub fn default_sharded_cached_summary(scan_us: u64) -> Result<LibraryRefreshSummary, String> {
    let storage = crate::catalog_config::default_sharded_catalog_path();
    sharded_cached_summary(&storage, scan_us)
}

pub(crate) fn sharded_cached_summary(
    storage: &Path,
    scan_us: u64,
) -> Result<LibraryRefreshSummary, String> {
    let state_path = crate::catalog_state::path_for_root(storage);
    let state = crate::catalog_state::read(&state_path)?;
    let manifest = crate::shard_registry::read_latest_manifest_lazy(
        storage,
        crate::shard_registry::production_registry_limits(),
    )
    .map_err(|error| format!("read cached V3 manifest: {error}"))?;
    let shard_bytes = manifest.systems.iter().try_fold(0u64, |total, system| {
        total
            .checked_add(system.active.sqlite_bytes)
            .and_then(|value| value.checked_add(system.active.navigation_bytes))
            .ok_or_else(|| "cached V3 byte count overflow".to_string())
    })?;
    let state_bytes = std::fs::metadata(&state_path)
        .map_err(|error| format!("stat cached V3 state: {error}"))?
        .len();
    Ok(LibraryRefreshSummary {
        skipped: true,
        scan_us,
        discover_us: 0,
        classify_us: 0,
        import_us: 0,
        bytes: shard_bytes.saturating_add(state_bytes),
        normal_files: state.stats.normal_files,
        containers: state.stats.containers,
        entries: state.stats.entries,
        audit_rows: state.stats.audit_rows,
        discoveries: state.stats.discoveries,
    })
}

pub(crate) fn rebuild_sqlite_database_with_archive_config(
    cfg: &BenchConfig,
    archive_cache: &crate::catalog_config::ArchiveCacheConfig,
    progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    CatalogRefreshPipeline::with_archive_cache(cfg, archive_cache)
        .rebuild_with_events(progress, None)
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
            .unwrap_or_else(|_| {
                crate::device_layout::current_app_path("library-scan-bench.sqlite3")
            });
        Self { roots, sqlite_path }
    }

    pub(crate) fn from_paths(paths: &crate::device_layout::CatalogPaths) -> Self {
        Self {
            roots: catalog_config::library_roots_from_env(),
            sqlite_path: paths.library_bench_sqlite().to_path_buf(),
        }
    }

    fn production() -> Self {
        let mut cfg = Self::from_env();
        cfg.sqlite_path = default_sqlite_path();
        cfg
    }

    #[cfg(feature = "builder")]
    fn production_with_paths(paths: &crate::device_layout::CatalogPaths) -> Self {
        Self {
            roots: catalog_config::library_roots_from_env(),
            sqlite_path: paths.library_sqlite().to_path_buf(),
        }
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

#[cfg(any(test, feature = "builder"))]
pub(crate) fn scan_library_artifact(
    cfg: &BenchConfig,
    progress: ProgressCallback<'_>,
) -> LibraryScanArtifact {
    CatalogRefreshPipeline::new(cfg).scan_artifact(progress)
}

pub(crate) fn scan_library_artifact_with_archive_config(
    cfg: &BenchConfig,
    archive_cache: &crate::catalog_config::ArchiveCacheConfig,
    progress: ProgressCallback<'_>,
) -> LibraryScanArtifact {
    CatalogRefreshPipeline::with_archive_cache(cfg, archive_cache).scan_artifact(progress)
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

pub(crate) fn save_scan_artifact_to_sqlite_for_bench(
    cfg: &BenchConfig,
    artifact: LibraryScanArtifact,
    progress: ProgressCallback<'_>,
    iteration: usize,
) -> Result<LibraryRefreshSummary, String> {
    CatalogRefreshPipeline::new(cfg).save_artifact_for_bench(artifact, progress, iteration)
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
    artifact: LibraryRamScanArtifact,
) -> LibraryRamScanArtifact {
    let rules = catalog_config::library_path_map_from_env();
    apply_library_path_map_to_ram_artifact_with_rules(artifact, &rules)
}

pub fn apply_library_path_map_to_ram_artifact_with_rules(
    mut artifact: LibraryRamScanArtifact,
    rules: &[catalog_config::PathMapRule],
) -> LibraryRamScanArtifact {
    if rules.is_empty() {
        return artifact;
    }
    remap_library_scan_paths(&mut artifact.scan, rules);
    let covered_payloads = covered_payload_paths(&artifact.scan.discoveries);
    artifact.preferred_discoveries =
        preferred_playable_discovery_indices_by_key(&artifact.scan.discoveries, &covered_payloads);
    artifact.stats.discoveries = artifact.preferred_discoveries.len();
    artifact
}

fn remap_library_scan_paths(scan: &mut LibraryScan, rules: &[catalog_config::PathMapRule]) {
    for root in &mut scan.roots {
        *root = catalog_config::map_library_path(root, rules);
    }
    for core in &mut scan.installed_cores {
        core.path = PathBuf::from(catalog_config::map_library_path(
            core.path.to_string_lossy().as_ref(),
            rules,
        ));
    }
    for game_dir in &mut scan.game_dir_facts {
        // Host-path signatures cannot authorize warm reuse after paths are
        // remapped to the device namespace. Force the exact fallback there.
        game_dir.signature = crate::catalog_discovery::GameDirSignature::Unavailable;
        game_dir.path = PathBuf::from(catalog_config::map_library_path(
            game_dir.path.to_string_lossy().as_ref(),
            rules,
        ));
        for zip_path in &mut game_dir.direct_zip_paths {
            *zip_path = PathBuf::from(catalog_config::map_library_path(
                zip_path.to_string_lossy().as_ref(),
                rules,
            ));
        }
        for (child_path, signature) in &mut game_dir.nested_probe_signatures {
            *child_path = PathBuf::from(catalog_config::map_library_path(
                child_path.to_string_lossy().as_ref(),
                rules,
            ));
            *signature = crate::catalog_discovery::GameDirSignature::Unavailable;
        }
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
    let covered_payloads = covered_payload_paths(&scan.discoveries);
    let preferred_discoveries =
        preferred_playable_discovery_indices_by_key(&scan.discoveries, &covered_payloads);
    build_catalog_from_scan_with_preferred(root, scan, &preferred_discoveries)
}

fn build_catalog_from_scan_with_preferred(
    root: impl AsRef<Path>,
    scan: &LibraryScan,
    preferred_discoveries: &BTreeMap<String, usize>,
) -> ArcadeCatalog {
    build_catalog_from_scan_with_preferred_and_progress(
        root,
        scan,
        preferred_discoveries,
        &mut |_, _| {},
    )
    .0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CatalogProjectionTiming {
    metadata_us: u64,
    projection_rows_us: u64,
    indexes_us: u64,
    total_us: u64,
}

fn build_catalog_from_scan_with_preferred_and_progress(
    root: impl AsRef<Path>,
    scan: &LibraryScan,
    preferred_discoveries: &BTreeMap<String, usize>,
    progress: &mut dyn FnMut(&str, &str),
) -> (
    ArcadeCatalog,
    CatalogProjectionTiming,
    crate::scanner_cache::ScannerCacheState,
) {
    let mame_sqlite_path = default_mame_sqlite_path();
    let hbmame_sqlite_path = default_hbmame_sqlite_path();
    let preview_paths = PreviewArchivePaths::from_paths_with_sidecar_entries(
        preview_worker::preview_archive_paths_for_catalog_projection(),
    );
    let scanner_cache = crate::scanner_cache::load_default();
    build_catalog_from_scan_with_sources_and_preferred_and_progress(
        root,
        scan,
        CatalogBuildSources {
            mame_sqlite_path: &mame_sqlite_path,
            hbmame_sqlite_path: &hbmame_sqlite_path,
            preview_paths: &preview_paths,
            software_hash_cache: scanner_cache.software_hash_cache,
            discovery_history: scanner_cache.discovery_history,
        },
        preferred_discoveries,
        progress,
    )
}

struct CatalogBuildSources<'a> {
    mame_sqlite_path: &'a Path,
    hbmame_sqlite_path: &'a Path,
    preview_paths: &'a PreviewArchivePaths,
    software_hash_cache: SoftwareHashCache,
    discovery_history: Option<sqlite_catalog::DiscoveryHistory>,
}

fn requires_mame_software_metadata(
    scan: &LibraryScan,
    discoveries: &BTreeMap<String, usize>,
) -> bool {
    discoveries
        .values()
        .any(|index| software_list_for_platform(&scan.discoveries[*index].platform_id).is_some())
}

#[cfg(test)]
fn build_catalog_from_scan_with_sources(
    root: impl AsRef<Path>,
    scan: &LibraryScan,
    mame_sqlite_path: &Path,
    hbmame_sqlite_path: &Path,
    preview_paths: &PreviewArchivePaths,
    software_hash_cache: SoftwareHashCache,
    discovery_history: Option<sqlite_catalog::DiscoveryHistory>,
) -> ArcadeCatalog {
    let covered_payloads = covered_payload_paths(&scan.discoveries);
    let preferred_discoveries =
        preferred_playable_discovery_indices_by_key(&scan.discoveries, &covered_payloads);
    build_catalog_from_scan_with_sources_and_preferred(
        root,
        scan,
        CatalogBuildSources {
            mame_sqlite_path,
            hbmame_sqlite_path,
            preview_paths,
            software_hash_cache,
            discovery_history,
        },
        &preferred_discoveries,
    )
}

#[cfg(test)]
fn build_catalog_from_scan_with_sources_and_preferred(
    root: impl AsRef<Path>,
    scan: &LibraryScan,
    sources: CatalogBuildSources<'_>,
    discoveries: &BTreeMap<String, usize>,
) -> ArcadeCatalog {
    build_catalog_from_scan_with_sources_and_preferred_and_progress(
        root,
        scan,
        sources,
        discoveries,
        &mut |_, _| {},
    )
    .0
}

fn build_catalog_from_scan_with_sources_and_preferred_and_progress(
    root: impl AsRef<Path>,
    scan: &LibraryScan,
    sources: CatalogBuildSources<'_>,
    discoveries: &BTreeMap<String, usize>,
    progress: &mut dyn FnMut(&str, &str),
) -> (
    ArcadeCatalog,
    CatalogProjectionTiming,
    crate::scanner_cache::ScannerCacheState,
) {
    let total_t = std::time::Instant::now();
    progress(
        "Indexing library",
        &format!("Preparing library — {} discoveries", discoveries.len()),
    );
    let metadata_t = std::time::Instant::now();
    let mut software_hash_cache = sources.software_hash_cache;
    let previous_discovery_history = sources.discovery_history;
    let mut updated_discovery_history = crate::scanner_cache::DiscoveryHistory::default();
    let mut platform_kinds = scan
        .profiles
        .iter()
        .map(|profile| {
            (
                profile.system_id.clone(),
                crate::catalog_classify::platform_kind_for_system(&profile.system_id),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut playable_counts = HashMap::<String, usize>::new();
    let manifest_backed_systems = scan
        .profiles
        .iter()
        .filter(|profile| {
            profile.provenance.kind != crate::launch_profiles::RuleSourceKind::ConfStr
        })
        .map(|profile| profile.system_id.clone())
        .collect::<HashSet<_>>();
    for discovery in discoveries.values().map(|index| &scan.discoveries[*index]) {
        let system_id = catalog_system_id_for_discovery(discovery);
        *playable_counts.entry(system_id.clone()).or_default() += 1;
        platform_kinds
            .entry(system_id)
            .or_insert_with_key(|id| crate::catalog_classify::platform_kind_for_system(id));
    }
    let promoted_systems = playable_counts
        .iter()
        .filter_map(|(system_id, count)| {
            (*count >= 2
                || manifest_backed_systems.contains(system_id)
                || matches!(system_id.as_str(), "arcade" | "neogeo"))
            .then_some(system_id.clone())
        })
        .collect::<HashSet<_>>();
    let arcade_setnames =
        arcade_metadata_setnames(discoveries.values().map(|index| &scan.discoveries[*index]));
    let software_metadata = if requires_mame_software_metadata(scan, discoveries) {
        with_catalog_progress_heartbeat(
            progress,
            "Preparing library — loading software metadata",
            || load_mame_software_metadata(sources.mame_sqlite_path),
        )
    } else {
        MameSoftwareMetadata::default()
    };
    let arcade_metadata = with_catalog_progress_heartbeat(
        progress,
        "Preparing library — loading arcade metadata",
        || {
            load_arcade_machine_metadata_for_setnames(
                sources.mame_sqlite_path,
                sources.hbmame_sqlite_path,
                &arcade_setnames,
            )
        },
    );
    let metadata_us = metadata_t.elapsed().as_micros() as u64;
    let projection_rows_t = std::time::Instant::now();
    let now = unix_now_secs();
    let mut arcade_rows = Vec::<CatalogProjectionRow>::new();
    let mut launcher_rows = Vec::<CatalogProjectionRow>::new();
    let mut launch_plans = Vec::<StructuredLaunchPlan>::new();
    let mut projection_context = CatalogProjectionBuildContext {
        scan,
        software_metadata: &software_metadata,
        arcade_metadata: &arcade_metadata,
        preview_paths: sources.preview_paths,
        software_hash_cache: &mut software_hash_cache,
        discovery_history: previous_discovery_history.as_ref(),
        now,
    };

    {
        let mut row_progress =
            CatalogProjectionProgress::new(progress, "Resolving playable games", discoveries.len());
        row_progress.report(0, true);
        for (position, (key, index)) in discoveries.iter().enumerate() {
            let completed = position + 1;
            let discovery = &scan.discoveries[*index];
            if is_raw_arcade_zip_set_discovery(discovery) {
                row_progress.report(completed, completed == discoveries.len());
                continue;
            }
            if !promoted_systems.contains(&catalog_system_id_for_discovery(discovery)) {
                row_progress.report(completed, completed == discoveries.len());
                continue;
            }
            let Some(projection) = projection_context.projection_for_discovery(key, discovery)
            else {
                row_progress.report(completed, completed == discoveries.len());
                continue;
            };
            updated_discovery_history.by_game_id.insert(
                key.clone(),
                previous_discovery_history
                    .as_ref()
                    .and_then(|history| history.discovered_at_for(key, scan)),
            );
            if let Some(plan) = projection.launch_plan {
                launch_plans.push(plan);
            }
            if projection.is_arcade {
                arcade_rows.push(projection.row);
            } else {
                launcher_rows.push(projection.row);
            }
            row_progress.report(completed, completed == discoveries.len());
        }
    }
    let projection_rows_us = projection_rows_t.elapsed().as_micros() as u64;
    let indexes_t = std::time::Instant::now();
    let projected_games = arcade_rows.len() + launcher_rows.len();
    let mut index_progress =
        CatalogProjectionProgress::new(progress, "Building launcher indexes", projected_games);
    index_progress.report(0, true);
    let mut report_index_progress = |completed: usize, _total: usize| {
        index_progress.report(completed, completed == projected_games);
    };

    let catalog = catalog_from_sqlite_launcher_projection_order_with_platform_kinds_and_progress(
        root,
        arcade_rows,
        launcher_rows,
        launch_plans,
        platform_kinds,
        Some(&mut report_index_progress),
    );
    let indexes_us = indexes_t.elapsed().as_micros() as u64;
    (
        catalog,
        CatalogProjectionTiming {
            metadata_us,
            projection_rows_us,
            indexes_us,
            total_us: total_t.elapsed().as_micros() as u64,
        },
        crate::scanner_cache::ScannerCacheState {
            discovery_history: Some(updated_discovery_history),
            software_hash_cache,
        },
    )
}

fn catalog_progress_counter_detail(
    label: &str,
    completed: usize,
    total: usize,
    elapsed: std::time::Duration,
) -> String {
    let elapsed_seconds = elapsed.as_secs();
    if elapsed_seconds == 0 {
        format!("{label} — {completed} of {total}")
    } else {
        format!("{label} — {completed} of {total} — Still working… {elapsed_seconds}s")
    }
}

struct CatalogProjectionProgress<'a> {
    progress: &'a mut dyn FnMut(&str, &str),
    label: &'static str,
    total: usize,
    started: std::time::Instant,
    last_report: std::time::Instant,
}

impl<'a> CatalogProjectionProgress<'a> {
    fn new(progress: &'a mut dyn FnMut(&str, &str), label: &'static str, total: usize) -> Self {
        let started = std::time::Instant::now();
        Self {
            progress,
            label,
            total,
            started,
            last_report: started,
        }
    }

    fn report(&mut self, completed: usize, force: bool) {
        let now = std::time::Instant::now();
        if !force
            && !completed.is_multiple_of(250)
            && now.duration_since(self.last_report) < std::time::Duration::from_secs(1)
        {
            return;
        }
        (self.progress)(
            "Indexing library",
            &catalog_progress_counter_detail(
                self.label,
                completed,
                self.total,
                now.duration_since(self.started),
            ),
        );
        self.last_report = now;
    }
}

fn with_catalog_progress_heartbeat<T: Send>(
    progress: &mut dyn FnMut(&str, &str),
    detail: &str,
    work: impl FnOnce() -> T + Send,
) -> T {
    let started = std::time::Instant::now();
    let background = crate::cooperative_work::in_background_scope();
    progress("Indexing library", detail);
    std::thread::scope(|scope| {
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        scope.spawn(move || {
            let _background_scope =
                background.then(crate::cooperative_work::BackgroundScope::enter);
            crate::cooperative_work::checkpoint();
            let _ = result_tx.send(work());
        });
        loop {
            match result_rx.recv_timeout(std::time::Duration::from_secs(1)) {
                Ok(result) => return result,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    progress(
                        "Indexing library",
                        &format!("{detail} — Still working… {}s", started.elapsed().as_secs()),
                    );
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("catalog metadata worker disconnected")
                }
            }
        }
    })
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
                let available = self.preview_paths.has_entry(&system_id, &preview_key);
                LauncherPreviewAsset::with_availability(
                    preview_worker::preview_archive_path_for_system(&system_id),
                    preview_key,
                    available,
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
            let preview = amigavision_preview_asset(discovery, &system_id, self.preview_paths)
                .or_else(|| {
                    software_identity
                        .as_ref()
                        .and_then(|identity| console_preview_asset(identity, self.preview_paths))
                        .map(|asset| {
                            LauncherPreviewAsset::with_availability(
                                preview_worker::preview_archive_path_for_system(&system_id),
                                asset.asset_key.to_string(),
                                asset.has_preview,
                            )
                        })
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
                    category: discovery.category.clone(),
                    players: None,
                    control: String::new(),
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
                identity_matched: software_identity.is_some(),
                prepared: discovery.prepared,
            },
        );
        if is_arcade
            && let Some(identity_id) = mame_identity_for_discovery(discovery)
            && let Some(metadata) =
                mister_arcade_metadata_for_discovery(self.arcade_metadata, discovery, &identity_id)
            && !metadata.title.is_empty()
        {
            row.game.title = metadata.title.clone().into();
        }
        Some(CatalogProjectionForDiscovery {
            row,
            is_arcade,
            launch_plan,
        })
    }
}

fn amigavision_preview_asset(
    discovery: &GameDiscovery,
    system_id: &str,
    preview_paths: &PreviewArchivePaths,
) -> Option<LauncherPreviewAsset> {
    if system_id != "amiga" || discovery.genre.as_deref() != Some("AmigaVision") {
        return None;
    }
    preview_paths.archive_for_platform("amiga")?;
    Some(LauncherPreviewAsset::with_availability(
        preview_worker::preview_archive_path_for_system("amiga"),
        crate::media_identity::ScreenshotAssetId::from_amigavision_title(&discovery.title)
            .into_string(),
        preview_paths.has_entry(
            "amiga",
            &crate::media_identity::ScreenshotAssetId::from_amigavision_title(&discovery.title)
                .into_string(),
        ),
    ))
}

#[cfg(test)]
fn catalog_from_sqlite_launcher_projection_order(
    root: impl AsRef<Path>,
    arcade_rows: Vec<CatalogProjectionRow>,
    launcher_rows: Vec<CatalogProjectionRow>,
    launch_plans: Vec<StructuredLaunchPlan>,
) -> ArcadeCatalog {
    catalog_from_sqlite_launcher_projection_order_with_platform_kinds(
        root,
        arcade_rows,
        launcher_rows,
        launch_plans,
        HashMap::new(),
    )
}

#[cfg(test)]
fn catalog_from_sqlite_launcher_projection_order_with_platform_kinds(
    root: impl AsRef<Path>,
    arcade_rows: Vec<CatalogProjectionRow>,
    launcher_rows: Vec<CatalogProjectionRow>,
    launch_plans: Vec<StructuredLaunchPlan>,
    platform_kinds: HashMap<String, PlatformKind>,
) -> ArcadeCatalog {
    catalog_from_sqlite_launcher_projection_order_with_platform_kinds_and_progress(
        root,
        arcade_rows,
        launcher_rows,
        launch_plans,
        platform_kinds,
        None,
    )
}

fn catalog_from_sqlite_launcher_projection_order_with_platform_kinds_and_progress(
    root: impl AsRef<Path>,
    mut arcade_rows: Vec<CatalogProjectionRow>,
    mut launcher_rows: Vec<CatalogProjectionRow>,
    mut launch_plans: Vec<StructuredLaunchPlan>,
    platform_kinds: HashMap<String, PlatformKind>,
    index_progress: Option<&mut dyn FnMut(usize, usize)>,
) -> ArcadeCatalog {
    let mut source_games_by_system = HashMap::<String, usize>::new();
    for row in arcade_rows.iter().chain(launcher_rows.iter()) {
        *source_games_by_system
            .entry(row.game.system_id.to_string())
            .or_default() += 1;
    }
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
    let catalog = ArcadeCatalog::new_with_deferred_text_indexes_and_platform_kinds_with_progress(
        root.as_ref().to_path_buf(),
        games,
        systems,
        launch_plans,
        platform_kinds,
        index_progress,
    );
    if source_games_by_system.is_empty() {
        catalog
    } else {
        let projection_stats = source_games_by_system
            .into_iter()
            .map(|(system_id, source_games)| {
                let visible_families = catalog.system_game_count(&system_id);
                (
                    system_id,
                    SystemProjectionStats {
                        source_games,
                        visible_families,
                        collapsed_variants: source_games.saturating_sub(visible_families),
                    },
                )
            })
            .collect();
        catalog.with_projection_stats(projection_stats)
    }
}

fn structured_launch_plan_for_discovery(
    discovery: &GameDiscovery,
    launch_ref: &str,
    profiles: &[LaunchProfile],
) -> Option<StructuredLaunchPlan> {
    if launch_kind_for_discovery(discovery) != "virtual-mgl" {
        return None;
    }
    let profile =
        crate::catalog_scan::profile_for_path(profiles, Path::new(discovery.source_path.as_str()))
            .or_else(|| {
                profile_id_for_discovery(discovery).and_then(|profile_id| {
                    launch_profiles::profile_for_launch_target_id(profiles, profile_id)
                })
            })?;
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
    let mount = if discovery
        .prepared
        .is_some_and(|prepared| prepared.collection_id == PreparedCollectionId::OneLoad64)
    {
        launch_profiles::MountSpec::load_file(1)
    } else {
        payload_rule
            .as_ref()
            .map(|rule| rule.mount)
            .unwrap_or_else(|| launch_profiles::MountSpec::mount_image(0))
    };
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
        let (family_id, _, year, manufacturer, players, control, _) = mame_identity_projection(
            &identity_id,
            arcade_metadata,
            discovery.parent.as_deref(),
            &discovery.title,
        );
        let mister = mister_arcade_metadata_for_discovery(arcade_metadata, discovery, &identity_id);
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
                year: mister
                    .and_then(|metadata| metadata.year)
                    .or_else(|| optional_year_from_metadata(year)),
                manufacturer: mister
                    .filter(|metadata| !metadata.manufacturer.is_empty())
                    .map(|metadata| metadata.manufacturer.clone())
                    .unwrap_or_else(|| manufacturer.unwrap_or_default().to_string()),
                category: mister
                    .map(|metadata| metadata.category.clone())
                    .unwrap_or_default(),
                players: mister.and_then(|metadata| metadata.players).or(players),
                control: mister
                    .filter(|metadata| !metadata.control.is_empty())
                    .map(|metadata| metadata.control.clone())
                    .unwrap_or_else(|| control.unwrap_or_default().to_string()),
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
            category: discovery.category.clone(),
            players: None,
            control: String::new(),
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
                category: discovery.category.clone(),
                players: None,
                control: String::new(),
            },
            false,
            CatalogProjectionSource {
                source_kind: launch_kind_for_discovery(discovery).to_string(),
                setname,
                parent,
                family_key: Some(family_key),
                identity_matched: false,
                prepared: discovery.prepared,
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
        let (family_id, _, _, _, _, _, _) = mame_identity_projection(
            &identity_id,
            arcade_metadata,
            discovery.parent.as_deref(),
            &discovery.title,
        );
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
    (0..=buf.len() - 22).rev().find(|&idx| {
        buf[idx..idx + 4] == [0x50, 0x4b, 0x05, 0x06]
            && idx + 22 + le_u16(&buf[idx + 20..idx + 22]) as usize == buf.len()
    })
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

    #[test]
    fn find_eocd_ignores_signature_inside_zip_comment() {
        let mut archive = vec![0u8; 52];
        archive[..4].copy_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        archive[20..22].copy_from_slice(&30u16.to_le_bytes());
        archive[26..30].copy_from_slice(&[0x50, 0x4b, 0x05, 0x06]);

        assert_eq!(find_eocd(&archive), Some(0));
    }

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
        players: Option<u8>,
        control: String,
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
                players: game.players,
                control: game.control.to_string(),
                is_new: game.is_new,
            })
            .collect()
    }

    fn ram_artifact_for_test(scan: LibraryScan, scan_us: u64) -> LibraryRamScanArtifact {
        let covered_payloads = covered_payload_paths(&scan.discoveries);
        let preferred_discoveries =
            preferred_playable_discovery_indices_by_key(&scan.discoveries, &covered_payloads);
        LibraryRamScanArtifact {
            stats: LibraryScanStats {
                scan_us,
                discover_us: scan.discover_us,
                classify_us: scan.classify_us,
                normal_files: scan.normal_files.len(),
                containers: scan.containers.len(),
                entries: scan.entries.len(),
                audit_rows: 0,
                discoveries: preferred_discoveries.len(),
            },
            scan,
            preferred_discoveries,
        }
    }

    #[test]
    fn authoritative_scan_reusing_arcade_matches_a_single_full_scan() {
        let root = unique_temp_dir("reuse-arcade-scan");
        let arcade = root.join("_Arcade");
        let dos = root.join("_DOS Games");
        std::fs::create_dir_all(&arcade).expect("create Arcade");
        std::fs::create_dir_all(&dos).expect("create DOS launchers");
        std::fs::write(
            arcade.join("Puck Man.mra"),
            "<misterromdescription><name>Puck Man</name><setname>puckman</setname></misterromdescription>",
        )
        .expect("write Arcade launcher");
        std::fs::write(
            dos.join("Doom.mgl"),
            "<mistergamelist><rbf>AO486</rbf><file delay=\"1\" type=\"s\">../games/AO486/Doom.vhd</file></mistergamelist>",
        )
        .expect("write DOS launcher");

        let full_cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("full.sqlite3"),
        };
        let bootstrap_cfg = BenchConfig {
            roots: vec![arcade.display().to_string()],
            sqlite_path: root.join("bootstrap.sqlite3"),
        };
        let full = CatalogRefreshPipeline::new(&full_cfg)
            .scan_ram_artifact_foreground_with_events(None, None);
        let bootstrap = CatalogRefreshPipeline::new(&bootstrap_cfg)
            .scan_ram_artifact_foreground_with_events(None, None);
        let reused = CatalogRefreshPipeline::new(&full_cfg).scan_ram_artifact_with_reused_prefix(
            bootstrap,
            vec![arcade.clone()],
            None,
            None,
        );

        assert_eq!(reused.stats.discoveries, full.stats.discoveries);
        assert_eq!(reused.stats.normal_files, full.stats.normal_files);
        assert_eq!(reused.stats.containers, full.stats.containers);
        assert_eq!(reused.stats.entries, full.stats.entries);
        assert_eq!(
            catalog_game_snapshots(&reused.catalog(&arcade)),
            catalog_game_snapshots(&full.catalog(&arcade)),
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_games_use_amiga_pack_identity_but_demos_do_not() {
        let mut discovery = payload("/media/fat/games/Amiga/AmigaVision.hdf");
        discovery.title = "Alien Breed (OCS)[en]".to_string();
        discovery.platform_id = "amiga".to_string();
        discovery.genre = Some("AmigaVision".to_string());
        let paths = PreviewArchivePaths::from_paths(vec![
            "/media/fat/mister-magik/assets/amiga-screenshots.mmlz4b".to_string(),
        ]);

        let preview = amigavision_preview_asset(&discovery, "amiga", &paths)
            .expect("AmigaVision game preview");
        assert_eq!(
            preview.archive_path,
            "/media/fat/mister-magik/assets/amiga-screenshots.mmlz4b"
        );
        assert_eq!(preview.asset_key, "amigavision__667cdd86c04e1709");

        discovery.genre = Some("AmigaVision demos".to_string());
        assert!(amigavision_preview_asset(&discovery, "amiga", &paths).is_none());
    }

    #[test]
    fn fresh_arcade_catalog_marks_preview_only_for_index_members() {
        let root = unique_temp_dir("fresh-arcade-preview-index");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[
                ("mpatrol", None, "Moon Patrol", Some("1982"), Some("Irem")),
                ("adcanoe", None, "Agent X", Some("1983"), Some("Atari")),
            ],
        );
        let pack = preview_worker::PreviewArchiveIndex {
            path: root
                .join("arcade-screenshots-320x320.mmlz4b")
                .display()
                .to_string(),
            codec: "mmlz4b",
            entries: vec!["mpatrol.rgb565".to_string()],
        };
        let preview_paths = PreviewArchivePaths::from_preview_indexes(&[pack]);
        let mut with_preview = mra_discovery(1, "Moon Patrol");
        with_preview.setname = Some("mpatrol".to_string());
        let mut without_preview = mra_discovery(2, "Agent X");
        without_preview.setname = Some("adcanoe".to_string());
        let scan = sqlite_scan_with_discoveries(vec![with_preview, without_preview]);

        let catalog = build_catalog_from_scan_with_sources(
            "/media/fat/_Arcade",
            &scan,
            &mame_db,
            &PathBuf::new(),
            &preview_paths,
            SoftwareHashCache::load(&root.join("library.sqlite3")),
            None,
        );

        let present = catalog
            .games
            .iter()
            .find(|game| game.preview_asset_key.as_ref() == "mpatrol")
            .expect("indexed arcade preview row");
        assert!(present.has_preview);
        let absent = catalog
            .games
            .iter()
            .find(|game| game.preview_asset_key.as_ref() == "adcanoe")
            .expect("non-indexed arcade preview row");
        assert!(!absent.has_preview);
        assert_eq!(absent.preview_asset_key.as_ref(), "adcanoe");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn projection_returns_lossless_v3_scanner_state() {
        let root = unique_temp_dir("projection-scanner-state");
        let scan = sqlite_scan_with_discoveries(vec![mra_discovery(1, "1942")]);
        let covered_payloads = covered_payload_paths(&scan.discoveries);
        let preferred =
            preferred_playable_discovery_indices_by_key(&scan.discoveries, &covered_payloads);
        let game_id = preferred.keys().next().expect("projected game id").clone();
        let mut history = crate::scanner_cache::DiscoveryHistory::default();
        history.by_game_id.insert(game_id.clone(), Some(123));
        let cache_key = crate::software_identity::SoftwareHashCacheKey {
            list_name: "nes".into(),
            file_path: "/games/One.nes".into(),
            size: 42,
            mtime_secs: 7,
        };
        let mut software_hash_cache = SoftwareHashCache::default();
        software_hash_cache
            .entries
            .insert(cache_key.clone(), Some("one".into()));

        let (_catalog, _timing, scanner_cache) =
            build_catalog_from_scan_with_sources_and_preferred_and_progress(
                "/media/fat/_Arcade",
                &scan,
                CatalogBuildSources {
                    mame_sqlite_path: &root.join("missing-mame.sqlite3"),
                    hbmame_sqlite_path: &root.join("missing-hbmame.sqlite3"),
                    preview_paths: &PreviewArchivePaths::from_paths(Vec::<String>::new()),
                    software_hash_cache,
                    discovery_history: Some(history),
                },
                &preferred,
                &mut |_, _| {},
            );

        assert_eq!(
            scanner_cache
                .discovery_history
                .expect("updated discovery history")
                .by_game_id
                .get(&game_id),
            Some(&Some(123))
        );
        assert_eq!(
            scanner_cache.software_hash_cache.entries.get(&cache_key),
            Some(&Some("one".into()))
        );
        assert!(!root.join("library.sqlite3").exists());
        let _ = std::fs::remove_dir_all(root);
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

    #[test]
    fn only_software_list_projection_requires_mame_software_metadata() {
        let arcade_scan = sqlite_scan_with_discoveries(vec![mra_discovery(1, "Puck Man")]);
        let arcade_preferred = preferred_playable_discovery_indices_by_key(
            &arcade_scan.discoveries,
            &covered_payload_paths(&arcade_scan.discoveries),
        );
        assert!(!requires_mame_software_metadata(
            &arcade_scan,
            &arcade_preferred
        ));

        let mut neogeo = mra_discovery(2, "Neo Geo Game");
        neogeo.platform_id = "neogeo".to_string();
        let neogeo_scan = sqlite_scan_with_discoveries(vec![neogeo]);
        let neogeo_preferred = preferred_playable_discovery_indices_by_key(
            &neogeo_scan.discoveries,
            &covered_payload_paths(&neogeo_scan.discoveries),
        );
        assert!(!requires_mame_software_metadata(
            &neogeo_scan,
            &neogeo_preferred
        ));

        let mut console = mra_discovery(3, "Console Game");
        console.platform_id = "megadrive".to_string();
        console.category = "Console".to_string();
        let mixed_scan = sqlite_scan_with_discoveries(vec![mra_discovery(1, "Puck Man"), console]);
        let mixed_preferred = preferred_playable_discovery_indices_by_key(
            &mixed_scan.discoveries,
            &covered_payload_paths(&mixed_scan.discoveries),
        );
        assert!(requires_mame_software_metadata(
            &mixed_scan,
            &mixed_preferred
        ));
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
            prepared: None,
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
        assert!(
            !ram_catalog
                .games
                .iter()
                .any(|game| game.mra_path.contains("/games/mame/")
                    || game.mra_path.contains("/games/hbmame/"))
        );
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
        assert!(
            loaded
                .catalog
                .games
                .iter()
                .any(|game| game.title.as_ref() == "Puck Man")
        );
        assert!(
            loaded
                .catalog
                .games
                .iter()
                .any(|game| game.title.as_ref() == "Homebrew Demo")
        );
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
        let ram_artifact = ram_artifact_for_test(scan, 42);

        assert_eq!(ram_artifact.stats().audit_rows, 0);
        assert_eq!(ram_artifact.catalog("/media/fat/_Arcade").len(), 1);

        let early_identity = ram_artifact.clone().complete_coverage_audit_for_decision();
        let expected_state = ram_artifact
            .clone()
            .complete_coverage_audit()
            .catalog_state();
        assert_eq!(early_identity.catalog_state(), &expected_state);
        assert_eq!(
            early_identity.stats().audit_rows,
            expected_state.stats.audit_rows
        );

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
    fn compact_catalog_prepare_is_equivalent_to_full_scan_prepare() {
        let root = unique_temp_dir("parallel-catalog-prepare-equivalence");
        std::fs::create_dir_all(root.join("games/NES")).expect("create games dir");
        install_test_console_core(&root, "NES");
        let mut scan = sqlite_scan_with_discoveries(vec![mra_discovery(1, "1942")]);
        scan.roots = vec![root.display().to_string()];
        scan.installed_cores = crate::catalog_discovery::installed_cores_for_roots(&scan.roots);
        scan.game_dir_facts = crate::catalog_discovery::top_level_game_dirs_for_roots(&scan.roots);

        let sequential_ram = ram_artifact_for_test(scan.clone(), 42);
        let sequential_catalog = sequential_ram.catalog("/media/fat/_Arcade");
        let sequential_artifact = sequential_ram.complete_coverage_audit();

        let compact_ram = ram_artifact_for_test(scan, 42);
        let (compact_state, compact_catalog, timing, _scanner_cache) = compact_ram
            .complete_coverage_audit_and_catalog_foreground("/media/fat/_Arcade")
            .expect("compact catalog prepare");

        assert_eq!(
            catalog_game_snapshots(&compact_catalog),
            catalog_game_snapshots(&sequential_catalog)
        );
        assert_eq!(compact_catalog.systems, sequential_catalog.systems);
        assert_eq!(compact_state.stamp(), &sequential_artifact.stamp);
        assert_eq!(
            compact_state.stats().audit_rows,
            sequential_artifact.stats.audit_rows
        );
        let sequential_navigation =
            crate::catalog_navigation::encode_catalog_navigation_for_storage(
                &sequential_catalog,
                &sequential_artifact.stamp,
            )
            .expect("encode sequential navigation");
        let compact_navigation = crate::catalog_navigation::encode_catalog_navigation_for_storage(
            &compact_catalog,
            compact_state.stamp(),
        )
        .expect("encode compact navigation");
        assert_eq!(compact_navigation, sequential_navigation);
        assert_eq!(timing.overlapped_us, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compact_catalog_prepare_preserves_post_scan_drift_detection() {
        let root = unique_temp_dir("parallel-catalog-prepare-drift");
        std::fs::create_dir_all(root.join("games/NES")).expect("create initial games dir");
        install_test_console_core(&root, "NES");
        let mut scan = sqlite_scan_with_discoveries(vec![mra_discovery(1, "1942")]);
        scan.roots = vec![root.display().to_string()];
        scan.installed_cores = crate::catalog_discovery::installed_cores_for_roots(&scan.roots);
        scan.game_dir_facts = crate::catalog_discovery::top_level_game_dirs_for_roots(&scan.roots);
        let profiles = scan.profiles.clone();
        let roots = scan.roots.clone();
        let ram = ram_artifact_for_test(scan, 0);
        let (prepared, _catalog, _timing, _scanner_cache) = ram
            .complete_coverage_audit_and_catalog_foreground("/media/fat/_Arcade")
            .expect("compact catalog prepare");
        install_test_console_core(&root, "SNES");
        std::fs::create_dir_all(root.join("games/SNES")).expect("create drift games dir");
        std::fs::write(root.join("games/SNES/Game.sfc"), b"sfc").expect("write drift game");

        let current_cores = crate::catalog_discovery::installed_cores_for_roots(&roots);
        let current_game_dirs = crate::catalog_discovery::top_level_game_dirs_for_roots(&roots);
        let current_audit = core_audit::audit_catalog_coverage_from_facts(
            &roots,
            &profiles,
            &current_cores,
            &current_game_dirs,
        );
        let current_checkpoint =
            crate::catalog_checkpoint::compute_catalog_discovery_checkpoint_from_facts(
                &roots,
                &default_mame_sqlite_path(),
                &default_hbmame_sqlite_path(),
                &current_audit,
                &current_cores,
                &current_game_dirs,
            );
        let drift = CatalogDriftSummary::from_checkpoints(
            Some(&prepared.catalog_state.checkpoint),
            &current_checkpoint,
        );
        assert!(!drift.unchanged);
        assert!(drift.changed_cores > 0 || drift.changed_game_dirs > 0);
        let current_stamp =
            catalog_stamp::compute_default_catalog_stamp_with_audit(&roots, &current_audit);
        assert_ne!(prepared.catalog_state.stamp, current_stamp);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn path_map_remaps_retained_checkpoint_facts() {
        let mut scan = sqlite_scan_with_discoveries(Vec::new());
        scan.roots = vec!["/source/library".to_string()];
        scan.installed_cores = vec![InstalledCore {
            core_id: "NES".to_string(),
            path: PathBuf::from("/source/library/_Console/NES.rbf"),
        }];
        scan.game_dir_facts = vec![GameDirFact {
            name: "NES".to_string(),
            path: PathBuf::from("/source/library/games/NES"),
            signature: crate::catalog_discovery::GameDirSignature::Unavailable,
            has_payload_files: true,
            has_zip_files: true,
            direct_zip_paths: vec![PathBuf::from("/source/library/games/NES/Unsupported.zip")],
            nested_probe_signatures: Vec::new(),
            payload_extensions: ["nes".to_string()].into_iter().collect(),
        }];
        let rules = catalog_config::parse_library_path_map("/source/library=/mapped/library");

        remap_library_scan_paths(&mut scan, &rules);

        assert_eq!(scan.roots, vec!["/mapped/library"]);
        assert_eq!(
            scan.installed_cores[0].path,
            PathBuf::from("/mapped/library/_Console/NES.rbf")
        );
        assert_eq!(
            scan.game_dir_facts[0].path,
            PathBuf::from("/mapped/library/games/NES")
        );
        assert_eq!(
            scan.game_dir_facts[0].direct_zip_paths,
            vec![PathBuf::from("/mapped/library/games/NES/Unsupported.zip")]
        );
        let checkpoint = crate::catalog_checkpoint::compute_catalog_discovery_checkpoint_from_facts(
            &scan.roots,
            Path::new("/mapped/mame.sqlite3"),
            Path::new("/mapped/hbmame.sqlite3"),
            &[],
            &scan.installed_cores,
            &scan.game_dir_facts,
        );
        assert!(
            checkpoint
                .lines()
                .iter()
                .all(|line| !line.contains("/source/library"))
        );
    }

    #[test]
    fn parallel_catalog_prepare_worker_keeps_foreground_all_online_policy() {
        use crate::runtime_thread::{RuntimeThreadPolicy, ThreadAffinity};

        assert_eq!(
            CATALOG_PREPARE_WORKER_ROLE,
            RuntimeThreadRole::CatalogForeground
        );
        assert_eq!(
            CATALOG_PREPARE_WORKER_ROLE.default_policy(),
            RuntimeThreadPolicy::new(0, ThreadAffinity::AllOnline)
        );
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
            prepared: None,
            confidence: DiscoveryConfidence::ArchiveToc,
        };

        let amigavision_game = GameDiscovery {
            source_path: "/media/fat/games/Amiga/AmigaVision.hdf::Alien Breed".to_string(),
            launch_ref: media_metadata::amigavision_game_launch_ref(
                "listings/games.txt",
                "Alien Breed",
            ),
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
            prepared: None,
            confidence: DiscoveryConfidence::CatalogMetadata,
        };
        let amigavision_demo = GameDiscovery {
            source_path: "/media/fat/games/Amiga/AmigaVision.hdf::State of the Art".to_string(),
            launch_ref: media_metadata::amigavision_game_launch_ref(
                "listings/demos.txt",
                "State of the Art",
            ),
            source_kind: DiscoverySourceKind::CatalogEntry,
            title: "State of the Art".to_string(),
            category: "Computer".to_string(),
            platform_id: "amiga".to_string(),
            core_id: "Minimig".to_string(),
            hardware_id: "amiga".to_string(),
            manufacturer: None,
            genre: Some("AmigaVision demos".to_string()),
            year: None,
            setname: None,
            parent: None,
            covered_payload_path: None,
            prepared: None,
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
            amigavision_demo,
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
        for (system_id, expected) in [
            ("arcade", PlatformKind::Arcade),
            ("neogeo", PlatformKind::Arcade),
            ("nes", PlatformKind::Console),
            ("dos", PlatformKind::Computer),
            ("amiga", PlatformKind::Computer),
        ] {
            assert_eq!(ram_catalog.platform_kind(system_id), expected);
            assert_eq!(
                sqlite_catalog.catalog.platform_kind(system_id),
                expected,
                "fresh and SQLite catalog platform kinds diverged for {system_id}"
            );
        }
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
        assert!(
            ram_catalog
                .games
                .iter()
                .any(|game| game.mra_path.starts_with("magik-amigavision:"))
        );
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
        assert!(
            !messages
                .iter()
                .any(|(title, detail)| title == "Classifying library"
                    && detail.starts_with("Games found: "))
        );
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
            LibraryScanEvent::ReconciliationPlanReady { .. }
            | LibraryScanEvent::SystemScanning { .. }
            | LibraryScanEvent::TargetProgress { .. } => {}
        };

        let scan = scan_library_with_progress_and_events(&cfg, None, Some(&mut scan_events));

        assert_eq!(unique_discovery_count(&scan.discoveries), 3);
        systems.sort();
        assert_eq!(systems, ["nes", "snes"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scan_events_report_systems_without_screenshot_packs() {
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
            LibraryScanEvent::ReconciliationPlanReady { .. }
            | LibraryScanEvent::SystemScanning { .. }
            | LibraryScanEvent::TargetProgress { .. } => {}
        };

        let scan = scan_library_with_progress_and_events(&cfg, None, Some(&mut scan_events));

        assert_eq!(unique_discovery_count(&scan.discoveries), 2);
        systems.sort();
        assert_eq!(systems, ["gba", "nes"]);
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
        assert!(
            messages
                .iter()
                .any(|(title, detail)| title == "Finding games" && detail == "Games found: 50")
        );
        assert!(
            !messages
                .iter()
                .any(|(title, detail)| title == "Finding games" && detail == "Games found: 1")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn catalog_projection_progress_counter_includes_elapsed_heartbeat() {
        assert_eq!(
            catalog_progress_counter_detail(
                "Resolving playable games",
                250,
                500,
                std::time::Duration::ZERO,
            ),
            "Resolving playable games — 250 of 500"
        );
        assert_eq!(
            catalog_progress_counter_detail(
                "Building launcher indexes",
                500,
                500,
                std::time::Duration::from_secs(3),
            ),
            "Building launcher indexes — 500 of 500 — Still working… 3s"
        );
    }
}
