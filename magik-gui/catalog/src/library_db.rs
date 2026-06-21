//! Whole-MiSTer library database scanning and loading.
//!
//! This is deliberately TOC/header-only for archives. Indexing must never
//! decompress full game libraries just to make the launcher searchable.

use crate::arcade_catalog::{self, ArcadeCatalog, ArcadeGameEntry};
use crate::catalog_config;
#[cfg(test)]
use crate::catalog_config::DEFAULT_SQLITE_BUILD_DIR;
pub use crate::catalog_config::{
    default_hbmame_sqlite_path, default_mame_sqlite_path, default_sqlite_path,
};
use crate::catalog_config::{DEFAULT_SQLITE_PATH, SCHEMA_VERSION};
use crate::catalog_scan::{self, DiscoveryEvent};
use crate::catalog_stamp;
#[cfg(test)]
use crate::catalog_store;
use crate::game_discovery::{
    catalog_system_id_for_discovery, covered_payload_paths, discovery_from_profile_archive_entry,
    discovery_from_profile_file, is_launcher_launch_ref, launch_kind_for_discovery,
    launch_ref_for_discovery, preferred_playable_discoveries_by_key, unique_discovery_count,
    GameDiscovery,
};
use crate::launch_profiles::{
    self, CollectionListing, PayloadDisposition, PayloadRule, ProfilePathClass,
};
pub(crate) use crate::library_cli::{
    canonical_variant_title, collapse_catalog_variants, CatalogRow,
};
use crate::media_metadata;
#[cfg(test)]
use crate::preview_worker;
#[cfg(test)]
use crate::software_identity::{
    crc32, mame_software_identity_for_discovery_with_hash_matcher,
    match_software_by_file_hash_with_cache, preview_asset_pack_platform, rom_hash_candidates,
    software_asset_key, MameSoftwareItemMetadata, MameSoftwareMetadata, SoftwareHashCache,
};
use crate::software_identity::{
    load_arcade_machine_metadata, mame_identity_for_discovery, mame_identity_projection,
    write_simple_mame_metadata_db, ArcadeMachineMetadata, MachineMetadataRows,
};
use crate::sqlite_catalog;
#[cfg(test)]
use crate::sqlite_catalog::{SqliteBuildTempPlan, SqliteBuildTempSource};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub(crate) const MRA_PREFIX_BYTES: usize = 160 * 1024;
pub(crate) type ProgressCallback<'a> = Option<&'a mut dyn FnMut(&str, &str)>;

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
    scan: LibraryScan,
    stats: LibraryScanStats,
    stamp: catalog_stamp::CatalogStamp,
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

pub fn load_virtual_launch_plan(launch_ref: &str) -> Result<Option<VirtualLaunchPlan>, String> {
    sqlite_catalog::load_virtual_launch_plan(launch_ref)
}

pub fn load_virtual_launch_plans_for_system(
    system_id: &str,
    limit: usize,
) -> Result<Vec<VirtualLaunchPlan>, String> {
    sqlite_catalog::load_virtual_launch_plans_for_system(system_id, limit)
}

pub fn load_virtual_launch_plans() -> Result<Vec<VirtualLaunchPlan>, String> {
    sqlite_catalog::load_virtual_launch_plans()
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

#[cfg(test)]
fn load_virtual_launch_plans_for_system_from_conn(
    conn: &Connection,
    system_id: &str,
    limit: usize,
) -> Result<Vec<VirtualLaunchPlan>, String> {
    sqlite_catalog::load_virtual_launch_plans_for_system_from_conn(conn, system_id, limit)
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

fn save_sqlite_scan_with_progress_and_stamp(
    path: &Path,
    scan: &LibraryScan,
    stamp: Option<&catalog_stamp::CatalogStamp>,
    progress: ProgressCallback<'_>,
) -> Result<u64, String> {
    sqlite_catalog::save_sqlite_scan_with_progress_and_stamp(path, scan, stamp, progress)
}

#[cfg(test)]
fn save_sqlite_scan_with_progress_using_writer(
    path: &Path,
    scan: &LibraryScan,
    progress: ProgressCallback<'_>,
    initial_plan: SqliteBuildTempPlan,
    writer: &mut dyn FnMut(&Path, &LibraryScan, &mut ProgressCallback<'_>) -> Result<(), String>,
) -> Result<u64, String> {
    sqlite_catalog::save_sqlite_scan_with_progress_using_writer(
        path,
        scan,
        progress,
        initial_plan,
        writer,
    )
}

#[cfg(test)]
fn sqlite_build_temp_plan_for(
    path: &Path,
    build_dir_override: Option<&Path>,
) -> SqliteBuildTempPlan {
    sqlite_catalog::sqlite_build_temp_plan_for(path, build_dir_override)
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

#[cfg(test)]
fn write_sqlite_scan_with_mame_and_hbmame(
    path: &Path,
    scan: &LibraryScan,
    mame_sqlite_path: &Path,
    hbmame_sqlite_path: &Path,
) -> Result<(), String> {
    sqlite_catalog::write_sqlite_scan_with_mame_and_hbmame(
        path,
        scan,
        mame_sqlite_path,
        hbmame_sqlite_path,
    )
}

#[cfg(test)]
fn write_sqlite_scan_with_mame_and_preview_pack(
    path: &Path,
    scan: &LibraryScan,
    mame_sqlite_path: &Path,
    preview_asset_pack: &preview_worker::PreviewArchiveIndex,
) -> Result<(), String> {
    sqlite_catalog::write_sqlite_scan_with_mame_and_preview_pack(
        path,
        scan,
        mame_sqlite_path,
        preview_asset_pack,
    )
}

pub fn rebuild_default_sqlite_database(
    progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    let cfg = BenchConfig::production();
    rebuild_sqlite_database(&cfg, progress)
}

pub fn scan_default_library(progress: ProgressCallback<'_>) -> Result<LibraryScanArtifact, String> {
    let cfg = BenchConfig::production();
    Ok(scan_library_artifact(&cfg, progress))
}

pub fn write_default_hbmame_metadata_from_library() -> Result<HbmameMetadataSummary, String> {
    write_hbmame_metadata_from_library(&default_sqlite_path(), &default_hbmame_sqlite_path())
}

fn write_hbmame_metadata_from_library(
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
    mut progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    let scan_t = Instant::now();
    if let Some(report) = progress.as_mut() {
        report("Indexing library", "Full catalog build...");
    }
    let artifact = match progress.as_mut() {
        Some(report) => scan_library_artifact(cfg, Some(&mut **report)),
        None => scan_library_artifact(cfg, None),
    };
    let scan_us = scan_t.elapsed().as_micros() as u64;
    if let Some(report) = progress.as_mut() {
        report(
            "Indexing library",
            &format!(
                "Writing {} games, {} archives...",
                artifact.stats.discoveries, artifact.stats.containers
            ),
        );
    }
    let mut summary = save_scan_artifact_to_sqlite(cfg, artifact, progress)?;
    summary.scan_us = scan_us;
    Ok(summary)
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
    scan_library_with_progress(cfg, None)
}

pub(crate) fn scan_library_artifact(
    cfg: &BenchConfig,
    mut progress: ProgressCallback<'_>,
) -> LibraryScanArtifact {
    let stamp = catalog_stamp::compute_default_catalog_stamp(&cfg.roots);
    let scan_t = Instant::now();
    let scan = match progress.as_mut() {
        Some(report) => scan_library_with_progress(cfg, Some(&mut **report)),
        None => scan_library(cfg),
    };
    let stats = LibraryScanStats {
        scan_us: scan_t.elapsed().as_micros() as u64,
        discover_us: scan.discover_us,
        classify_us: scan.classify_us,
        normal_files: scan.normal_files.len(),
        containers: scan.containers.len(),
        entries: scan.entries.len(),
        discoveries: unique_discovery_count(&scan.discoveries),
    };
    LibraryScanArtifact { scan, stats, stamp }
}

pub(crate) fn save_scan_artifact_to_sqlite(
    cfg: &BenchConfig,
    artifact: LibraryScanArtifact,
    progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    let import_t = Instant::now();
    let bytes = save_sqlite_scan_with_progress_and_stamp(
        &cfg.sqlite_path,
        &artifact.scan,
        Some(&artifact.stamp),
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
        discoveries: artifact.stats.discoveries,
    })
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
    let mut rows = Vec::<CatalogRow>::new();
    for (key, discovery) in discoveries {
        let system_id = catalog_system_id_for_discovery(discovery);
        let plan_launch_ref = launch_ref_for_discovery(&key, discovery);
        if !is_launcher_launch_ref(&plan_launch_ref) {
            continue;
        }
        let (setname, parent) = catalog_family_fields_for_discovery(discovery, arcade_metadata);
        rows.push(CatalogRow {
            game: ArcadeGameEntry {
                title: discovery.title.clone().into(),
                mra_path: plan_launch_ref.into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: system_id.into(),
            },
            source_kind: launch_kind_for_discovery(discovery).to_string(),
            setname,
            parent,
            family_key: None,
        });
    }
    rows.sort_by_cached_key(|row| row.game.title.to_ascii_lowercase());
    let games = collapse_catalog_variants(rows);
    let systems = arcade_catalog::systems_from_games(&games);
    ArcadeCatalog::new(root.as_ref().to_path_buf(), games, systems)
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

#[derive(Default)]
struct ScanTimingStats {
    profile_match_us: u64,
    profile_match_count: usize,
    file_discovery_us: u64,
    file_discovery_count: usize,
    archive_toc_us: u64,
    archive_toc_count: usize,
    installed_collection_us: u64,
    installed_collection_count: usize,
    collection_listing_us: u64,
    collection_listing_count: usize,
}

fn scan_library_with_progress(
    cfg: &BenchConfig,
    mut progress: ProgressCallback<'_>,
) -> LibraryScan {
    let discover_t = Instant::now();
    let rx = catalog_scan::discover_files_pipelined(cfg.roots.clone());
    let profiles = launch_profiles::builtin_profiles();
    let mut discover_us = 0;
    if let Some(report) = progress.as_mut() {
        report(
            "Classifying library",
            "Walking candidates and parsing metadata...",
        );
    }

    let mut normal_files = Vec::new();
    let mut containers = Vec::new();
    let mut entries = Vec::new();
    let mut ignored_files = 0usize;
    let mut discoveries = Vec::new();
    let classify_t = Instant::now();
    let mut timing = ScanTimingStats::default();
    let mut idx = 0usize;
    while let Ok(event) = rx.recv() {
        let f = match event {
            DiscoveryEvent::File(file) => file,
            DiscoveryEvent::Done {
                discover_us: us, ..
            } => {
                discover_us = us;
                break;
            }
        };
        if idx.is_multiple_of(250) {
            if let Some(report) = progress.as_mut() {
                report(
                    "Classifying library",
                    &format!("Games found: {}", discoveries.len()),
                );
            }
        }
        idx += 1;
        let profile_match_t = Instant::now();
        let profile_match = catalog_scan::classify_profile_path(&profiles, &f.path);
        timing.profile_match_us += profile_match_t.elapsed().as_micros() as u64;
        timing.profile_match_count += 1;
        match profile_match {
            Some((
                profile,
                ProfilePathClass::Payload {
                    rule:
                        payload_rule @ PayloadRule {
                            disposition: PayloadDisposition::Playable,
                            ..
                        },
                },
            )) => {
                if media_metadata::is_amigavision_save_media_path(&f.path) {
                    ignored_files += 1;
                    continue;
                }
                let installed_t = Instant::now();
                let installed =
                    media_metadata::installed_amigavision_discoveries_from_hdf(&f, profile);
                timing.installed_collection_us += installed_t.elapsed().as_micros() as u64;
                timing.installed_collection_count += 1;
                if let Some(installed) = installed {
                    ignored_files += 1;
                    discoveries.extend(installed);
                    continue;
                }
                let mut has_archive_entries = false;
                if let Some(format) = ArchiveFormat::from_ext(&f.ext) {
                    let archive_t = Instant::now();
                    let scan = catalog_scan::scan_archive_toc(&f, format, profile);
                    timing.archive_toc_us += archive_t.elapsed().as_micros() as u64;
                    timing.archive_toc_count += 1;
                    has_archive_entries = !scan.entries.is_empty();
                    for entry in scan.entries {
                        discoveries.push(discovery_from_profile_archive_entry(
                            &entry,
                            profile,
                            &entry.rule,
                        ));
                        entries.push(entry);
                    }
                    containers.push(scan.container);
                }
                if has_archive_entries {
                    continue;
                }
                normal_files.push(LibraryPayloadFile {
                    path: f.path.display().to_string(),
                    profile_id: profile.id.to_string(),
                    size: f.size,
                    mtime_secs: f.mtime_secs,
                    rule: payload_rule,
                });
                let discovery_t = Instant::now();
                discoveries.push(discovery_from_profile_file(
                    &f,
                    profile,
                    &payload_rule,
                    &profiles,
                ));
                timing.file_discovery_us += discovery_t.elapsed().as_micros() as u64;
                timing.file_discovery_count += 1;
            }
            Some((
                profile,
                ProfilePathClass::Payload {
                    rule:
                        payload_rule @ PayloadRule {
                            disposition: PayloadDisposition::AttachedMedia,
                            ..
                        },
                },
            )) => {
                normal_files.push(LibraryPayloadFile {
                    path: f.path.display().to_string(),
                    profile_id: profile.id.to_string(),
                    size: f.size,
                    mtime_secs: f.mtime_secs,
                    rule: payload_rule,
                });
                ignored_files += 1;
            }
            Some((profile, ProfilePathClass::Collection { rule })) => {
                if let Some(format) = ArchiveFormat::from_ext(&f.ext) {
                    containers.push(catalog_scan::scan_container_header(&f, format));
                }
                let collection_t = Instant::now();
                discoveries.extend(media_metadata::collection_discoveries_from_container(
                    &f, profile, &rule,
                ));
                timing.collection_listing_us += collection_t.elapsed().as_micros() as u64;
                timing.collection_listing_count += 1;
            }
            Some((_profile, ProfilePathClass::Ignored { .. })) => {
                ignored_files += 1;
            }
            Some((_, ProfilePathClass::NotMatched)) | None => {}
        }
    }
    if discover_us == 0 {
        discover_us = discover_t.elapsed().as_micros() as u64;
    }
    report_library_scan_timing("walk", discover_us, format!("candidates={idx}"));
    report_library_scan_timing(
        "profile_match",
        timing.profile_match_us,
        format!("calls={}", timing.profile_match_count),
    );
    report_library_scan_timing(
        "installed_collection",
        timing.installed_collection_us,
        format!("calls={}", timing.installed_collection_count),
    );
    report_library_scan_timing(
        "archive_toc",
        timing.archive_toc_us,
        format!("containers={}", timing.archive_toc_count),
    );
    report_library_scan_timing(
        "collection_listings",
        timing.collection_listing_us,
        format!("collections={}", timing.collection_listing_count),
    );
    report_library_scan_timing(
        "file_discovery",
        timing.file_discovery_us,
        format!("files={}", timing.file_discovery_count),
    );
    report_library_scan_timing(
        "classify_total",
        classify_t.elapsed().as_micros() as u64,
        format!(
            "discoveries={} normal_files={} containers={} entries={}",
            discoveries.len(),
            normal_files.len(),
            containers.len(),
            entries.len()
        ),
    );
    LibraryScan {
        version: SCHEMA_VERSION,
        scanned_at_unix: unix_now_secs(),
        normal_files,
        containers,
        entries,
        ignored_files,
        discoveries,
        discover_us,
        classify_us: classify_t.elapsed().as_micros() as u64,
    }
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
    use crate::test_support::*;

    #[test]
    fn catalog_variants_group_by_parent_and_prefer_us_release() {
        let rows = vec![
            catalog_row(
                "Moon Patrol (Japan)",
                "/media/fat/_Arcade/Moon Patrol (Japan).mra",
                "mpatrolj",
                "mpatrol",
            ),
            catalog_row(
                "Moon Patrol (prototype)",
                "/media/fat/_Arcade/Moon Patrol (prototype).mra",
                "mpatrolp",
                "mpatrol",
            ),
            catalog_row(
                "Moon Patrol (US)",
                "/media/fat/_Arcade/Moon Patrol (US).mra",
                "mpatrol",
                "",
            ),
        ];

        let games = collapse_catalog_variants(rows);

        assert_eq!(games.len(), 1);
        assert_eq!(games[0].title.as_ref(), "Moon Patrol (US)");
    }

    #[test]
    fn catalog_variants_keep_non_mra_launchers_separate() {
        let rows = vec![
            catalog_launcher_row("Amiga", "/media/fat/_Computer/Amiga.mgl"),
            catalog_launcher_row("Amiga 500", "/media/fat/_Computer/Amiga 500.mgl"),
        ];

        let games = collapse_catalog_variants(rows);

        assert_eq!(games.len(), 2);
    }

    #[test]
    fn catalog_entries_with_shared_collection_launch_ref_stay_separate() {
        let rows = vec![
            catalog_entry_row("Agony", "/media/fat/games/Amiga/AmigaVision-MiSTer.7z"),
            catalog_entry_row(
                "Alien Breed",
                "/media/fat/games/Amiga/AmigaVision-MiSTer.7z",
            ),
        ];

        let games = collapse_catalog_variants(rows);

        assert_eq!(games.len(), 2);
        assert!(games.iter().any(|game| game.title.as_ref() == "Agony"));
        assert!(games
            .iter()
            .any(|game| game.title.as_ref() == "Alien Breed"));
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
    #[cfg(unix)]
    #[cfg(unix)]
    #[test]
    fn nes_software_identity_matches_title_and_preview_pack() {
        let root = unique_temp_dir("nes-software-identity");
        let rom_path = root.join("Super Mario Bros.nes");
        let mut rom = b"NES\x1a".to_vec();
        rom.extend_from_slice(&[0; 12]);
        rom.extend_from_slice(b"fixture-rom");
        std::fs::write(&rom_path, &rom).expect("write rom");
        let stripped = &rom[16..];
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
            &[("nes", "smb", stripped.len() as i64, crc32(stripped))],
        );
        let db = root.join("library.sqlite3");
        let mut discovery = payload(&rom_path.display().to_string());
        discovery.platform_id = "nes".to_string();
        discovery.category = "Console".to_string();
        discovery.core_id = "NES".to_string();
        discovery.hardware_id = "nes".to_string();
        discovery.title = "Super Mario Bros. (USA)".to_string();
        let pack = preview_worker::PreviewArchiveIndex {
            path: "/media/fat/mister-magik/assets/nes-screenshots.mmlz4b".to_string(),
            codec: "mmlz4b",
            entries: vec![software_asset_key("nes", "smb")],
        };

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let row: (String, String, String, String, String, i64, String) = conn
            .query_row(
                "SELECT i.namespace,i.identity_id,i.family_id,l.preview_archive_path,l.preview_asset_key,l.has_preview,r.confidence
                 FROM launchable_identities i
                 JOIN launchables lb ON lb.launchable_id=i.launchable_id
                 JOIN launcher_catalog l ON l.launch_ref=lb.launch_ref
                 JOIN region_metadata r ON r.game_id=i.launchable_id
                 WHERE i.namespace='mame-software'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .expect("software identity row");

        assert_eq!(
            row,
            (
                "mame-software".to_string(),
                "nes:smb".to_string(),
                "nes:smb".to_string(),
                "/media/fat/mister-magik/assets/nes-screenshots.mmlz4b".to_string(),
                software_asset_key("nes", "smb"),
                1,
                "filename".to_string()
            )
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn software_identity_title_match_skips_content_hash() {
        let mut metadata = MameSoftwareMetadata::default();
        metadata.items.insert(
            ("snes".to_string(), "example".to_string()),
            MameSoftwareItemMetadata {
                description: "Example Game (USA)".to_string(),
                year: Some("1992".to_string()),
                publisher: Some("Example".to_string()),
                region: Some("usa".to_string()),
                parent_name: None,
            },
        );
        metadata.title_index.insert(
            ("snes".to_string(), "example-game".to_string()),
            vec!["example".to_string()],
        );
        let mut discovery = payload("/media/fat/games/SNES/Example Game (USA).sfc");
        discovery.platform_id = "snes".to_string();
        discovery.title = "Example Game (USA)".to_string();

        let identity = mame_software_identity_for_discovery_with_hash_matcher(
            &discovery,
            &metadata,
            |_, _, _| panic!("title match should not read or hash payload content"),
        )
        .expect("title identity");

        assert_eq!(identity.list_name, "snes");
        assert_eq!(identity.software_name, "example");
        assert_eq!(identity.source, "filename");
    }

    #[test]
    fn software_identity_hash_match_is_disabled_by_default() {
        let root = unique_temp_dir("software-hash-disabled");
        let rom_path = root.join("Fixture.sfc");
        std::fs::write(&rom_path, b"fixture-rom").expect("write rom");
        let mut metadata = MameSoftwareMetadata::default();
        metadata.hash_index.insert(
            ("snes".to_string(), 11, crc32(b"fixture-rom")),
            vec!["fixture".to_string()],
        );
        let discovery = payload(&rom_path.display().to_string());

        let mut cache = SoftwareHashCache::default();
        let matched = match_software_by_file_hash_with_cache(
            &discovery, "snes", &metadata, false, &mut cache,
        );

        assert_eq!(matched, None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn software_identity_hash_match_can_be_enabled() {
        let root = unique_temp_dir("software-hash-enabled");
        let rom_path = root.join("Fixture.sfc");
        std::fs::write(&rom_path, b"fixture-rom").expect("write rom");
        let mut metadata = MameSoftwareMetadata::default();
        metadata.hash_index.insert(
            ("snes".to_string(), 11, crc32(b"fixture-rom")),
            vec!["fixture".to_string()],
        );
        let discovery = payload(&rom_path.display().to_string());

        let mut cache = SoftwareHashCache::default();
        let matched =
            match_software_by_file_hash_with_cache(&discovery, "snes", &metadata, true, &mut cache);

        assert_eq!(matched, Some("fixture".to_string()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn software_identity_hash_cache_hit_avoids_file_read() {
        let root = unique_temp_dir("software-hash-cache-hit");
        let payload_dir = root.join("Cached.sfc");
        std::fs::create_dir(&payload_dir).expect("create payload dir");
        let signature = file_signature(&payload_dir);
        let db = root.join("library.sqlite3");
        write_software_hash_cache_fixture(
            &db,
            &[(
                "snes",
                &payload_dir.display().to_string(),
                signature.size,
                signature.mtime_secs,
                Some("cached"),
            )],
        );
        let mut cache = SoftwareHashCache::load(&db);
        let mut metadata = MameSoftwareMetadata::default();
        metadata
            .hash_index
            .insert(("snes".to_string(), 123, 456), vec!["wrong".to_string()]);
        let discovery = payload(&payload_dir.display().to_string());

        let matched =
            match_software_by_file_hash_with_cache(&discovery, "snes", &metadata, true, &mut cache);

        assert_eq!(matched, Some("cached".to_string()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn software_identity_hash_cache_stale_signature_recomputes() {
        let root = unique_temp_dir("software-hash-cache-stale");
        let rom_path = root.join("Fixture.sfc");
        std::fs::write(&rom_path, b"fresh-rom").expect("write rom");
        let signature = file_signature(&rom_path);
        let db = root.join("library.sqlite3");
        write_software_hash_cache_fixture(
            &db,
            &[(
                "snes",
                &rom_path.display().to_string(),
                signature.size + 1,
                signature.mtime_secs,
                Some("stale"),
            )],
        );
        let mut cache = SoftwareHashCache::load(&db);
        let mut metadata = MameSoftwareMetadata::default();
        metadata.hash_index.insert(
            ("snes".to_string(), 9, crc32(b"fresh-rom")),
            vec!["fresh".to_string()],
        );
        let discovery = payload(&rom_path.display().to_string());

        let matched =
            match_software_by_file_hash_with_cache(&discovery, "snes", &metadata, true, &mut cache);

        assert_eq!(matched, Some("fresh".to_string()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn console_preview_uses_parent_family_fallback() {
        let root = unique_temp_dir("software-family-preview");
        let rom_path = root.join("Variant.sfc");
        std::fs::write(&rom_path, b"variant-rom").expect("write rom");
        let mame_db = root.join("mame.sqlite3");
        write_mame_software_fixture_db(
            &mame_db,
            &[
                (
                    "snes",
                    "parent",
                    None,
                    "Example Game (USA)",
                    Some("1992"),
                    Some("Example"),
                    Some("usa"),
                ),
                (
                    "snes",
                    "child",
                    Some("parent"),
                    "Example Game (Rev 1) (USA)",
                    Some("1992"),
                    Some("Example"),
                    Some("usa"),
                ),
            ],
            &[("snes", "child", 11, crc32(b"variant-rom"))],
        );
        let db = root.join("library.sqlite3");
        let mut discovery = payload(&rom_path.display().to_string());
        discovery.platform_id = "snes".to_string();
        discovery.category = "Console".to_string();
        discovery.core_id = "SNES".to_string();
        discovery.hardware_id = "snes".to_string();
        discovery.title = "Example Game (Rev 1) (USA)".to_string();
        let pack = preview_worker::PreviewArchiveIndex {
            path: "/media/fat/mister-magik/assets/snes-screenshots.mmlz4b".to_string(),
            codec: "mmlz4b",
            entries: vec![software_asset_key("snes", "parent")],
        };

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let row: (String, String, i64, String) = conn
            .query_row(
                "SELECT preview_archive_path,preview_asset_key,has_preview,system_id FROM launcher_catalog",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("launcher row");

        assert_eq!(
            row.0,
            "/media/fat/mister-magik/assets/snes-screenshots.mmlz4b"
        );
        assert_eq!(row.1, software_asset_key("snes", "parent"));
        assert_eq!(row.2, 1);
        assert_eq!(row.3, "snes");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn console_preview_pack_platform_distinguishes_nes_and_snes() {
        assert_eq!(
            preview_asset_pack_platform("/media/fat/mister-magik/assets/nes-screenshots.mmlz4b"),
            "nes"
        );
        assert_eq!(
            preview_asset_pack_platform("/media/fat/mister-magik/assets/snes-screenshots.mmlz4b"),
            "snes"
        );
    }

    #[test]
    fn console_preview_derives_key_without_reading_pack_entries() {
        let root = unique_temp_dir("derived-console-preview");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_software_fixture_db(
            &mame_db,
            &[(
                "saturn",
                "albert",
                None,
                "Albert Odyssey: Legend of Eldean (USA)",
                Some("1997"),
                Some("Working Designs"),
                Some("usa"),
            )],
            &[],
        );
        let mut discovery = saturn_payload("/media/fat/games/Saturn/Albert Odyssey.chd");
        discovery.title = "Albert Odyssey: Legend of Eldean (USA)".to_string();
        let pack = preview_worker::PreviewArchiveIndex {
            path: "/media/fat/mister-magik/assets/saturn-screenshots.mmlz4b".to_string(),
            codec: "mmlz4b",
            entries: vec!["albert-odyssey-legend-of-eldean-us".to_string()],
        };

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let row: (String, String, i64) = conn
            .query_row(
                "SELECT l.preview_archive_path,l.preview_asset_key,l.has_preview
                 FROM launcher_catalog l
                 WHERE l.system_id='saturn'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("launcher row");

        assert_eq!(
            row,
            (
                "/media/fat/mister-magik/assets/saturn-screenshots.mmlz4b".to_string(),
                software_asset_key("saturn", "albert"),
                1
            )
        );
        assert!(!sqlite_table_exists(&conn, "asset_entries").expect("check asset_entries table"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn console_preview_ignores_wrong_system_canonical_entries() {
        let root = unique_temp_dir("wrong-system-console-preview");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        let rom_path = root.join("Fixture.nes");
        std::fs::write(&rom_path, b"fixture-rom").expect("write rom");
        write_mame_software_fixture_db(
            &mame_db,
            &[(
                "nes",
                "fixture",
                None,
                "Fixture Game (USA)",
                Some("1985"),
                Some("Example"),
                Some("usa"),
            )],
            &[("nes", "fixture", 11, crc32(b"fixture-rom"))],
        );
        let mut discovery = payload(&rom_path.display().to_string());
        discovery.platform_id = "nes".to_string();
        discovery.category = "Console".to_string();
        discovery.core_id = "NES".to_string();
        discovery.hardware_id = "nes".to_string();
        let pack = preview_worker::PreviewArchiveIndex {
            path: "/media/fat/mister-magik/assets/saturn-screenshots.mmlz4b".to_string(),
            codec: "mmlz4b",
            entries: vec![software_asset_key("nes", "fixture")],
        };

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let row: (String, String, i64) = conn
            .query_row(
                "SELECT preview_archive_path,preview_asset_key,has_preview FROM launcher_catalog",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("launcher row");

        assert_eq!(row, (String::new(), String::new(), 0));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn saturn_software_identity_matches_chd_raw_sha1() {
        let root = unique_temp_dir("saturn-chd-identity");
        let chd_path = root.join("Disc.chd");
        let sha1 = [0x42u8; 20];
        let mut header = [0u8; 124];
        header[..8].copy_from_slice(b"MComprHD");
        header[8..12].copy_from_slice(&124u32.to_be_bytes());
        header[12..16].copy_from_slice(&5u32.to_be_bytes());
        header[56..60].copy_from_slice(&4096u32.to_be_bytes());
        header[60..64].copy_from_slice(&2448u32.to_be_bytes());
        header[64..84].copy_from_slice(&sha1);
        std::fs::write(&chd_path, header).expect("write chd header");
        let mame_db = root.join("mame.sqlite3");
        write_mame_software_fixture_db(
            &mame_db,
            &[(
                "saturn",
                "nights",
                None,
                "Nights into Dreams (USA)",
                Some("1996"),
                Some("Sega"),
                Some("usa"),
            )],
            &[],
        );
        let conn = Connection::open(&mame_db).expect("open mame fixture");
        conn.execute(
            "INSERT INTO mame_software_hashes(list_name,software_name,disk_sha1)
             VALUES ('saturn','nights',?1)",
            [hex_lower(&sha1)],
        )
        .expect("insert disk hash");
        drop(conn);
        let db = root.join("library.sqlite3");
        let mut discovery = saturn_payload(&chd_path.display().to_string());
        discovery.title = "Untrusted Scraper Name".to_string();

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let identity: String = conn
            .query_row(
                "SELECT identity_id FROM launchable_identities WHERE namespace='mame-software'",
                [],
                |row| row.get(0),
            )
            .expect("software identity");
        assert_eq!(identity, "saturn:nights");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn saturn_multidisc_software_identity_materializes_one_launcher_game() {
        let root = unique_temp_dir("saturn-multidisc-identity");
        let disc1_path = root.join("Fixture RPG (Disc 1).chd");
        let disc2_path = root.join("Fixture RPG (Disc 2).chd");
        let sha1_disc1 = [0x41u8; 20];
        let sha1_disc2 = [0x42u8; 20];
        std::fs::write(&disc1_path, chd_v5_header(sha1_disc1)).expect("write disc 1 chd");
        std::fs::write(&disc2_path, chd_v5_header(sha1_disc2)).expect("write disc 2 chd");

        let mame_db = root.join("mame.sqlite3");
        write_mame_software_fixture_db(
            &mame_db,
            &[(
                "saturn",
                "fixturerpg",
                None,
                "Fixture RPG (USA)",
                Some("1997"),
                Some("Example"),
                Some("usa"),
            )],
            &[],
        );
        let conn = Connection::open(&mame_db).expect("open mame fixture");
        conn.execute(
            "INSERT INTO mame_software_hashes(list_name,software_name,disk_sha1)
             VALUES ('saturn','fixturerpg',?1)",
            [hex_lower(&sha1_disc1)],
        )
        .expect("insert disc 1 hash");
        conn.execute(
            "INSERT INTO mame_software_hashes(list_name,software_name,disk_sha1)
             VALUES ('saturn','fixturerpg',?1)",
            [hex_lower(&sha1_disc2)],
        )
        .expect("insert disc 2 hash");
        drop(conn);

        let mut disc1 = saturn_payload(&disc1_path.display().to_string());
        disc1.title = "Fixture RPG Disc 1".to_string();
        let mut disc2 = saturn_payload(&disc2_path.display().to_string());
        disc2.title = "Fixture RPG Disc 2".to_string();
        let db = root.join("library.sqlite3");

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![disc2, disc1]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let launcher: (i64, String, String) = conn
            .query_row(
                "SELECT
                    (SELECT count(*) FROM launcher_catalog WHERE system_id='saturn'),
                    title,
                    launch_ref
                 FROM launcher_catalog
                 WHERE system_id='saturn'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("saturn launcher row");
        let identity_count: i64 = conn
            .query_row(
                "SELECT count(*)
                 FROM launchable_identities
                 WHERE namespace='mame-software'
                   AND identity_id='saturn:fixturerpg'",
                [],
                |row| row.get(0),
            )
            .expect("software identity count");

        assert_eq!(launcher.0, 1);
        assert_eq!(launcher.1, "Fixture RPG Disc 1");
        assert!(launcher.2.ends_with("Fixture RPG (Disc 1).chd"));
        assert_eq!(identity_count, 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rom_normalization_covers_snes_and_n64_byte_orders() {
        let snes = [0xaa; 512]
            .into_iter()
            .chain(b"plain-snes".iter().copied())
            .collect::<Vec<_>>();
        assert!(rom_hash_candidates("snes", &snes)
            .iter()
            .any(|candidate| candidate == b"plain-snes"));

        let z64 = [0x12, 0x34, 0x56, 0x78];
        let candidates = rom_hash_candidates("n64", &z64);
        assert!(candidates
            .iter()
            .any(|candidate| candidate == &[0x34, 0x12, 0x78, 0x56]));
        assert!(candidates
            .iter()
            .any(|candidate| candidate == &[0x56, 0x78, 0x12, 0x34]));
    }

    #[test]
    fn arcade_mra_identity_uses_mame_parent_family() {
        let root = unique_temp_dir("arcade-mame-identity");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[
                (
                    "1942",
                    None,
                    "1942 (Revision B)",
                    Some("1984"),
                    Some("Capcom"),
                ),
                (
                    "1942b",
                    Some("1942"),
                    "1942 (First Version)",
                    Some("1984"),
                    Some("Capcom"),
                ),
            ],
        );
        let mut discovery = mra_discovery(1, "1942 (First Version)");
        discovery.setname = Some("1942b".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let row = conn
            .query_row(
                "SELECT l.system_id,l.launch_kind,i.identity_id,i.family_id,i.metadata_title,i.year,i.manufacturer,i.source
                 FROM launchables l
                 JOIN launchable_identities i ON i.launchable_id=l.launchable_id",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .expect("query identity row");

        assert_eq!(row.0, "arcade");
        assert_eq!(row.1, "mra");
        assert_eq!(row.2, "1942b");
        assert_eq!(row.3, "1942");
        assert_eq!(row.4.as_deref(), Some("1942 (First Version)"));
        assert_eq!(row.5.as_deref(), Some("1984"));
        assert_eq!(row.6.as_deref(), Some("Capcom"));
        assert_eq!(row.7, "mame");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn neogeo_mgl_identity_uses_mame_setname() {
        let root = unique_temp_dir("neogeo-mame-identity");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[(
                "mslug3",
                None,
                "Metal Slug 3 (NGM-2560)",
                Some("2000"),
                Some("SNK"),
            )],
        );
        let path = "/media/fat/_Games/_Neo Geo MVS & AES/Metal Slug 3 (mslug3).mgl";
        let mut discovery = mgl(path, path);
        discovery.title = "Metal Slug 3".to_string();
        discovery.platform_id = "neogeo".to_string();
        discovery.core_id = "neogeo".to_string();
        discovery.hardware_id = "neogeo".to_string();
        discovery.setname = Some("mslug3".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let row = conn
            .query_row(
                "SELECT identity_id,family_id,metadata_title,year,manufacturer,source
                 FROM launchable_identities",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .expect("query identity row");

        assert_eq!(row.0, "mslug3");
        assert_eq!(row.1, "mslug3");
        assert_eq!(row.2.as_deref(), Some("Metal Slug 3 (NGM-2560)"));
        assert_eq!(row.3.as_deref(), Some("2000"));
        assert_eq!(row.4.as_deref(), Some("SNK"));
        assert_eq!(row.5, "mame");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unknown_mame_identity_remains_launchable_without_enrichment() {
        let root = unique_temp_dir("unknown-mame-identity");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(&mame_db, &[]);
        let mut discovery = mra_discovery(1, "Mystery Arcade Game");
        discovery.setname = Some("mystery".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![discovery]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let launchable_count: i64 = conn
            .query_row("SELECT count(*) FROM launchables", [], |row| row.get(0))
            .expect("query launchable count");
        let row = conn
            .query_row(
                "SELECT identity_id,family_id,metadata_title,year,manufacturer,source
                 FROM launchable_identities",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .expect("query identity row");

        assert_eq!(launchable_count, 1);
        assert_eq!(row.0, "mystery");
        assert_eq!(row.1, "mystery");
        assert!(row.2.is_none());
        assert!(row.3.is_none());
        assert!(row.4.is_none());
        assert_eq!(row.5, "setname");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn arcade_identity_uses_hbmame_metadata_after_mame_miss() {
        let root = unique_temp_dir("hbmame-identity");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        let hbmame_db = root.join("hbmame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[("bombjack", None, "Bomb Jack", Some("1984"), Some("Tehkan"))],
        );
        write_mame_fixture_db(
            &hbmame_db,
            &[(
                "bombjckb",
                Some("bombjack"),
                "Bomb Jack (Bootleg)",
                Some("1984"),
                Some("Tehkan"),
            )],
        );
        let mut parent = mra_discovery(1, "Bomb Jack");
        parent.setname = Some("bombjack".to_string());
        let mut hbmame_clone = mra_discovery(2, "Bomb Jack");
        hbmame_clone.setname = Some("bombjckb".to_string());
        hbmame_clone.parent = Some("bombjack".to_string());
        hbmame_clone.source_path =
            "/media/fat/_Arcade/_alternatives/_Bomb Jack/Bomb Jack (Bootleg) - HBMame.mra"
                .to_string();
        hbmame_clone.launch_ref = hbmame_clone.source_path.clone();

        write_sqlite_scan_with_mame_and_hbmame(
            &db,
            &sqlite_scan_with_discoveries(vec![parent, hbmame_clone]),
            &mame_db,
            &hbmame_db,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let identity = conn
            .query_row(
                "SELECT identity_id,family_id,metadata_title,manufacturer,source
                 FROM launchable_identities
                 WHERE identity_id='bombjckb'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .expect("query hbmame identity");
        assert_eq!(identity.0, "bombjckb");
        assert_eq!(identity.1, "bombjack");
        assert_eq!(identity.2.as_deref(), Some("Bomb Jack (Bootleg)"));
        assert_eq!(identity.3.as_deref(), Some("Tehkan"));
        assert_eq!(identity.4, "hbmame");

        let preferred_count: i64 = conn
            .query_row("SELECT count(*) FROM ui_arcade_preferred", [], |row| {
                row.get(0)
            })
            .expect("query preferred count");
        let variant_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_variants WHERE family_id='bombjack'",
                [],
                |row| row.get(0),
            )
            .expect("query variant count");
        assert_eq!(preferred_count, 1);
        assert_eq!(variant_count, 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn hbmame_metadata_from_library_uses_mra_parent_rows() {
        let root = unique_temp_dir("hbmame-from-library");
        let db = root.join("library.sqlite3");
        let hbmame_db = root.join("hbmame.sqlite3");
        let mut parent = mra_discovery(1, "Bomb Jack");
        parent.setname = Some("bombjack".to_string());
        parent.parent = Some("bombjack".to_string());
        let mut hbmame_clone = mra_discovery(2, "Bomb Jack");
        hbmame_clone.setname = Some("bombjckb".to_string());
        hbmame_clone.parent = Some("bombjack".to_string());

        save_sqlite_scan(
            &db,
            &sqlite_scan_with_discoveries(vec![parent, hbmame_clone]),
        )
        .expect("save sqlite");
        let summary =
            write_hbmame_metadata_from_library(&db, &hbmame_db).expect("write hbmame metadata");
        assert_eq!(summary.rows, 1);

        let conn = Connection::open(&hbmame_db).expect("open hbmame db");
        let row = conn
            .query_row(
                "SELECT parent_setname,title FROM mame_machines WHERE setname='bombjckb'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("query hbmame row");
        assert_eq!(row.0, "bombjack");
        assert_eq!(row.1, "Bomb Jack");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn arcade_parent_override_collapses_mvsc_unlocked_variants() {
        let root = unique_temp_dir("arcade-parent-override-mvsc");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[(
                "mvsc",
                None,
                "Marvel Vs. Capcom: Clash of Super Heroes (Europe 980123)",
                Some("1998"),
                Some("Capcom"),
            )],
        );
        let pack = preview_worker::PreviewArchiveIndex {
            path: root.join("arcade-screenshots.mmlz4b").display().to_string(),
            codec: "lz4-block",
            entries: vec!["mvsc".to_string()],
        };
        let mut parent = mra_discovery(1, "Marvel Vs. Capcom: Clash of Super Heroes");
        parent.setname = Some("mvsc".to_string());
        let mut variants = (1..=4)
            .map(|idx| {
                let mut discovery = mra_discovery(
                    idx + 1,
                    &format!("Marvel Vs. Capcom: Clash of Super Heroes [Unlocked {idx}]"),
                );
                discovery.setname = Some(format!("mvsc_{idx}"));
                discovery.source_path = format!(
                    "/media/fat/_Arcade/_Arcade Offset/_CP System II/_Unlocked/mvsc_{idx}.mra"
                );
                discovery.launch_ref = discovery.source_path.clone();
                discovery
            })
            .collect::<Vec<_>>();
        let mut discoveries = vec![parent];
        discoveries.append(&mut variants);

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(discoveries),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let preferred_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_preferred WHERE family_id='mvsc'",
                [],
                |row| row.get(0),
            )
            .expect("query preferred mvsc count");
        let variant_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_variants WHERE family_id='mvsc'",
                [],
                |row| row.get(0),
            )
            .expect("query mvsc variant count");
        let override_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM launchable_identities
                 WHERE family_id='mvsc' AND source='arcade-parent-override'",
                [],
                |row| row.get(0),
            )
            .expect("query override identity count");
        let missing_preview_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_variants
                 WHERE family_id='mvsc' AND has_preview=0",
                [],
                |row| row.get(0),
            )
            .expect("query mvsc missing previews");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");

        assert_eq!(preferred_count, 1);
        assert_eq!(variant_count, 5);
        assert_eq!(override_count, 4);
        assert_eq!(missing_preview_count, 0);
        assert_eq!(loaded.catalog.system_game_count("arcade"), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn arcade_parent_override_collapses_street_fighter_offset_variants() {
        let root = unique_temp_dir("arcade-parent-override-street-fighter");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[
                (
                    "hsf2",
                    None,
                    "Hyper Street Fighter II: The Anniversary Edition (USA 040202)",
                    Some("2004"),
                    Some("Capcom"),
                ),
                (
                    "sf2ce",
                    None,
                    "Street Fighter II': Champion Edition (World 920513)",
                    Some("1992"),
                    Some("Capcom"),
                ),
            ],
        );
        let pack = preview_worker::PreviewArchiveIndex {
            path: root.join("arcade-screenshots.mmlz4b").display().to_string(),
            codec: "lz4-block",
            entries: vec!["hsf2".to_string(), "sf2ce".to_string()],
        };
        let mut hsf2 = mra_discovery(1, "Hyper Street Fighter II");
        hsf2.setname = Some("hsf2".to_string());
        let mut sf2ce = mra_discovery(2, "Street Fighter II': Champion Edition");
        sf2ce.setname = Some("sf2ce".to_string());
        let aliases = [
            ("hsf2j1gouki", "hsf2"),
            ("hsf2j1tgouki", "hsf2"),
            ("sf2ceaimedb", "sf2ce"),
            ("sf2ceaimedf", "sf2ce"),
            ("sf2cebfire", "sf2ce"),
            ("sf2cebih", "sf2ce"),
            ("sf2cebof", "sf2ce"),
            ("sf2cefires", "sf2ce"),
            ("sf2ces15", "sf2ce"),
            ("sf2ces17", "sf2ce"),
            ("sf2ces21", "sf2ce"),
            ("sf2ces22", "sf2ce"),
            ("sf2ces23", "sf2ce"),
            ("sf2cevampiric", "sf2ce"),
        ];
        let mut discoveries = vec![hsf2, sf2ce];
        discoveries.extend(aliases.iter().enumerate().map(|(idx, (alias, _))| {
            let mut discovery =
                mra_discovery(idx + 3, &format!("Street Fighter offset variant {alias}"));
            discovery.setname = Some((*alias).to_string());
            discovery.source_path = format!("/media/fat/_Arcade/_Arcade Offset/{alias}.mra");
            discovery.launch_ref = discovery.source_path.clone();
            discovery
        }));

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(discoveries),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let preferred_count: i64 = conn
            .query_row("SELECT count(*) FROM ui_arcade_preferred", [], |row| {
                row.get(0)
            })
            .expect("query preferred count");
        let override_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM launchable_identities
                 WHERE source='arcade-parent-override'
                   AND family_id IN ('hsf2','sf2ce')",
                [],
                |row| row.get(0),
            )
            .expect("query override identity count");
        let hsf2_variants: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_variants
                 WHERE family_id='hsf2' AND has_preview=1",
                [],
                |row| row.get(0),
            )
            .expect("query hsf2 variants");
        let sf2ce_variants: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_variants
                 WHERE family_id='sf2ce' AND has_preview=1",
                [],
                |row| row.get(0),
            )
            .expect("query sf2ce variants");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");

        assert_eq!(preferred_count, 2);
        assert_eq!(override_count, aliases.len() as i64);
        assert_eq!(hsf2_variants, 3);
        assert_eq!(sf2ce_variants, 13);
        assert_eq!(loaded.catalog.system_game_count("arcade"), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn arcade_mra_parent_tag_collapses_unknown_metadata_variants() {
        let root = unique_temp_dir("arcade-mra-parent-fallback");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(&mame_db, &[]);
        let mut parent = mra_discovery(1, "Mystery Parent");
        parent.setname = Some("mystery".to_string());
        let mut clone = mra_discovery(2, "Mystery Parent [Hack]");
        clone.setname = Some("mystery_hack".to_string());
        clone.parent = Some("mystery".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![parent, clone]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let clone_identity = conn
            .query_row(
                "SELECT identity_id,family_id,source
                 FROM launchable_identities
                 WHERE identity_id='mystery-hack'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .expect("query clone identity");
        let preferred_count: i64 = conn
            .query_row("SELECT count(*) FROM ui_arcade_preferred", [], |row| {
                row.get(0)
            })
            .expect("query preferred count");
        let variant_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM ui_arcade_variants WHERE family_id='mystery'",
                [],
                |row| row.get(0),
            )
            .expect("query variant count");

        assert_eq!(clone_identity.0, "mystery-hack");
        assert_eq!(clone_identity.1, "mystery");
        assert_eq!(clone_identity.2, "mra-parent");
        assert_eq!(preferred_count, 1);
        assert_eq!(variant_count, 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ui_arcade_preferred_collapses_family_and_keeps_variants() {
        let root = unique_temp_dir("ui-arcade-preferred-parent");
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

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![clone, parent]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let preferred = conn
            .query_row(
                "SELECT identity_id,family_id,preferred_reason,title,has_preview
                 FROM ui_arcade_preferred",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .expect("query preferred row");
        let variant_count: i64 = conn
            .query_row("SELECT count(*) FROM ui_arcade_variants", [], |row| {
                row.get(0)
            })
            .expect("query variant count");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");

        assert_eq!(preferred.0.as_deref(), Some("1942"));
        assert_eq!(preferred.1, "1942");
        assert_eq!(preferred.2, "installed-parent");
        assert_eq!(preferred.3, "1942");
        assert_eq!(preferred.4, 0);
        assert_eq!(variant_count, 2);
        assert_eq!(loaded.catalog.games.len(), 1);
        assert_eq!(loaded.catalog.games[0].title.as_ref(), "1942");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ui_arcade_preferred_uses_deterministic_child_when_parent_missing() {
        let root = unique_temp_dir("ui-arcade-preferred-child");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[
                (
                    "1942b",
                    Some("1942"),
                    "1942 (First Version)",
                    Some("1984"),
                    Some("Capcom"),
                ),
                (
                    "1942w",
                    Some("1942"),
                    "1942 (World)",
                    Some("1984"),
                    Some("Capcom"),
                ),
            ],
        );
        let mut first = mra_discovery(1, "1942 (First Version)");
        first.setname = Some("1942b".to_string());
        let mut world = mra_discovery(2, "1942 (World)");
        world.setname = Some("1942w".to_string());
        let pack = preview_worker::PreviewArchiveIndex {
            path: root
                .join("320x320-screenshots.mmlz4b")
                .display()
                .to_string(),
            codec: "lz4-block",
            entries: vec!["1942w".to_string()],
        };

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![first, world]),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let preferred = conn
            .query_row(
                "SELECT identity_id,family_id,preferred_reason,has_preview
                 FROM ui_arcade_preferred",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .expect("query preferred row");
        let variant_count: i64 = conn
            .query_row("SELECT count(*) FROM ui_arcade_variants", [], |row| {
                row.get(0)
            })
            .expect("query variant count");

        assert_eq!(preferred.0.as_deref(), Some("1942b"));
        assert_eq!(preferred.1, "1942");
        assert_eq!(preferred.2, "deterministic-child");
        assert_eq!(preferred.3, 1);
        assert_eq!(variant_count, 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn arcade_preview_keys_are_derived_from_family_without_pack_index() {
        let root = unique_temp_dir("arcade-family-preview-key");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(
            &mame_db,
            &[
                ("1941", None, "1941", Some("1990"), Some("Capcom")),
                (
                    "1941j",
                    Some("1941"),
                    "1941: Counter Attack (Japan)",
                    Some("1990"),
                    Some("Capcom"),
                ),
                (
                    "1941r1",
                    Some("1941"),
                    "1941: Counter Attack (World, earlier)",
                    Some("1990"),
                    Some("Capcom"),
                ),
                (
                    "1941u",
                    Some("1941"),
                    "1941: Counter Attack (USA)",
                    Some("1990"),
                    Some("Capcom"),
                ),
            ],
        );
        let pack = preview_worker::PreviewArchiveIndex {
            path: root.join("arcade-screenshots.mmlz4b").display().to_string(),
            codec: "lz4-block",
            entries: vec!["1941u".to_string()],
        };
        let mut parent = mra_discovery(1, "1941");
        parent.setname = Some("1941".to_string());
        let mut japan = mra_discovery(2, "1941: Counter Attack (Japan)");
        japan.setname = Some("1941j".to_string());
        let mut world = mra_discovery(3, "1941: Counter Attack (World, earlier)");
        world.setname = Some("1941r1".to_string());
        let mut usa = mra_discovery(4, "1941: Counter Attack (USA)");
        usa.setname = Some("1941u".to_string());

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![parent, japan, world, usa]),
            &mame_db,
            &pack,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let mut stmt = conn
            .prepare(
                "SELECT identity_id,asset_key,asset_link_reason,preview_archive_path,preview_asset_key,has_preview
                 FROM ui_arcade_variants
                 ORDER BY identity_id",
            )
            .expect("prepare variant asset query");
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .expect("query variant assets")
            .map(|row| row.expect("read variant asset row"))
            .collect::<Vec<_>>();
        let preferred = conn
            .query_row(
                "SELECT identity_id,asset_key,asset_link_reason,preview_archive_path,preview_asset_key,has_preview
                 FROM ui_arcade_preferred",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .expect("query preferred asset");

        assert_eq!(rows.len(), 4);
        for row in &rows {
            assert_eq!(row.1.as_deref(), Some("1941"));
            assert_eq!(row.2, "derived-family");
            assert_eq!(row.3, pack.path);
            assert_eq!(row.4, "1941");
            assert_eq!(row.5, 1);
        }
        assert_eq!(preferred.0.as_deref(), Some("1941"));
        assert_eq!(preferred.1.as_deref(), Some("1941"));
        assert_eq!(preferred.2, "derived-family");
        assert_eq!(preferred.3, pack.path);
        assert_eq!(preferred.4, "1941");
        assert_eq!(preferred.5, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_save_keeps_previous_database_when_replacement_fails() {
        let root = unique_temp_dir("sqlite-atomic-replace");
        let db = root.join("library.sqlite3");
        save_sqlite_scan(&db, &sqlite_scan_with_normal_files(&["/old/game.mra"]))
            .expect("write old database");
        let old_summary = sqlite_cached_summary(&db, 0).expect("old database readable");
        assert_eq!(old_summary.normal_files, 1);

        let err = save_sqlite_scan(
            &db,
            &sqlite_scan_with_normal_files(&["/new/game.mra", "/new/game.mra"]),
        )
        .expect_err("duplicate normal_files row should fail temp import");

        assert!(
            err.contains("insert payload file"),
            "unexpected error: {err}"
        );
        let still_old = sqlite_cached_summary(&db, 0).expect("old database survived failed import");
        assert_eq!(still_old.normal_files, 1);
        assert!(
            !sqlite_temp_path(&db).exists(),
            "failed temp database should be cleaned up"
        );
        let _ = std::fs::remove_dir_all(root);
    }

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
    fn sqlite_catalog_stamp_check_detects_match_and_root_change() {
        let root = unique_temp_dir("sqlite-catalog-stamp-check");
        let db = root.join("library.sqlite3");
        let games = root.join("games");
        let system = games.join("NES");
        std::fs::create_dir_all(&system).expect("create system dir");
        set_file_mtime_for_test(&games, 10, 0);
        let cfg = BenchConfig {
            roots: vec![games.display().to_string()],
            sqlite_path: db.clone(),
        };
        let artifact = scan_library_artifact(&cfg, None);
        save_scan_artifact_to_sqlite(&cfg, artifact, None).expect("save artifact");

        let unchanged = sqlite_catalog_stamp_check(&cfg).expect("check unchanged stamp");
        assert!(unchanged.unchanged);
        let summary = sqlite_cached_summary(&db, unchanged.check_us).expect("cached summary");
        assert!(summary.skipped);

        set_file_mtime_for_test(&games, 20, 0);
        let changed = sqlite_catalog_stamp_check(&cfg).expect("check changed stamp");

        assert!(!changed.unchanged);
        assert_ne!(
            changed.stored_fingerprint,
            Some(changed.current_fingerprint)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_build_temp_defaults_to_tmpfs_for_media_fat_database() {
        let path = Path::new("/media/fat/mister-magik/library.sqlite3");
        let plan = sqlite_build_temp_plan_for(path, None);

        assert_eq!(plan.source, SqliteBuildTempSource::DefaultTmpfs);
        assert!(plan
            .build_tmp_path
            .starts_with(Path::new(DEFAULT_SQLITE_BUILD_DIR)));
        let expected_name = format!(".library.sqlite3.build.{}", std::process::id());
        assert_eq!(
            plan.build_tmp_path
                .file_name()
                .and_then(|name| name.to_str()),
            Some(expected_name.as_str())
        );
        assert_eq!(
            plan.final_tmp_path,
            PathBuf::from(format!(
                "/media/fat/mister-magik/.library.sqlite3.tmp.{}",
                std::process::id()
            ))
        );
    }

    #[test]
    fn sqlite_build_temp_env_override_wins_for_media_fat_database() {
        let override_dir = Path::new("/custom/sqlite-build");
        let path = Path::new("/media/fat/mister-magik/library.sqlite3");
        let plan = sqlite_build_temp_plan_for(path, Some(override_dir));

        assert_eq!(plan.source, SqliteBuildTempSource::EnvOverride);
        assert!(plan.build_tmp_path.starts_with(override_dir));
        assert_ne!(plan.build_tmp_path, plan.final_tmp_path);
    }

    #[test]
    fn sqlite_build_temp_stays_beside_non_media_fat_database() {
        let root = unique_temp_dir("sqlite-build-host-path");
        let db = root.join("library.sqlite3");
        let plan = sqlite_build_temp_plan_for(&db, None);

        assert_eq!(plan.source, SqliteBuildTempSource::BesideFinal);
        assert_eq!(plan.build_tmp_path, sqlite_temp_path(&db));
        assert_eq!(plan.build_tmp_path, plan.final_tmp_path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_save_retries_beside_final_after_tmpfs_filesystem_error() {
        let root = unique_temp_dir("sqlite-build-fallback");
        let db = root.join("library.sqlite3");
        let build_tmp = root
            .join("tmpfs-build")
            .join(format!(".library.sqlite3.build.{}", std::process::id()));
        let initial_plan = SqliteBuildTempPlan {
            build_tmp_path: build_tmp.clone(),
            final_tmp_path: sqlite_temp_path(&db),
            source: SqliteBuildTempSource::DefaultTmpfs,
        };
        let mut attempts = Vec::<PathBuf>::new();
        let mut writer = |path: &Path,
                          _scan: &LibraryScan,
                          _progress: &mut ProgressCallback<'_>|
         -> Result<(), String> {
            attempts.push(path.to_path_buf());
            if path == build_tmp {
                return Err("database or disk is full".to_string());
            }
            std::fs::write(path, b"fallback-db").map_err(|e| e.to_string())
        };

        let bytes = save_sqlite_scan_with_progress_using_writer(
            &db,
            &sqlite_scan_with_normal_files(&[]),
            None,
            initial_plan,
            &mut writer,
        )
        .expect("fallback save");

        assert_eq!(bytes, b"fallback-db".len() as u64);
        assert_eq!(attempts, vec![build_tmp, sqlite_temp_path(&db)]);
        assert_eq!(
            std::fs::read(&db).expect("read fallback db"),
            b"fallback-db"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_save_does_not_retry_logical_import_error() {
        let root = unique_temp_dir("sqlite-build-no-logical-retry");
        let db = root.join("library.sqlite3");
        let build_tmp = root
            .join("tmpfs-build")
            .join(format!(".library.sqlite3.build.{}", std::process::id()));
        let initial_plan = SqliteBuildTempPlan {
            build_tmp_path: build_tmp.clone(),
            final_tmp_path: sqlite_temp_path(&db),
            source: SqliteBuildTempSource::DefaultTmpfs,
        };
        let mut attempts = 0usize;
        let mut writer = |_path: &Path,
                          _scan: &LibraryScan,
                          _progress: &mut ProgressCallback<'_>|
         -> Result<(), String> {
            attempts += 1;
            Err("insert payload file: UNIQUE constraint failed".to_string())
        };

        let err = save_sqlite_scan_with_progress_using_writer(
            &db,
            &sqlite_scan_with_normal_files(&[]),
            None,
            initial_plan,
            &mut writer,
        )
        .expect_err("logical import error should not retry");

        assert!(
            err.contains("insert payload file"),
            "unexpected error: {err}"
        );
        assert_eq!(attempts, 1);
        assert!(!db.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_save_does_not_retry_explicit_build_dir_failure() {
        let root = unique_temp_dir("sqlite-build-env-no-retry");
        let db = root.join("library.sqlite3");
        let build_tmp = root
            .join("explicit-build")
            .join(format!(".library.sqlite3.build.{}", std::process::id()));
        let initial_plan = SqliteBuildTempPlan {
            build_tmp_path: build_tmp,
            final_tmp_path: sqlite_temp_path(&db),
            source: SqliteBuildTempSource::EnvOverride,
        };
        let mut attempts = 0usize;
        let mut writer = |_path: &Path,
                          _scan: &LibraryScan,
                          _progress: &mut ProgressCallback<'_>|
         -> Result<(), String> {
            attempts += 1;
            Err("database or disk is full".to_string())
        };

        let err = save_sqlite_scan_with_progress_using_writer(
            &db,
            &sqlite_scan_with_normal_files(&[]),
            None,
            initial_plan,
            &mut writer,
        )
        .expect_err("explicit build dir failure should not retry");

        assert!(
            err.contains("database or disk is full"),
            "unexpected error: {err}"
        );
        assert_eq!(attempts, 1);
        assert!(!db.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_inspect_does_not_create_missing_database() {
        let root = unique_temp_dir("sqlite-inspect-missing");
        let db = root.join("library.sqlite3");

        let err = run_sqlite_inspect_cli(&[
            "--path".to_string(),
            db.display().to_string(),
            "SELECT 1".to_string(),
        ])
        .expect_err("missing database should fail before sqlite open");

        assert!(err.starts_with("stat "), "unexpected error: {err}");
        assert!(!db.exists(), "read-only inspect must not create database");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_inspect_rejects_empty_database() {
        let root = unique_temp_dir("sqlite-inspect-empty");
        let db = root.join("library.sqlite3");
        std::fs::write(&db, b"").expect("write empty database");

        let err = run_sqlite_inspect_cli(&[
            "--path".to_string(),
            db.display().to_string(),
            "SELECT 1".to_string(),
        ])
        .expect_err("empty database should fail before sqlite open");

        assert!(
            err.ends_with(" is empty"),
            "unexpected empty database error: {err}"
        );
        assert_eq!(std::fs::metadata(&db).expect("metadata").len(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_inspect_formats_common_cell_types() {
        let root = unique_temp_dir("sqlite-inspect-cell-types");
        let db = root.join("library.sqlite3");
        let conn = Connection::open(&db).expect("open sqlite");
        conn.execute_batch(
            "CREATE TABLE values_fixture(
                int_value INTEGER,
                real_value REAL,
                text_value TEXT,
                blob_value BLOB,
                null_value TEXT
             );
             INSERT INTO values_fixture VALUES(42, 1.5, 'hello', x'010203', NULL);",
        )
        .expect("create inspect fixture");
        drop(conn);

        let out = run_sqlite_inspect_cli(&[
            "--path".to_string(),
            db.display().to_string(),
            "SELECT int_value, real_value, text_value, blob_value, null_value".to_string(),
            "FROM values_fixture".to_string(),
        ])
        .expect("inspect sqlite fixture");

        assert_eq!(
            out,
            "int_value\treal_value\ttext_value\tblob_value\tnull_value\n42\t1.5\thello\t<blob:3>\t\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_arcade_load_returns_launchables_beyond_old_cap() {
        const ROWS: usize = 20_005;
        let root = unique_temp_dir("sqlite-arcade-no-cap");
        let db = root.join("library.sqlite3");
        let discoveries = (0..ROWS)
            .map(|idx| mra_discovery(idx, &format!("Game {idx:05}")))
            .collect::<Vec<_>>();
        save_sqlite_scan(&db, &sqlite_scan_with_discoveries(discoveries))
            .expect("write large arcade database");

        let loaded = load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db)
            .expect("load arcade catalog");

        assert_eq!(loaded.rows, ROWS);
        assert_eq!(loaded.catalog.games.len(), ROWS);
        assert!(loaded
            .catalog
            .games
            .iter()
            .any(|game| game.title.as_ref() == "Game 20004"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_arcade_load_ignores_hot_rollback_journal() {
        let root = unique_temp_dir("sqlite-hot-journal");
        let db = root.join("library.sqlite3");
        save_sqlite_scan(
            &db,
            &sqlite_scan_with_discoveries(vec![mra_discovery(1, "Hot Journal")]),
        )
        .expect("write catalog database");

        let child = std::process::Command::new(std::env::current_exe().expect("current test exe"))
            .arg("--exact")
            .arg("library_db::tests::sqlite_hot_journal_child_abort")
            .env("MISTER_MAGIK_HOT_JOURNAL_DB", &db)
            .output()
            .expect("run hot journal child");
        assert!(
            !child.status.success(),
            "hot journal child should abort, stdout={}, stderr={}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        let journal = PathBuf::from(format!("{}-journal", db.display()));
        assert!(
            journal.exists(),
            "child abort should leave rollback journal at {}",
            journal.display()
        );

        let loaded = load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db)
            .expect("load cached catalog despite hot rollback journal");
        assert_eq!(loaded.catalog.games.len(), 1);
        assert_eq!(loaded.catalog.games[0].title.as_ref(), "Hot Journal");
        assert!(
            sqlite_cached_summary(&db, 0).is_ok(),
            "cached summary reads should also ignore the stale rollback journal"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn old_schema_database_is_not_a_usable_cache() {
        let root = unique_temp_dir("sqlite-old-schema");
        let db = root.join("library.sqlite3");
        save_sqlite_scan(
            &db,
            &sqlite_scan_with_discoveries(vec![mra_discovery(1, "Old Schema")]),
        )
        .expect("write catalog database");
        let conn = Connection::open(&db).expect("open sqlite");
        conn.execute(
            "UPDATE meta SET value=?1 WHERE key='version'",
            [i64::from(SCHEMA_VERSION - 1)],
        )
        .expect("downgrade schema");
        drop(conn);

        let load_err = match load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db) {
            Ok(_) => panic!("old schema should not load as cache"),
            Err(err) => err,
        };
        assert!(
            load_err.contains("catalog schema mismatch"),
            "unexpected load error: {load_err}"
        );
        let summary_err =
            sqlite_cached_summary(&db, 0).expect_err("old schema should not summarize as cache");
        assert!(
            summary_err.contains("catalog schema mismatch"),
            "unexpected summary error: {summary_err}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_hot_journal_child_abort() {
        let Some(path) = std::env::var_os("MISTER_MAGIK_HOT_JOURNAL_DB").map(PathBuf::from) else {
            return;
        };
        let conn = Connection::open(&path).expect("open child sqlite");
        conn.execute_batch(
            "PRAGMA journal_mode=DELETE;
             BEGIN IMMEDIATE;
             UPDATE meta SET value = value + 1 WHERE key = 'normal_files';",
        )
        .expect("create hot rollback journal");
        std::process::abort();
    }

    #[test]
    fn sqlite_save_materializes_launcher_catalog_variants() {
        let root = unique_temp_dir("sqlite-launcher-catalog");
        let db = root.join("library.sqlite3");
        let mame_db = root.join("mame.sqlite3");
        write_mame_fixture_db(&mame_db, &[]);
        let mut world = mra_discovery(1, "Moon Patrol (World)");
        world.launch_ref = "/media/fat/_Arcade/Moon Patrol (World).mra".to_string();
        world.source_path = world.launch_ref.clone();
        world.setname = Some("mpatrol".to_string());
        let mut us = mra_discovery(2, "Moon Patrol (US)");
        us.launch_ref = "/media/fat/_Arcade/Moon Patrol (US).mra".to_string();
        us.source_path = us.launch_ref.clone();
        us.setname = Some("mpatrol".to_string());
        let pack = preview_worker::PreviewArchiveIndex {
            path: root
                .join("320x320-screenshots.mmlz4b")
                .display()
                .to_string(),
            codec: "lz4-block",
            entries: vec!["mpatrol".to_string()],
        };

        write_sqlite_scan_with_mame_and_preview_pack(
            &db,
            &sqlite_scan_with_discoveries(vec![world, us]),
            &mame_db,
            &pack,
        )
        .expect("write sqlite");
        let conn = Connection::open(&db).expect("open sqlite");
        let materialized_rows: i64 = conn
            .query_row("SELECT count(*) FROM launcher_catalog", [], |row| {
                row.get(0)
            })
            .expect("count launcher catalog");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");

        assert_eq!(materialized_rows, 1);
        assert_eq!(loaded.rows, 1);
        assert_eq!(loaded.catalog.games[0].title.as_ref(), "Moon Patrol (US)");
        assert!(loaded.catalog.games[0].has_preview);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn virtual_launch_plan_query_returns_system_scoped_rows() {
        let root = unique_temp_dir("sqlite-virtual-launch-plans");
        let db = root.join("library.sqlite3");
        let saturn = saturn_payload("/media/fat/games/Saturn/Nights.chd");
        let mut snes = payload("/media/fat/games/SNES/F-Zero.sfc");
        snes.platform_id = "snes".to_string();
        snes.core_id = "SNES".to_string();
        snes.hardware_id = "snes".to_string();

        save_sqlite_scan(&db, &sqlite_scan_with_discoveries(vec![saturn, snes]))
            .expect("write sqlite");
        let conn = Connection::open(&db).expect("open sqlite");
        let plans = load_virtual_launch_plans_for_system_from_conn(&conn, "saturn", 8)
            .expect("load virtual launch plans");

        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].system_id, "saturn");
        assert_eq!(plans[0].core_path, "_Console/Saturn");
        assert_eq!(plans[0].payload_path, "/media/fat/games/Saturn/Nights.chd");
        let _ = std::fs::remove_dir_all(root);
    }
}
