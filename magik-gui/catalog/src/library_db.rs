//! Whole-MiSTer library database scanning and loading.
//!
//! This is deliberately TOC/header-only for archives. Indexing must never
//! decompress full game libraries just to make the launcher searchable.

use crate::arcade_catalog::{self, ArcadeCatalog, ArcadeGameEntry};
use crate::catalog_config;
use crate::catalog_scan::{self, DiscoveryEvent};
use crate::game_discovery::{
    catalog_system_id_for_discovery, confidence_str, covered_payload_paths,
    discovery_from_profile_archive_entry, discovery_from_profile_file, is_launcher_launch_ref,
    launch_kind_for_discovery, launch_ref_for_discovery, preferred_playable_discoveries_by_key,
    profile_id_for_discovery, system_title_for_discovery, unique_discovery_count,
    variant_score_from_haystack, DiscoverySourceKind, GameDiscovery,
};
use crate::media_metadata;
#[cfg(test)]
use crate::catalog_scan::{
    classify_profile_path, profile_for_path, scan_zip_central_directory, FoundFile,
};
#[cfg(test)]
use crate::media_metadata::{
    collection_discoveries_from_container, collection_discoveries_from_listing_text,
    collection_listing_text_with_tool, infer_region_metadata, parse_saturn_boot_header,
    read_mgl_metadata, read_mra_metadata, RegionInference,
};
#[cfg(test)]
use crate::game_discovery::{
    first_disc_number_from_haystack, is_playable_discovery, DiscoveryConfidence,
};
#[cfg(test)]
use crate::software_identity::{
    crc32, match_software_by_file_hash_with_cache,
    mame_software_identity_for_discovery_with_hash_matcher, preview_asset_pack_platform,
    rom_hash_candidates, software_asset_key, MameSoftwareItemMetadata, MameSoftwareMetadata,
};
pub use crate::catalog_config::{
    default_hbmame_sqlite_path, default_mame_sqlite_path, default_sqlite_path,
};
use crate::catalog_config::{DEFAULT_SQLITE_BUILD_DIR, DEFAULT_SQLITE_PATH, SCHEMA_VERSION};
use crate::catalog_stamp;
use crate::catalog_store;
use crate::launch_profiles::{
    self, CollectionListing, MountKind, PayloadDisposition, PayloadRule, ProfilePathClass,
    RuleSourceKind,
};
use crate::preview_worker;
use crate::software_identity::{
    console_preview_asset, load_arcade_machine_metadata, load_mame_software_metadata,
    mame_identity_for_discovery,
    mame_identity_projection, mame_software_identity_for_discovery, write_simple_mame_metadata_db,
    ArcadeMachineMetadata, MachineMetadataRows, PreviewArchivePaths, SoftwareHashCache,
};
use rusqlite::{params, Connection, OpenFlags};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

pub(crate) const MRA_PREFIX_BYTES: usize = 160 * 1024;
type ProgressCallback<'a> = Option<&'a mut dyn FnMut(&str, &str)>;

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
struct LibraryScan {
    version: u32,
    scanned_at_unix: i64,
    normal_files: Vec<LibraryPayloadFile>,
    containers: Vec<LibraryContainer>,
    entries: Vec<LibraryContainerEntry>,
    ignored_files: usize,
    discoveries: Vec<GameDiscovery>,
    discover_us: u64,
    classify_us: u64,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SqliteBuildTempSource {
    EnvOverride,
    DefaultTmpfs,
    BesideFinal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SqliteBuildTempPlan {
    build_tmp_path: PathBuf,
    final_tmp_path: PathBuf,
    source: SqliteBuildTempSource,
}


#[derive(Clone, Debug)]
struct LibraryPayloadFile {
    path: String,
    profile_id: String,
    pub(crate) size: u64,
    pub(crate) mtime_secs: i64,
    rule: PayloadRule,
}

pub fn run_scan_bench() {
    let cfg = BenchConfig::from_env();
    let label =
        std::env::var("MISTER_LIBRARY_BENCH_LABEL").unwrap_or_else(|_| "LIB-BENCH".to_string());
    let iterations = std::env::var("MISTER_LIBRARY_BENCH_ITERATIONS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let bench_force_rebuild = env_bool("MISTER_LIBRARY_BENCH_FORCE_REBUILD");
    let bench_precount = env_bool("MISTER_LIBRARY_BENCH_PRECOUNT");
    println!("library-scan-bench label={label}");
    println!("library-scan-bench roots={}", cfg.roots.join("|"));
    println!(
        "library-scan-bench sqlite_path={}",
        cfg.sqlite_path.display()
    );
    for iteration in 1..=iterations {
        match std::fs::remove_file(&cfg.sqlite_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("library-scan-bench remove old sqlite: {e}"),
        }

        if bench_precount {
            let (candidates, dirs, precount_us) = catalog_scan::precount_discovery_candidates(&cfg.roots);
            println!(
                "library_scan_bench_tsv\t{label}\t{iteration}\tprecount_discovery\t{precount_us}\tcandidates={candidates}\tdirs={dirs}"
            );
        }

        let build_t = Instant::now();
        let artifact = scan_library_artifact(&cfg, None);
        let stats = artifact.stats().clone();
        let build_us = build_t.elapsed().as_micros() as u64;

        let import_t = Instant::now();
        let summary = match save_scan_artifact_to_sqlite(&cfg, artifact, None) {
            Ok(summary) => summary,
            Err(e) => {
                println!(
                    "library_scan_bench_tsv\t{label}\t{iteration}\timport_error\t{}\t{e}",
                    import_t.elapsed().as_micros()
                );
                continue;
            }
        };
        let import_us = import_t.elapsed().as_micros() as u64;
        let bytes = summary.bytes;

        let load_t = Instant::now();
        let loaded = load_arcade_catalog_from_sqlite("/media/fat/_Arcade");
        let (load_us, arcade_rows) = match loaded {
            Ok(load) => (load.us, load.rows),
            Err(e) => {
                eprintln!("library-scan-bench arcade load failed: {e}");
                (load_t.elapsed().as_micros() as u64, 0)
            }
        };

        let stamp_t = Instant::now();
        let stamp_check = sqlite_catalog_stamp_check(&cfg);
        let stamp_us = stamp_t.elapsed().as_micros() as u64;

        let force_rebuild = if bench_force_rebuild {
            let change_dir = Path::new(&cfg.roots[0]).join("games/NES");
            let change_parent = if change_dir.is_dir() {
                change_dir
            } else {
                PathBuf::from(&cfg.roots[0])
            };
            let change_path =
                change_parent.join(format!("Mister_Magik_Refresh_Bench_{iteration}.nes"));
            if let Err(e) = std::fs::write(&change_path, b"[mister]\nrbf=menu\n") {
                eprintln!(
                    "library-scan-bench force rebuild setup failed at {}: {e}",
                    change_path.display()
                );
            }
            let force_rebuild_t = Instant::now();
            let summary = rebuild_sqlite_database(&cfg, None);
            Some((force_rebuild_t.elapsed().as_micros() as u64, summary))
        } else {
            None
        };

        println!(
            "library_scan_bench_tsv\t{label}\t{iteration}\tfresh_build\t{build_us}\tdiscover_us={}\tclassify_us={}\tnormal_files={}\tcontainers={}\tentries={}\tdiscoveries={}",
            stats.discover_us,
            stats.classify_us,
            stats.normal_files,
            stats.containers,
            stats.entries,
            stats.discoveries
        );
        println!(
            "library_scan_bench_tsv\t{label}\t{iteration}\timport\t{import_us}\tbytes={bytes}"
        );
        println!(
            "library_scan_bench_tsv\t{label}\t{iteration}\tcached_arcade_load\t{load_us}\trows={arcade_rows}"
        );
        match stamp_check {
            Ok(check) => println!(
                "library_scan_bench_tsv\t{label}\t{iteration}\troot_stamp_check\t{stamp_us}\tunchanged={} check_us={} compute_us={} open_us={} read_us={} compare_us={} stored={} current={} stored_lines={} current_lines={}",
                check.unchanged,
                check.check_us,
                check.compute_us,
                check.open_us,
                check.read_us,
                check.compare_us,
                check.stored_fingerprint.as_deref().unwrap_or("missing"),
                check.current_fingerprint,
                check.stored_lines,
                check.current_lines
            ),
            Err(e) => println!(
                "library_scan_bench_tsv\t{label}\t{iteration}\troot_stamp_check_error\t{stamp_us}\t{e}"
            ),
        }
        if let Some((force_rebuild_us, force_rebuild_summary)) = force_rebuild {
            match force_rebuild_summary {
                Ok(summary) => println!(
                    "library_scan_bench_tsv\t{label}\t{iteration}\tforce_rebuild\t{force_rebuild_us}\tscan_us={}\tdiscover_us={}\tclassify_us={}\timport_us={}\tskipped={}\tdiscoveries={}",
                    summary.scan_us,
                    summary.discover_us,
                    summary.classify_us,
                    summary.import_us,
                    summary.skipped,
                    summary.discoveries
                ),
                Err(e) => println!(
                    "library_scan_bench_tsv\t{label}\t{iteration}\tforce_rebuild_error\t{force_rebuild_us}\t{e}"
                ),
            }
        }
    }
}

pub fn run_sqlite_inspect_cli(args: &[String]) -> Result<String, String> {
    let mut path = default_sqlite_path();
    let mut query_parts = Vec::new();
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--path" => {
                let Some(value) = args.get(i + 1) else {
                    return Err("library-sql: --path needs a value".into());
                };
                path = PathBuf::from(value);
                i += 2;
            }
            other => {
                query_parts.push(other.to_string());
                i += 1;
            }
        }
    }
    if query_parts.is_empty() {
        return Err("usage: library-sql [--path PATH] SELECT ...".into());
    }
    let query = query_parts.join(" ");
    let trimmed = query.trim_start().to_ascii_lowercase();
    if !trimmed.starts_with("select") && !trimmed.starts_with("with") {
        return Err("library-sql only allows read-only SELECT/WITH queries".into());
    }

    let metadata = std::fs::metadata(&path).map_err(|e| format!("stat {}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    if metadata.len() == 0 {
        return Err(format!("{} is empty", path.display()));
    }

    let conn = open_sqlite_read_only(&path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    let mut stmt = conn
        .prepare(&query)
        .map_err(|e| format!("prepare query: {e}"))?;
    let column_count = stmt.column_count();
    let mut out = String::new();
    if column_count > 0 {
        out.push_str(&stmt.column_names().join("\t"));
        out.push('\n');
    }
    let mut rows = stmt.query([]).map_err(|e| format!("run query: {e}"))?;
    while let Some(row) = rows.next().map_err(|e| format!("read row: {e}"))? {
        for col in 0..column_count {
            if col > 0 {
                out.push('\t');
            }
            out.push_str(&sqlite_cell_to_string(row, col)?);
        }
        out.push('\n');
    }
    Ok(out)
}

fn sqlite_cell_to_string(row: &rusqlite::Row<'_>, col: usize) -> Result<String, String> {
    use rusqlite::types::ValueRef;

    match row
        .get_ref(col)
        .map_err(|e| format!("read column {col}: {e}"))?
    {
        ValueRef::Null => Ok(String::new()),
        ValueRef::Integer(value) => Ok(value.to_string()),
        ValueRef::Real(value) => Ok(value.to_string()),
        ValueRef::Text(value) => Ok(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Blob(value) => Ok(format!("<blob:{}>", value.len())),
    }
}

pub fn remove_default_sqlite_database() -> Result<(), String> {
    let path = default_sqlite_path();
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("failed to delete {}: {e}", path.display())),
    }
    Ok(())
}

pub fn load_virtual_launch_plan(launch_ref: &str) -> Result<Option<VirtualLaunchPlan>, String> {
    let path = default_sqlite_path();
    let conn = open_sqlite_read_only(&path).map_err(|e| format!("open library db: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    ensure_sqlite_schema_current(&conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT launch_plans.launch_ref,
                    games.title,
                    games.system_id,
                    COALESCE(profiles.core_path, launch_plans.core_id),
                    COALESCE(launch_plans.payload_path, ''),
                    COALESCE(payloads.mount_kind, 'mount-image'),
                    COALESCE(payloads.mount_index, 0),
                    COALESCE(payloads.mount_delay_secs, 1)
             FROM launch_plans
             JOIN games ON games.game_id = launch_plans.game_id
             LEFT JOIN profiles ON profiles.profile_id = launch_plans.profile_id
             LEFT JOIN payloads
                    ON payloads.launch_ref = launch_plans.payload_path
                   AND payloads.profile_id = launch_plans.profile_id
             WHERE launch_plans.launch_ref = ?1
               AND launch_plans.launch_kind = 'virtual-mgl'",
        )
        .map_err(|e| format!("prepare virtual launch query: {e}"))?;
    let mut rows = stmt
        .query([launch_ref])
        .map_err(|e| format!("query virtual launch: {e}"))?;
    let Some(row) = rows
        .next()
        .map_err(|e| format!("read virtual launch: {e}"))?
    else {
        return Ok(None);
    };
    Ok(Some(VirtualLaunchPlan {
        launch_ref: row
            .get::<_, String>(0)
            .map_err(|e| format!("read launch_ref: {e}"))?,
        title: row
            .get::<_, String>(1)
            .map_err(|e| format!("read title: {e}"))?,
        system_id: row
            .get::<_, String>(2)
            .map_err(|e| format!("read system_id: {e}"))?,
        core_path: row
            .get::<_, String>(3)
            .map_err(|e| format!("read core_path: {e}"))?,
        payload_path: row
            .get::<_, String>(4)
            .map_err(|e| format!("read payload_path: {e}"))?,
        mount_kind: row
            .get::<_, String>(5)
            .map_err(|e| format!("read mount_kind: {e}"))?,
        mount_index: row
            .get::<_, i64>(6)
            .map_err(|e| format!("read mount_index: {e}"))?
            .clamp(0, u8::MAX as i64) as u8,
        mount_delay_secs: row
            .get::<_, i64>(7)
            .map_err(|e| format!("read mount_delay_secs: {e}"))?
            .clamp(0, u8::MAX as i64) as u8,
    }))
}

pub fn load_virtual_launch_plans_for_system(
    system_id: &str,
    limit: usize,
) -> Result<Vec<VirtualLaunchPlan>, String> {
    let path = default_sqlite_path();
    let conn = open_sqlite_read_only(&path).map_err(|e| format!("open library db: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    ensure_sqlite_schema_current(&conn)?;
    load_virtual_launch_plans_for_system_from_conn(&conn, system_id, limit)
}

pub fn load_virtual_launch_plans() -> Result<Vec<VirtualLaunchPlan>, String> {
    let path = default_sqlite_path();
    let conn = open_sqlite_read_only(&path).map_err(|e| format!("open library db: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    ensure_sqlite_schema_current(&conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT launch_plans.launch_ref,
                    games.title,
                    games.system_id,
                    COALESCE(profiles.core_path, launch_plans.core_id),
                    COALESCE(launch_plans.payload_path, ''),
                    COALESCE(payloads.mount_kind, 'mount-image'),
                    COALESCE(payloads.mount_index, 0),
                    COALESCE(payloads.mount_delay_secs, 1)
             FROM launch_plans
             JOIN games ON games.game_id = launch_plans.game_id
             LEFT JOIN profiles ON profiles.profile_id = launch_plans.profile_id
             LEFT JOIN payloads
                    ON payloads.launch_ref = launch_plans.payload_path
                   AND payloads.profile_id = launch_plans.profile_id
             WHERE launch_plans.launch_kind = 'virtual-mgl'
             ORDER BY games.system_id, games.sort_title, launch_plans.launch_ref",
        )
        .map_err(|e| format!("prepare virtual launch list query: {e}"))?;
    let rows = stmt
        .query_map([], virtual_launch_plan_from_row)
        .map_err(|e| format!("query virtual launch list: {e}"))?;
    rows.map(|row| row.map_err(|e| format!("read virtual launch row: {e}")))
        .collect()
}

fn load_virtual_launch_plans_for_system_from_conn(
    conn: &Connection,
    system_id: &str,
    limit: usize,
) -> Result<Vec<VirtualLaunchPlan>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT launch_plans.launch_ref,
                    games.title,
                    games.system_id,
                    COALESCE(profiles.core_path, launch_plans.core_id),
                    COALESCE(launch_plans.payload_path, ''),
                    COALESCE(payloads.mount_kind, 'mount-image'),
                    COALESCE(payloads.mount_index, 0),
                    COALESCE(payloads.mount_delay_secs, 1)
             FROM launch_plans
             JOIN games ON games.game_id = launch_plans.game_id
             LEFT JOIN profiles ON profiles.profile_id = launch_plans.profile_id
             LEFT JOIN payloads
                    ON payloads.launch_ref = launch_plans.payload_path
                   AND payloads.profile_id = launch_plans.profile_id
             WHERE launch_plans.launch_kind = 'virtual-mgl'
               AND games.system_id = ?1
             ORDER BY games.sort_title, launch_plans.launch_ref
             LIMIT ?2",
        )
        .map_err(|e| format!("prepare virtual launch list query: {e}"))?;
    let rows = stmt
        .query_map(
            params![system_id, limit as i64],
            virtual_launch_plan_from_row,
        )
        .map_err(|e| format!("query virtual launch list: {e}"))?;
    rows.map(|row| row.map_err(|e| format!("read virtual launch row: {e}")))
        .collect()
}

pub fn load_amigavision_launch_refs(limit: usize) -> Result<Vec<String>, String> {
    let path = default_sqlite_path();
    let conn = open_sqlite_read_only(&path).map_err(|e| format!("open library db: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    ensure_sqlite_schema_current(&conn)?;
    let mut stmt = conn
        .prepare(
            "SELECT launch_ref
             FROM launchables
             WHERE launch_ref LIKE 'magik-amigavision:%'
             ORDER BY title, launch_ref
             LIMIT ?1",
        )
        .map_err(|e| format!("prepare AmigaVision launch query: {e}"))?;
    let rows = stmt
        .query_map([limit as i64], |row| row.get::<_, String>(0))
        .map_err(|e| format!("query AmigaVision launches: {e}"))?;
    rows.map(|row| row.map_err(|e| format!("read AmigaVision launch_ref: {e}")))
        .collect()
}

fn virtual_launch_plan_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<VirtualLaunchPlan> {
    Ok(VirtualLaunchPlan {
        launch_ref: row.get(0)?,
        title: row.get(1)?,
        system_id: row.get(2)?,
        core_path: row.get(3)?,
        payload_path: row.get(4)?,
        mount_kind: row.get(5)?,
        mount_index: row.get::<_, i64>(6)?.clamp(0, u8::MAX as i64) as u8,
        mount_delay_secs: row.get::<_, i64>(7)?.clamp(0, u8::MAX as i64) as u8,
    })
}

pub fn load_arcade_catalog_from_sqlite(
    root: impl AsRef<Path>,
) -> Result<LibraryCatalogLoad, String> {
    let path = default_sqlite_path();
    load_arcade_catalog_from_sqlite_at(root, &path)
}

fn load_arcade_catalog_from_sqlite_at(
    root: impl AsRef<Path>,
    path: &Path,
) -> Result<LibraryCatalogLoad, String> {
    let root = root.as_ref().to_path_buf();
    let t = Instant::now();
    let open_t = Instant::now();
    let conn = open_sqlite_read_only(path).map_err(|e| format!("open library db: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    ensure_sqlite_schema_current(&conn)?;
    let open_us = open_t.elapsed().as_micros() as u64;
    let query_t = Instant::now();
    let games = match load_materialized_ui_catalog(&conn) {
        Ok(Some(games)) => games,
        Ok(None) => match load_materialized_launcher_catalog(&conn) {
            Ok(Some(games)) => games,
            Ok(None) => load_joined_launcher_catalog(&conn)?,
            Err(e) => return Err(e),
        },
        Err(e) => return Err(e),
    };
    let query_us = query_t.elapsed().as_micros() as u64;
    let rows = games.len();
    let systems_t = Instant::now();
    let systems = arcade_catalog::systems_from_games(&games);
    let systems_us = systems_t.elapsed().as_micros() as u64;
    let catalog_t = Instant::now();
    let catalog = ArcadeCatalog::new(root, games, systems);
    let catalog_us = catalog_t.elapsed().as_micros() as u64;
    Ok(LibraryCatalogLoad {
        catalog,
        us: t.elapsed().as_micros() as u64,
        open_us,
        query_us,
        systems_us,
        catalog_us,
        rows,
    })
}

fn load_materialized_ui_catalog(conn: &Connection) -> Result<Option<Vec<ArcadeGameEntry>>, String> {
    if !sqlite_table_exists(conn, "ui_arcade_preferred")? {
        return Ok(None);
    }
    let mut games = query_game_entries(
        conn,
        "SELECT title,
                launch_ref,
                preview_archive_path,
                preview_asset_key,
                has_preview,
                system_id
         FROM ui_arcade_preferred
         ORDER BY ordinal",
        "ui arcade preferred",
    )?;
    if sqlite_table_exists(conn, "launcher_catalog")? {
        games.extend(query_game_entries(
            conn,
            "SELECT title,
                    launch_ref,
                    preview_archive_path,
                    preview_asset_key,
                    has_preview,
                    system_id
             FROM launcher_catalog
             WHERE system_id NOT IN ('arcade','neogeo')
             ORDER BY ordinal",
            "launcher catalog extras",
        )?);
    }
    Ok(Some(games))
}

fn load_materialized_launcher_catalog(
    conn: &Connection,
) -> Result<Option<Vec<ArcadeGameEntry>>, String> {
    if !sqlite_table_exists(conn, "launcher_catalog")? {
        return Ok(None);
    }
    Ok(Some(query_game_entries(
        conn,
        "SELECT title,
                launch_ref,
                preview_archive_path,
                preview_asset_key,
                has_preview,
                system_id
         FROM launcher_catalog
         ORDER BY ordinal",
        "launcher catalog",
    )?))
}

fn query_game_entries(
    conn: &Connection,
    sql: &str,
    label: &str,
) -> Result<Vec<ArcadeGameEntry>, String> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("prepare {label} query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ArcadeGameEntry {
                title: row.get::<_, String>(0)?.into(),
                mra_path: row.get::<_, String>(1)?.into(),
                preview_archive_path: row.get::<_, String>(2)?.into(),
                preview_asset_key: row.get::<_, String>(3)?.into(),
                has_preview: row.get::<_, i64>(4)? != 0,
                system_id: row.get::<_, String>(5)?.into(),
            })
        })
        .map_err(|e| format!("query {label}: {e}"))?;
    let mut games = Vec::new();
    for row in rows {
        games.push(row.map_err(|e| format!("read {label} row: {e}"))?);
    }
    Ok(games)
}

pub(crate) fn sqlite_table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|exists| exists != 0)
    .map_err(|e| format!("check sqlite table {table}: {e}"))
}

pub(crate) fn open_sqlite_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let uri = format!("file:{}?mode=ro&immutable=1", sqlite_uri_path(path));
    Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
}

fn sqlite_uri_path(path: &Path) -> String {
    path.to_string_lossy()
        .bytes()
        .flat_map(|byte| match byte {
            b'%' => "%25".bytes().collect::<Vec<_>>(),
            b'?' => "%3F".bytes().collect(),
            b'#' => "%23".bytes().collect(),
            b' ' => "%20".bytes().collect(),
            other => vec![other],
        })
        .map(char::from)
        .collect()
}

fn load_joined_launcher_catalog(conn: &Connection) -> Result<Vec<ArcadeGameEntry>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT games.title,
                    launch_plans.launch_ref,
                    '',
                    '',
                    0,
                    COALESCE(games.system_id,'unknown'),
                    launch_plans.launch_kind,
                    COALESCE(launch_plans.setname,''),
                    COALESCE(launch_plans.parent,'')
             FROM games
             JOIN launch_plans ON launch_plans.game_id = games.game_id
             WHERE launch_plans.launch_ref != ''
               AND launch_plans.launch_kind IN ('mra','mgl','virtual-mgl','catalog-entry')
               AND (
                 lower(launch_plans.launch_ref) LIKE '%.mra'
                 OR lower(launch_plans.launch_ref) LIKE '%.mgl'
                 OR launch_plans.launch_kind='virtual-mgl'
                 OR launch_plans.launch_kind='catalog-entry'
               )
             ORDER BY lower(games.title)",
        )
        .map_err(|e| format!("prepare arcade catalog query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CatalogRow {
                game: ArcadeGameEntry {
                    title: row.get::<_, String>(0)?.into(),
                    mra_path: row.get::<_, String>(1)?.into(),
                    preview_archive_path: row.get::<_, String>(2)?.into(),
                    preview_asset_key: row.get::<_, String>(3)?.into(),
                    has_preview: row.get::<_, i64>(4)? != 0,
                    system_id: row.get::<_, String>(5)?.into(),
                },
                source_kind: row.get::<_, String>(6)?,
                setname: row.get::<_, String>(7)?,
                parent: row.get::<_, String>(8)?,
                family_key: None,
            })
        })
        .map_err(|e| format!("query arcade catalog: {e}"))?;
    let mut rows_out = Vec::new();
    for row in rows {
        rows_out.push(row.map_err(|e| format!("read arcade catalog row: {e}"))?);
    }
    rows_out.retain(|row| is_launcher_launch_ref(&row.game.mra_path));
    Ok(collapse_catalog_variants(rows_out))
}

#[derive(Clone, Debug)]
struct CatalogRow {
    game: ArcadeGameEntry,
    source_kind: String,
    setname: String,
    parent: String,
    family_key: Option<String>,
}

fn collapse_catalog_variants(rows: Vec<CatalogRow>) -> Vec<ArcadeGameEntry> {
    let mut best_idx: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<CatalogRow> = Vec::with_capacity(rows.len());

    for row in rows {
        let key = catalog_variant_group_key(&row);
        if let Some(&idx) = best_idx.get(&key) {
            if prefer_catalog_variant(&row, &out[idx]) {
                out[idx] = row;
            }
        } else {
            best_idx.insert(key, out.len());
            out.push(row);
        }
    }

    out.into_iter().map(|row| row.game).collect()
}

fn catalog_variant_group_key(row: &CatalogRow) -> String {
    if let Some(family_key) = row.family_key.as_deref() {
        return format!("family:{}", normalize_id(family_key));
    }
    if row.source_kind == "mra" {
        if !row.setname.trim().is_empty() {
            let parent = row.parent.trim();
            let group = if parent.is_empty() {
                row.setname.as_str()
            } else {
                parent
            };
            return format!("mra:set:{}", normalize_id(group));
        }
        return format!("mra:title:{}", canonical_variant_title(&row.game.title));
    }
    if row.source_kind == "catalog-entry" {
        return format!(
            "catalog-entry:{}:{}",
            row.game.mra_path,
            normalize_id(&row.game.title)
        );
    }
    format!("{}:{}", row.source_kind, row.game.mra_path)
}

fn prefer_catalog_variant(a: &CatalogRow, b: &CatalogRow) -> bool {
    let a_score = catalog_variant_score(a);
    let b_score = catalog_variant_score(b);
    if a_score != b_score {
        return a_score > b_score;
    }
    if a.game.has_preview != b.game.has_preview {
        return a.game.has_preview;
    }
    a.game.mra_path < b.game.mra_path
}

fn catalog_variant_score(row: &CatalogRow) -> i32 {
    let haystack = format!(
        "{} {} {} {}",
        row.game.title, row.game.mra_path, row.setname, row.parent
    )
    .to_ascii_lowercase();

    let mut score = variant_score_from_haystack(&haystack);
    if row.source_kind == "mra" && !row.setname.trim().is_empty() && row.parent.trim().is_empty() {
        score += 1000;
    }
    score
}


pub(crate) fn canonical_variant_title(title: &str) -> String {
    let mut out = String::new();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    for ch in title.chars() {
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ if paren_depth == 0 && bracket_depth == 0 => out.push(ch),
            _ => {}
        }
    }
    normalize_id(out.trim_matches(|ch: char| ch.is_whitespace() || ch == '-' || ch == ','))
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

fn sqlite_catalog_stamp_check(cfg: &BenchConfig) -> Result<CatalogStampCheckSummary, String> {
    let started = Instant::now();
    let open_t = Instant::now();
    let conn = open_sqlite_read_only(&cfg.sqlite_path)
        .map_err(|e| format!("open catalog stamp db {}: {e}", cfg.sqlite_path.display()))?;
    let open_us = open_t.elapsed().as_micros() as u64;
    let read_t = Instant::now();
    let stored = catalog_store::read_catalog_stamp(&conn)?;
    let read_us = read_t.elapsed().as_micros() as u64;
    let compute_t = Instant::now();
    let current = catalog_stamp::compute_default_catalog_stamp(&cfg.roots);
    let current_fingerprint = current.fingerprint_hex();
    let current_lines = current.lines().len();
    let compute_us = compute_t.elapsed().as_micros() as u64;
    let compare_t = Instant::now();
    let (stored_fingerprint, stored_lines, unchanged) = match stored {
        Some(stored) => {
            let stored_fingerprint = stored.fingerprint_hex();
            let stored_lines = stored.lines().len();
            let unchanged = stored == current;
            (Some(stored_fingerprint), stored_lines, unchanged)
        }
        None => (None, 0, false),
    };
    let compare_us = compare_t.elapsed().as_micros() as u64;
    Ok(CatalogStampCheckSummary {
        unchanged,
        check_us: started.elapsed().as_micros() as u64,
        compute_us,
        open_us,
        read_us,
        compare_us,
        stored_fingerprint,
        current_fingerprint,
        stored_lines,
        current_lines,
    })
}

fn rebuild_sqlite_database(
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

struct BenchConfig {
    roots: Vec<String>,
    sqlite_path: PathBuf,
}

impl BenchConfig {
    fn from_env() -> Self {
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

fn scan_library(cfg: &BenchConfig) -> LibraryScan {
    scan_library_with_progress(cfg, None)
}

fn scan_library_artifact(
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

fn save_scan_artifact_to_sqlite(
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
                let installed = media_metadata::installed_amigavision_discoveries_from_hdf(&f, profile);
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
                discoveries.extend(media_metadata::collection_discoveries_from_container(&f, profile, &rule));
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


#[cfg(test)]
fn save_sqlite_scan(path: &Path, scan: &LibraryScan) -> Result<u64, String> {
    save_sqlite_scan_with_progress(path, scan, None)
}

#[cfg(test)]
fn save_sqlite_scan_with_progress(
    path: &Path,
    scan: &LibraryScan,
    progress: ProgressCallback<'_>,
) -> Result<u64, String> {
    save_sqlite_scan_with_progress_and_stamp(path, scan, None, progress)
}

fn save_sqlite_scan_with_progress_and_stamp(
    path: &Path,
    scan: &LibraryScan,
    stamp: Option<&catalog_stamp::CatalogStamp>,
    progress: ProgressCallback<'_>,
) -> Result<u64, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create sqlite dir: {e}"))?;
    }

    let mut writer =
        |build_path: &Path, scan: &LibraryScan, progress: &mut ProgressCallback<'_>| {
            let software_hash_cache = SoftwareHashCache::load(path);
            write_sqlite_scan(
                build_path,
                scan,
                reborrow_progress(progress),
                software_hash_cache,
                stamp,
            )
        };
    save_sqlite_scan_with_progress_using_writer(
        path,
        scan,
        progress,
        sqlite_build_temp_plan(path),
        &mut writer,
    )
}

fn save_sqlite_scan_with_progress_using_writer(
    path: &Path,
    scan: &LibraryScan,
    progress: ProgressCallback<'_>,
    initial_plan: SqliteBuildTempPlan,
    writer: &mut dyn FnMut(&Path, &LibraryScan, &mut ProgressCallback<'_>) -> Result<(), String>,
) -> Result<u64, String> {
    let mut progress = progress;
    let first =
        save_sqlite_scan_attempt_with_writer(path, scan, &mut progress, &initial_plan, writer);
    match first {
        Ok(bytes) => Ok(bytes),
        Err(e)
            if initial_plan.source == SqliteBuildTempSource::DefaultTmpfs
                && sqlite_build_error_should_retry_beside_final(&e) =>
        {
            eprintln!(
                "library sqlite build temp failed at {}; retrying beside final DB: {e}",
                initial_plan.build_tmp_path.display()
            );
            let fallback_plan = sqlite_build_temp_plan_beside_final(path);
            save_sqlite_scan_attempt_with_writer(path, scan, &mut progress, &fallback_plan, writer)
        }
        Err(e) => Err(e),
    }
}

fn save_sqlite_scan_attempt_with_writer(
    path: &Path,
    scan: &LibraryScan,
    progress: &mut ProgressCallback<'_>,
    plan: &SqliteBuildTempPlan,
    writer: &mut dyn FnMut(&Path, &LibraryScan, &mut ProgressCallback<'_>) -> Result<(), String>,
) -> Result<u64, String> {
    if let Some(parent) = plan.build_tmp_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create sqlite build dir {}: {e}", parent.display()))?;
    }
    if let Some(parent) = plan.final_tmp_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create sqlite final temp dir {}: {e}", parent.display()))?;
    }
    for tmp_path in [&plan.build_tmp_path, &plan.final_tmp_path] {
        match std::fs::remove_file(tmp_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("remove stale sqlite temp: {e}")),
        }
    }

    if let Err(e) = writer(&plan.build_tmp_path, scan, progress) {
        let _ = std::fs::remove_file(&plan.build_tmp_path);
        return Err(e);
    }
    sync_file_best_effort(&plan.build_tmp_path, "sqlite build temp")?;
    if plan.build_tmp_path != plan.final_tmp_path {
        std::fs::copy(&plan.build_tmp_path, &plan.final_tmp_path)
            .map_err(|e| format!("copy sqlite temp into final dir: {e}"))?;
        let _ = std::fs::remove_file(&plan.build_tmp_path);
    }
    sync_file_best_effort(&plan.final_tmp_path, "sqlite temp")?;
    std::fs::rename(&plan.final_tmp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&plan.final_tmp_path);
        let _ = std::fs::remove_file(&plan.build_tmp_path);
        format!("replace sqlite: {e}")
    })?;
    sync_parent_dir(path);
    std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("stat sqlite: {e}"))
}

fn reborrow_progress<'a>(progress: &'a mut ProgressCallback<'_>) -> ProgressCallback<'a> {
    progress
        .as_mut()
        .map(|callback| &mut **callback as &mut dyn FnMut(&str, &str))
}

fn sqlite_build_error_should_retry_beside_final(error: &str) -> bool {
    [
        "database or disk is full",
        "disk I/O error",
        "No space left on device",
        "Read-only file system",
        "Permission denied",
        "Input/output error",
        "Not a directory",
        "No such file or directory",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

fn sqlite_build_temp_plan(path: &Path) -> SqliteBuildTempPlan {
    sqlite_build_temp_plan_for(
        path,
        std::env::var_os("MISTER_LIBRARY_SQLITE_BUILD_DIR")
            .map(PathBuf::from)
            .as_deref(),
    )
}

fn sqlite_build_temp_plan_for(
    path: &Path,
    build_dir_override: Option<&Path>,
) -> SqliteBuildTempPlan {
    if let Some(build_dir) = build_dir_override {
        return SqliteBuildTempPlan {
            build_tmp_path: sqlite_build_temp_path_in_dir(path, build_dir),
            final_tmp_path: sqlite_temp_path(path),
            source: SqliteBuildTempSource::EnvOverride,
        };
    }
    if is_media_fat_path(path) {
        return SqliteBuildTempPlan {
            build_tmp_path: sqlite_build_temp_path_in_dir(
                path,
                Path::new(DEFAULT_SQLITE_BUILD_DIR),
            ),
            final_tmp_path: sqlite_temp_path(path),
            source: SqliteBuildTempSource::DefaultTmpfs,
        };
    }
    sqlite_build_temp_plan_beside_final(path)
}

fn sqlite_build_temp_plan_beside_final(path: &Path) -> SqliteBuildTempPlan {
    let final_tmp_path = sqlite_temp_path(path);
    SqliteBuildTempPlan {
        build_tmp_path: final_tmp_path.clone(),
        final_tmp_path,
        source: SqliteBuildTempSource::BesideFinal,
    }
}

fn sqlite_build_temp_path_in_dir(path: &Path, build_dir: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("library.sqlite3");
    build_dir.join(format!(".{name}.build.{}", std::process::id()))
}

fn is_media_fat_path(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(std::path::Component::RootDir))
        && matches!(
            components.next(),
            Some(std::path::Component::Normal(component)) if component == "media"
        )
        && matches!(
            components.next(),
            Some(std::path::Component::Normal(component)) if component == "fat"
        )
}

pub(crate) fn sqlite_temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("library.sqlite3");
    path.with_file_name(format!(".{name}.tmp.{}", std::process::id()))
}

pub(crate) fn sync_parent_dir(path: &Path) {
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
}

fn sync_file_best_effort(path: &Path, label: &str) -> Result<(), String> {
    match File::open(path).and_then(|f| f.sync_all()) {
        Ok(()) => Ok(()),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
            ) =>
        {
            Ok(())
        }
        Err(e) => Err(format!("sync {label}: {e}")),
    }
}

pub(crate) fn file_signature(path: &Path) -> FileSignature {
    std::fs::metadata(path)
        .map(|metadata| FileSignature {
            size: metadata.len(),
            mtime_secs: mtime_secs(&metadata),
        })
        .unwrap_or_default()
}


fn materialize_arcade_ui_projections(
    tx: &rusqlite::Transaction<'_>,
    arcade_preview_archive_path: &str,
    neogeo_preview_archive_path: &str,
) -> Result<(), String> {
    tx.execute(
        r#"
        INSERT INTO ui_arcade_variants(
            family_id,
            variant_ordinal,
            launchable_id,
            title,
            sort_title,
            launch_ref,
            preview_archive_path,
            preview_asset_key,
            has_preview,
            system_id,
            identity_id,
            parent_setname,
            asset_pack_id,
            asset_key,
            asset_link_reason,
            preferred,
            preferred_reason
        )
        WITH candidates AS (
            SELECT
                COALESCE(i.family_id, l.launchable_id) AS family_id,
                l.launchable_id AS launchable_id,
                l.title AS title,
                lower(l.title) AS sort_title,
                l.launch_ref AS launch_ref,
                l.system_id AS system_id,
                l.setname AS setname,
                i.identity_id AS identity_id,
                CASE
                    WHEN i.identity_id IS NOT NULL
                     AND i.family_id IS NOT NULL
                     AND i.identity_id != i.family_id
                    THEN i.family_id
                    ELSE NULL
                END AS parent_setname,
                CASE
                    WHEN i.identity_id IS NOT NULL
                     AND i.identity_id = COALESCE(i.family_id, i.identity_id)
                    THEN 1
                    ELSE 0
                END AS is_parent
            FROM launchables l
            JOIN games g ON g.game_id = l.launchable_id
            LEFT JOIN launchable_identities i
              ON i.launchable_id = l.launchable_id
             AND i.namespace = 'mame'
            WHERE l.system_id IN ('arcade','neogeo')
              AND l.launch_ref != ''
        ),
        resolved AS (
            SELECT
                *,
                CASE
                    WHEN system_id = 'neogeo' THEN ?2
                    ELSE ?1
                END AS preview_archive_path,
                COALESCE(NULLIF(family_id, ''), NULLIF(identity_id, ''), NULLIF(setname, ''), '') AS preview_key
            FROM candidates
        ),
        resolved_with_preview AS (
            SELECT
                *,
                CASE
                    WHEN preview_archive_path != '' AND preview_key != '' THEN 1
                    ELSE 0
                END AS preview_available
            FROM resolved
        ),
        ranked AS (
            SELECT
                *,
                row_number() OVER (
                    PARTITION BY family_id
                    ORDER BY is_parent DESC,
                             sort_title ASC,
                             launch_ref ASC
                ) AS family_rank,
                row_number() OVER (
                    PARTITION BY family_id
                    ORDER BY is_parent DESC,
                             sort_title ASC,
                             launch_ref ASC
                ) - 1 AS variant_ordinal
            FROM resolved_with_preview
        )
        SELECT
            family_id,
            variant_ordinal,
            launchable_id,
            title,
            sort_title,
            launch_ref,
            preview_archive_path,
            preview_key,
            preview_available,
            system_id,
            identity_id,
            parent_setname,
            NULL,
            preview_key,
            CASE WHEN preview_available = 1 THEN 'derived-family' ELSE 'none' END,
            CASE WHEN family_rank = 1 THEN 1 ELSE 0 END,
            CASE
                WHEN family_rank = 1 AND is_parent = 1 THEN 'installed-parent'
                WHEN family_rank = 1 THEN 'deterministic-child'
                ELSE 'variant'
            END
        FROM ranked
        ORDER BY family_id, variant_ordinal;
        "#,
        params![arcade_preview_archive_path, neogeo_preview_archive_path],
    )
    .map_err(|e| format!("materialize arcade ui variants: {e}"))?;
    tx.execute(
        r#"
        INSERT INTO ui_arcade_preferred(
            ordinal,
            launchable_id,
            title,
            sort_title,
            launch_ref,
            preview_archive_path,
            preview_asset_key,
            has_preview,
            system_id,
            identity_id,
            family_id,
            parent_setname,
            asset_pack_id,
            asset_key,
            asset_link_reason,
            preferred_reason
        )
        SELECT
            row_number() OVER (ORDER BY sort_title ASC, launch_ref ASC) - 1,
            launchable_id,
            title,
            sort_title,
            launch_ref,
            preview_archive_path,
            preview_asset_key,
            has_preview,
            system_id,
            identity_id,
            family_id,
            parent_setname,
            asset_pack_id,
            asset_key,
            asset_link_reason,
            preferred_reason
        FROM ui_arcade_variants
        WHERE preferred = 1
        ORDER BY sort_title ASC, launch_ref ASC;
        "#,
        [],
    )
    .map(|_| ())
    .map_err(|e| format!("materialize arcade ui projections: {e}"))
}


fn write_sqlite_scan(
    path: &Path,
    scan: &LibraryScan,
    progress: ProgressCallback<'_>,
    software_hash_cache: SoftwareHashCache,
    stamp: Option<&catalog_stamp::CatalogStamp>,
) -> Result<(), String> {
    let preview_paths = PreviewArchivePaths::from_paths(
        preview_worker::preview_archive_paths_for_catalog_projection(),
    );
    let mame_sqlite_path = default_mame_sqlite_path();
    let hbmame_sqlite_path = default_hbmame_sqlite_path();
    write_sqlite_scan_with_sources(
        path,
        scan,
        SqliteScanSources {
            mame_sqlite_path: &mame_sqlite_path,
            hbmame_sqlite_path: &hbmame_sqlite_path,
            preview_paths: &preview_paths,
            software_hash_cache,
            stamp,
        },
        progress,
    )
}

#[cfg(test)]
fn write_sqlite_scan_with_mame(
    path: &Path,
    scan: &LibraryScan,
    mame_sqlite_path: &Path,
) -> Result<(), String> {
    write_sqlite_scan_with_sources(
        path,
        scan,
        SqliteScanSources {
            mame_sqlite_path,
            hbmame_sqlite_path: &PathBuf::new(),
            preview_paths: &PreviewArchivePaths::default(),
            software_hash_cache: SoftwareHashCache::load(path),
            stamp: None,
        },
        None,
    )
}

#[cfg(test)]
fn write_sqlite_scan_with_mame_and_hbmame(
    path: &Path,
    scan: &LibraryScan,
    mame_sqlite_path: &Path,
    hbmame_sqlite_path: &Path,
) -> Result<(), String> {
    write_sqlite_scan_with_sources(
        path,
        scan,
        SqliteScanSources {
            mame_sqlite_path,
            hbmame_sqlite_path,
            preview_paths: &PreviewArchivePaths::default(),
            software_hash_cache: SoftwareHashCache::load(path),
            stamp: None,
        },
        None,
    )
}

#[cfg(test)]
fn write_sqlite_scan_with_mame_and_preview_pack(
    path: &Path,
    scan: &LibraryScan,
    mame_sqlite_path: &Path,
    preview_asset_pack: &preview_worker::PreviewArchiveIndex,
) -> Result<(), String> {
    let preview_paths = PreviewArchivePaths::from_paths(vec![preview_asset_pack.path.clone()]);
    write_sqlite_scan_with_sources(
        path,
        scan,
        SqliteScanSources {
            mame_sqlite_path,
            hbmame_sqlite_path: &PathBuf::new(),
            preview_paths: &preview_paths,
            software_hash_cache: SoftwareHashCache::load(path),
            stamp: None,
        },
        None,
    )
}

struct SqliteScanSources<'a> {
    mame_sqlite_path: &'a Path,
    hbmame_sqlite_path: &'a Path,
    preview_paths: &'a PreviewArchivePaths,
    software_hash_cache: SoftwareHashCache,
    stamp: Option<&'a catalog_stamp::CatalogStamp>,
}

fn write_sqlite_scan_with_sources(
    path: &Path,
    scan: &LibraryScan,
    mut sources: SqliteScanSources<'_>,
    mut progress: ProgressCallback<'_>,
) -> Result<(), String> {
    let total_t = Instant::now();
    let mut conn = Connection::open(path).map_err(|e| format!("open sqlite: {e}"))?;
    let schema_t = Instant::now();
    conn.execute_batch(
        r#"
        PRAGMA journal_mode=OFF;
        PRAGMA synchronous=OFF;
        PRAGMA temp_store=MEMORY;
        PRAGMA locking_mode=EXCLUSIVE;
        CREATE TABLE profiles (
            profile_id TEXT PRIMARY KEY,
            system_id TEXT NOT NULL,
            category TEXT NOT NULL,
            title TEXT NOT NULL,
            core_name TEXT NOT NULL,
            core_path TEXT,
            source_kind TEXT NOT NULL,
            source_detail TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE payloads (
            payload_id TEXT PRIMARY KEY,
            file_path TEXT NOT NULL,
            entry_path TEXT,
            launch_ref TEXT NOT NULL,
            profile_id TEXT,
            title TEXT NOT NULL,
            mount_kind TEXT,
            mount_index INTEGER,
            mount_delay_secs INTEGER,
            disposition TEXT NOT NULL,
            size INTEGER NOT NULL,
            mtime_secs INTEGER NOT NULL,
            source_kind TEXT NOT NULL,
            source_detail TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE systems (
            system_id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            category TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE games (
            game_id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            sort_title TEXT NOT NULL,
            system_id TEXT NOT NULL,
            manufacturer TEXT,
            genre TEXT,
            year INTEGER
        ) WITHOUT ROWID;
        CREATE TABLE launch_plans (
            plan_id TEXT PRIMARY KEY,
            game_id TEXT NOT NULL,
            profile_id TEXT,
            launch_kind TEXT NOT NULL,
            source_path TEXT NOT NULL,
            launch_ref TEXT NOT NULL,
            launcher_path TEXT,
            payload_path TEXT,
            core_id TEXT NOT NULL,
            hardware_id TEXT NOT NULL,
            setname TEXT,
            parent TEXT,
            priority INTEGER NOT NULL,
            confidence TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE launchables (
            launchable_id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            system_id TEXT NOT NULL,
            launch_kind TEXT NOT NULL,
            source_path TEXT NOT NULL,
            launch_ref TEXT NOT NULL,
            setname TEXT,
            core_id TEXT NOT NULL,
            hardware_id TEXT NOT NULL,
            confidence TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE launchable_identities (
            launchable_id TEXT NOT NULL,
            namespace TEXT NOT NULL,
            identity_id TEXT NOT NULL,
            family_id TEXT,
            metadata_title TEXT,
            year TEXT,
            manufacturer TEXT,
            source TEXT NOT NULL,
            PRIMARY KEY(launchable_id, namespace, identity_id)
        ) WITHOUT ROWID;
        CREATE TABLE ui_arcade_preferred (
            ordinal INTEGER PRIMARY KEY,
            launchable_id TEXT NOT NULL,
            title TEXT NOT NULL,
            sort_title TEXT NOT NULL,
            launch_ref TEXT NOT NULL,
            preview_archive_path TEXT NOT NULL,
            preview_asset_key TEXT NOT NULL,
            has_preview INTEGER NOT NULL,
            system_id TEXT NOT NULL,
            identity_id TEXT,
            family_id TEXT NOT NULL,
            parent_setname TEXT,
            asset_pack_id TEXT,
            asset_key TEXT,
            asset_link_reason TEXT NOT NULL,
            preferred_reason TEXT NOT NULL
        );
        CREATE TABLE ui_arcade_variants (
            family_id TEXT NOT NULL,
            variant_ordinal INTEGER NOT NULL,
            launchable_id TEXT NOT NULL,
            title TEXT NOT NULL,
            sort_title TEXT NOT NULL,
            launch_ref TEXT NOT NULL,
            preview_archive_path TEXT NOT NULL,
            preview_asset_key TEXT NOT NULL,
            has_preview INTEGER NOT NULL,
            system_id TEXT NOT NULL,
            identity_id TEXT,
            parent_setname TEXT,
            asset_pack_id TEXT,
            asset_key TEXT,
            asset_link_reason TEXT NOT NULL,
            preferred INTEGER NOT NULL,
            preferred_reason TEXT NOT NULL,
            PRIMARY KEY(family_id, variant_ordinal)
        ) WITHOUT ROWID;
        CREATE TABLE launcher_catalog (
            ordinal INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            sort_title TEXT NOT NULL,
            launch_ref TEXT NOT NULL,
            preview_archive_path TEXT NOT NULL,
            preview_asset_key TEXT NOT NULL,
            has_preview INTEGER NOT NULL,
            system_id TEXT NOT NULL
        );
        CREATE TABLE region_metadata (
            game_id TEXT PRIMARY KEY,
            inferred_region TEXT,
            confidence TEXT NOT NULL,
            override_region TEXT
        ) WITHOUT ROWID;
        CREATE TABLE meta (
            key TEXT PRIMARY KEY,
            value INTEGER NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE software_hash_cache (
            list_name TEXT NOT NULL,
            file_path TEXT NOT NULL,
            size INTEGER NOT NULL,
            mtime_secs INTEGER NOT NULL,
            software_name TEXT,
            PRIMARY KEY(list_name, file_path, size, mtime_secs)
        ) WITHOUT ROWID;
        CREATE TABLE catalog_stamp (
            ordinal INTEGER PRIMARY KEY,
            line TEXT NOT NULL
        );
        "#,
    )
    .map_err(|e| format!("create sqlite schema: {e}"))?;
    report_library_import_timing("schema", schema_t, "tables=14");

    let metadata_t = Instant::now();
    let mame_signature = file_signature(sources.mame_sqlite_path);
    let hbmame_signature = file_signature(sources.hbmame_sqlite_path);
    let software_metadata = load_mame_software_metadata(sources.mame_sqlite_path);
    let arcade_metadata =
        load_arcade_machine_metadata(sources.mame_sqlite_path, sources.hbmame_sqlite_path);
    report_library_import_timing(
        "metadata_load",
        metadata_t,
        format!(
            "mame={} hbmame={} software_lists={} preview_paths={}",
            arcade_metadata.mame.len(),
            arcade_metadata.hbmame.len(),
            software_metadata.items.len(),
            sources.preview_paths.len()
        ),
    );
    let tx_t = Instant::now();
    let tx = conn
        .transaction()
        .map_err(|e| format!("begin sqlite tx: {e}"))?;
    report_library_import_timing("begin_tx", tx_t, "");
    {
        let stage_t = Instant::now();
        let mut stmt = tx
            .prepare(
                "INSERT INTO profiles(profile_id,system_id,category,title,core_name,core_path,source_kind,source_detail)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            )
            .map_err(|e| format!("prepare profile insert: {e}"))?;
        for profile in launch_profiles::builtin_profiles() {
            stmt.execute(params![
                profile.id,
                profile.system_id,
                profile.category,
                profile.title,
                profile.core_name,
                profile.core_path,
                source_kind_name(profile.provenance.kind),
                profile.provenance.detail
            ])
            .map_err(|e| format!("insert profile: {e}"))?;
        }
        report_library_import_timing(
            "insert_profiles",
            stage_t,
            format!("rows={}", launch_profiles::builtin_profiles().len()),
        );
    }
    {
        let stage_t = Instant::now();
        let mut stmt = tx
            .prepare(
                "INSERT INTO payloads(payload_id,file_path,entry_path,launch_ref,profile_id,title,mount_kind,mount_index,mount_delay_secs,disposition,size,mtime_secs,source_kind,source_detail)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            )
            .map_err(|e| format!("prepare payload insert: {e}"))?;
        for payload in &scan.normal_files {
            let path = &payload.path;
            stmt.execute(params![
                format!("file:{path}"),
                path.as_str(),
                Option::<&str>::None,
                path.as_str(),
                payload.profile_id.as_str(),
                title_from_path(path),
                mount_kind_str(payload.rule.mount.kind),
                payload.rule.mount.index as i64,
                payload.rule.mount.delay_secs as i64,
                payload_disposition_str(payload.rule.disposition),
                payload.size as i64,
                payload.mtime_secs,
                source_kind_name(payload.rule.provenance.kind),
                payload.rule.provenance.detail
            ])
            .map_err(|e| format!("insert payload file: {e}"))?;
        }
        for entry in &scan.entries {
            stmt.execute(params![
                format!("entry:{}", entry.launch_ref),
                entry.file_path.as_str(),
                entry.entry_path.as_str(),
                entry.launch_ref.as_str(),
                entry.profile_id.as_str(),
                entry.normalized_title.as_str(),
                mount_kind_str(entry.rule.mount.kind),
                entry.rule.mount.index as i64,
                entry.rule.mount.delay_secs as i64,
                if entry.launchable {
                    "candidate"
                } else {
                    "support"
                },
                entry
                    .uncompressed_size
                    .or(entry.compressed_size)
                    .unwrap_or(0) as i64,
                0i64,
                source_kind_name(entry.rule.provenance.kind),
                entry.rule.provenance.detail
            ])
            .map_err(|e| format!("insert payload entry: {e}"))?;
        }
        report_library_import_timing(
            "insert_payloads",
            stage_t,
            format!(
                "normal_files={} entries={}",
                scan.normal_files.len(),
                scan.entries.len()
            ),
        );
    }
    {
        let stage_t = Instant::now();
        let mut launcher_rows = Vec::<CatalogRow>::new();
        let mut system_stmt = tx
            .prepare("INSERT OR IGNORE INTO systems(system_id,title,category) VALUES (?1,?2,?3)")
            .map_err(|e| format!("prepare system insert: {e}"))?;
        let mut game_stmt = tx
            .prepare(
                "INSERT INTO games(game_id,title,sort_title,system_id,manufacturer,genre,year)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(|e| format!("prepare game insert: {e}"))?;
        let mut plan_stmt = tx
            .prepare(
                "INSERT INTO launch_plans(plan_id,game_id,profile_id,launch_kind,source_path,launch_ref,launcher_path,payload_path,core_id,hardware_id,setname,parent,priority,confidence)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            )
            .map_err(|e| format!("prepare launch plan insert: {e}"))?;
        let mut launchable_stmt = tx
            .prepare(
                "INSERT INTO launchables(launchable_id,title,system_id,launch_kind,source_path,launch_ref,setname,core_id,hardware_id,confidence)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            )
            .map_err(|e| format!("prepare launchable insert: {e}"))?;
        let mut identity_stmt = tx
            .prepare(
                "INSERT INTO launchable_identities(launchable_id,namespace,identity_id,family_id,metadata_title,year,manufacturer,source)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            )
            .map_err(|e| format!("prepare launchable identity insert: {e}"))?;
        let mut region_stmt = tx
            .prepare(
                "INSERT INTO region_metadata(game_id,inferred_region,confidence,override_region)
                 VALUES (?1,?2,?3,?4)",
            )
            .map_err(|e| format!("prepare region metadata insert: {e}"))?;
        let covered_payloads = covered_payload_paths(&scan.discoveries);
        let discoveries =
            preferred_playable_discoveries_by_key(&scan.discoveries, &covered_payloads);
        let discovery_total = discoveries.len();
        report_sqlite_import_progress(&mut progress, 0, discovery_total);
        let mut chunk_t = Instant::now();
        let mut chunk_start = 0usize;
        for (idx, (key, discovery)) in discoveries.into_iter().enumerate() {
            if idx > 0 && idx % 250 == 0 {
                report_sqlite_import_progress(&mut progress, idx, discovery_total);
            }
            let system_id = catalog_system_id_for_discovery(discovery);
            let software_identity = mame_software_identity_for_discovery(
                discovery,
                &software_metadata,
                &mut sources.software_hash_cache,
            );
            let preview_asset = software_identity
                .as_ref()
                .and_then(|identity| console_preview_asset(identity, sources.preview_paths));
            let game_has_preview = preview_asset.is_some();
            system_stmt
                .execute(params![
                    system_id.as_str(),
                    system_title_for_discovery(discovery, &system_id),
                    discovery.category.as_str()
                ])
                .map_err(|e| format!("insert system: {e}"))?;
            game_stmt
                .execute(params![
                    key.as_str(),
                    discovery.title.as_str(),
                    normalize_title(&discovery.title),
                    system_id.as_str(),
                    discovery.manufacturer.as_deref(),
                    discovery.genre.as_deref(),
                    discovery.year.map(|n| n as i64)
                ])
                .map_err(|e| format!("insert game: {e}"))?;
            let launcher_path = match discovery.source_kind {
                DiscoverySourceKind::Mra | DiscoverySourceKind::Mgl => {
                    Some(discovery.launch_ref.as_str())
                }
                DiscoverySourceKind::PayloadFile
                | DiscoverySourceKind::ArchiveEntry
                | DiscoverySourceKind::CatalogEntry => None,
            };
            let payload_path = if launcher_path.is_none() {
                Some(discovery.launch_ref.as_str())
            } else {
                None
            };
            let plan_launch_ref = launch_ref_for_discovery(&key, discovery);
            if is_launcher_launch_ref(&plan_launch_ref)
                && system_id != "arcade"
                && system_id != "neogeo"
            {
                let software_family_key = software_identity
                    .as_ref()
                    .map(|identity| format!("mame-software:{}", identity.family_id));
                launcher_rows.push(CatalogRow {
                    game: ArcadeGameEntry {
                        title: discovery.title.clone().into(),
                        mra_path: plan_launch_ref.clone().into(),
                        preview_archive_path: preview_asset
                            .as_ref()
                            .map(|asset| asset.archive_path.as_str())
                            .unwrap_or_default()
                            .into(),
                        preview_asset_key: preview_asset
                            .as_ref()
                            .map(|asset| asset.asset_key.as_str())
                            .unwrap_or_default()
                            .into(),
                        has_preview: game_has_preview,
                        system_id: system_id.clone().into(),
                    },
                    source_kind: launch_kind_for_discovery(discovery).to_string(),
                    setname: discovery.setname.clone().unwrap_or_default(),
                    parent: discovery.parent.clone().unwrap_or_default(),
                    family_key: software_family_key,
                });
            }
            plan_stmt
                .execute(params![
                    format!("plan:{key}"),
                    key.as_str(),
                    profile_id_for_discovery(discovery),
                    launch_kind_for_discovery(discovery),
                    discovery.source_path.as_str(),
                    plan_launch_ref.as_str(),
                    launcher_path,
                    payload_path,
                    discovery.core_id.as_str(),
                    discovery.hardware_id.as_str(),
                    discovery.setname.as_deref(),
                    discovery.parent.as_deref(),
                    0i64,
                    confidence_str(discovery.confidence)
                ])
                .map_err(|e| format!("insert launch plan: {e}"))?;
            launchable_stmt
                .execute(params![
                    key.as_str(),
                    discovery.title.as_str(),
                    system_id.as_str(),
                    launch_kind_for_discovery(discovery),
                    discovery.source_path.as_str(),
                    plan_launch_ref.as_str(),
                    discovery.setname.as_deref(),
                    discovery.core_id.as_str(),
                    discovery.hardware_id.as_str(),
                    confidence_str(discovery.confidence)
                ])
                .map_err(|e| format!("insert launchable: {e}"))?;
            if let Some(identity_id) = mame_identity_for_discovery(discovery) {
                let (family_id, title, year, manufacturer, source) = mame_identity_projection(
                    &identity_id,
                    &arcade_metadata,
                    discovery.parent.as_deref(),
                );
                identity_stmt
                    .execute(params![
                        key.as_str(),
                        "mame",
                        identity_id.as_str(),
                        family_id.as_str(),
                        title,
                        year,
                        manufacturer,
                        source
                    ])
                    .map_err(|e| format!("insert launchable identity: {e}"))?;
            }
            if let Some(identity) = software_identity.as_ref() {
                let identity_id = format!("{}:{}", identity.list_name, identity.software_name);
                identity_stmt
                    .execute(params![
                        key.as_str(),
                        "mame-software",
                        identity_id.as_str(),
                        identity.family_id.as_str(),
                        identity.metadata_title.as_deref(),
                        identity.year.as_deref(),
                        identity.manufacturer.as_deref(),
                        identity.source
                    ])
                    .map_err(|e| format!("insert software launchable identity: {e}"))?;
            }
            let region = media_metadata::infer_region_metadata(discovery);
            let region = if let Some(identity) = software_identity.as_ref() {
                if let Some(region) = identity
                    .region
                    .as_deref()
                    .and_then(media_metadata::canonical_region_static)
                {
                    media_metadata::RegionInference {
                        region: Some(region),
                        confidence: identity.source,
                    }
                } else {
                    region
                }
            } else {
                region
            };
            region_stmt
                .execute(params![
                    key.as_str(),
                    region.region,
                    region.confidence,
                    Option::<&str>::None
                ])
                .map_err(|e| format!("insert region metadata: {e}"))?;
            let written = idx + 1;
            if written % 1000 == 0 || written == discovery_total {
                report_library_import_timing(
                    "insert_games_chunk",
                    chunk_t,
                    format!(
                        "from={} to={} total={discovery_total}",
                        chunk_start, written
                    ),
                );
                chunk_t = Instant::now();
                chunk_start = written;
            }
        }
        report_sqlite_import_progress(&mut progress, discovery_total, discovery_total);
        drop(region_stmt);
        drop(identity_stmt);
        drop(launchable_stmt);
        drop(plan_stmt);
        drop(game_stmt);
        drop(system_stmt);
        report_library_import_timing(
            "insert_games_total",
            stage_t,
            format!(
                "rows={discovery_total} launcher_rows={}",
                launcher_rows.len()
            ),
        );
        report_sqlite_import_finalizing(&mut progress);
        let projection_t = Instant::now();
        materialize_arcade_ui_projections(
            &tx,
            sources
                .preview_paths
                .archive_for_platform("arcade")
                .unwrap_or_default(),
            sources
                .preview_paths
                .archive_for_platform("neogeo")
                .unwrap_or_default(),
        )?;
        report_library_import_timing("materialize_arcade_ui", projection_t, "");
        let launcher_arcade_t = Instant::now();
        tx.execute(
            "INSERT INTO launcher_catalog(ordinal,title,sort_title,launch_ref,preview_archive_path,preview_asset_key,has_preview,system_id)
             SELECT ordinal,title,sort_title,launch_ref,preview_archive_path,preview_asset_key,has_preview,system_id
             FROM ui_arcade_preferred
             ORDER BY ordinal",
            [],
        )
        .map_err(|e| format!("insert preferred launcher catalog: {e}"))?;
        report_library_import_timing("insert_launcher_arcade", launcher_arcade_t, "");
        let ordinal_offset = tx
            .query_row("SELECT count(*) FROM launcher_catalog", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|e| format!("query launcher catalog offset: {e}"))?;
        let launcher_console_t = Instant::now();
        launcher_rows.sort_by_cached_key(|row| row.game.title.to_ascii_lowercase());
        let launcher_games = collapse_catalog_variants(launcher_rows);
        let mut launcher_stmt = tx
            .prepare(
                "INSERT INTO launcher_catalog(ordinal,title,sort_title,launch_ref,preview_archive_path,preview_asset_key,has_preview,system_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            )
            .map_err(|e| format!("prepare launcher catalog insert: {e}"))?;
        for (idx, game) in launcher_games.iter().enumerate() {
            launcher_stmt
                .execute(params![
                    ordinal_offset + idx as i64,
                    game.title.as_ref(),
                    normalize_title(&game.title),
                    game.mra_path.as_ref(),
                    game.preview_archive_path.as_ref(),
                    game.preview_asset_key.as_ref(),
                    if game.has_preview { 1 } else { 0 },
                    game.system_id.as_ref()
                ])
                .map_err(|e| format!("insert launcher catalog: {e}"))?;
        }
        report_library_import_timing(
            "insert_launcher_console",
            launcher_console_t,
            format!("rows={}", launcher_games.len()),
        );
    }
    {
        let stage_t = Instant::now();
        let mut stmt = tx
            .prepare("INSERT INTO meta(key,value) VALUES (?1,?2)")
            .map_err(|e| format!("prepare meta insert: {e}"))?;
        stmt.execute(params!["version", scan.version as i64])
            .map_err(|e| format!("insert version: {e}"))?;
        stmt.execute(params!["scanned_at_unix", scan.scanned_at_unix])
            .map_err(|e| format!("insert scanned_at_unix: {e}"))?;
        stmt.execute(params!["normal_files", scan.normal_files.len() as i64])
            .map_err(|e| format!("insert normal count: {e}"))?;
        stmt.execute(params!["containers", scan.containers.len() as i64])
            .map_err(|e| format!("insert container count: {e}"))?;
        stmt.execute(params!["entries", scan.entries.len() as i64])
            .map_err(|e| format!("insert entry count: {e}"))?;
        stmt.execute(params!["ignored_files", scan.ignored_files as i64])
            .map_err(|e| format!("insert ignored count: {e}"))?;
        stmt.execute(params![
            "discoveries",
            unique_discovery_count(&scan.discoveries) as i64
        ])
        .map_err(|e| format!("insert discovery count: {e}"))?;
        stmt.execute(params!["mame_metadata_size", mame_signature.size as i64])
            .map_err(|e| format!("insert mame metadata size: {e}"))?;
        stmt.execute(params!["mame_metadata_mtime", mame_signature.mtime_secs])
            .map_err(|e| format!("insert mame metadata mtime: {e}"))?;
        stmt.execute(params![
            "hbmame_metadata_size",
            hbmame_signature.size as i64
        ])
        .map_err(|e| format!("insert hbmame metadata size: {e}"))?;
        stmt.execute(params![
            "hbmame_metadata_mtime",
            hbmame_signature.mtime_secs
        ])
        .map_err(|e| format!("insert hbmame metadata mtime: {e}"))?;
        report_library_import_timing("insert_meta", stage_t, "rows=11");
    }
    if let Some(stamp) = sources.stamp {
        let stage_t = Instant::now();
        catalog_store::write_catalog_stamp(&tx, stamp)?;
        report_library_import_timing(
            "insert_catalog_stamp",
            stage_t,
            format!("rows={}", stamp.lines().len()),
        );
    }
    {
        let stage_t = Instant::now();
        let mut stmt = tx
            .prepare(
                "INSERT INTO software_hash_cache(list_name,file_path,size,mtime_secs,software_name)
                 VALUES (?1,?2,?3,?4,?5)",
            )
            .map_err(|e| format!("prepare software hash cache insert: {e}"))?;
        for (key, software_name) in &sources.software_hash_cache.entries {
            stmt.execute(params![
                key.list_name.as_str(),
                key.file_path.as_str(),
                key.size as i64,
                key.mtime_secs,
                software_name.as_deref()
            ])
            .map_err(|e| format!("insert software hash cache: {e}"))?;
        }
        report_library_import_timing(
            "insert_software_hash_cache",
            stage_t,
            format!("rows={}", sources.software_hash_cache.entries.len()),
        );
    }
    let commit_t = Instant::now();
    tx.commit().map_err(|e| format!("commit sqlite tx: {e}"))?;
    report_library_import_timing("commit", commit_t, "");
    report_library_import_timing("total", total_t, format!("path={}", path.display()));
    Ok(())
}

fn report_library_import_timing(stage: &str, started: Instant, detail: impl std::fmt::Display) {
    println!(
        "library_import_timing\t{stage}\t{}\t{detail}",
        started.elapsed().as_micros()
    );
}

pub(crate) fn report_library_scan_timing(stage: &str, us: u64, detail: impl std::fmt::Display) {
    println!("library_scan_timing\t{stage}\t{us}\t{detail}");
}

fn report_sqlite_import_progress(
    progress: &mut ProgressCallback<'_>,
    written: usize,
    total: usize,
) {
    if let Some(report) = progress.as_mut() {
        report(
            "Saving library",
            &format!("Writing {written} of {total} games into SQLite..."),
        );
    }
}

fn report_sqlite_import_finalizing(progress: &mut ProgressCallback<'_>) {
    if let Some(report) = progress.as_mut() {
        report(
            "Saving library",
            "Finalizing catalog views and search indexes...",
        );
    }
}

fn sqlite_cached_summary(path: &Path, scan_us: u64) -> Result<LibraryRefreshSummary, String> {
    let conn = open_sqlite_read_only(path).map_err(|e| format!("open cached summary: {e}"))?;
    ensure_sqlite_schema_current(&conn)?;
    let bytes = std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("stat cached summary db: {e}"))?;
    Ok(LibraryRefreshSummary {
        skipped: true,
        scan_us,
        discover_us: 0,
        classify_us: 0,
        import_us: 0,
        bytes,
        normal_files: sqlite_meta_usize(&conn, "normal_files").unwrap_or(0),
        containers: sqlite_meta_usize(&conn, "containers").unwrap_or(0),
        entries: sqlite_meta_usize(&conn, "entries").unwrap_or(0),
        discoveries: sqlite_meta_usize(&conn, "discoveries").unwrap_or(0),
    })
}

fn ensure_sqlite_schema_current(conn: &Connection) -> Result<(), String> {
    match sqlite_meta_usize(conn, "version") {
        Some(version) if version == SCHEMA_VERSION as usize => Ok(()),
        Some(version) => Err(format!(
            "catalog schema mismatch: expected {SCHEMA_VERSION}, found {version}"
        )),
        None => Err(format!(
            "catalog schema mismatch: expected {SCHEMA_VERSION}, found missing"
        )),
    }
}

fn sqlite_meta_usize(conn: &Connection, key: &str) -> Option<usize> {
    conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| {
        r.get::<_, i64>(0)
    })
    .ok()
    .map(|n| n.max(0) as usize)
}


fn source_kind_name(kind: RuleSourceKind) -> &'static str {
    match kind {
        RuleSourceKind::MainSource => "main-source",
        RuleSourceKind::Mgl => "mgl",
        RuleSourceKind::Mra => "mra",
        RuleSourceKind::ConfStr => "conf-str",
        RuleSourceKind::MagikProfile => "magik-profile",
    }
}

fn mount_kind_str(kind: MountKind) -> &'static str {
    match kind {
        MountKind::Launcher => "launcher",
        MountKind::LoadFile => "load-file",
        MountKind::MountImage => "mount-image",
        MountKind::Core => "core",
    }
}

fn payload_disposition_str(disposition: PayloadDisposition) -> &'static str {
    match disposition {
        PayloadDisposition::Playable => "playable",
        PayloadDisposition::AttachedMedia => "attached-media",
    }
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

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    type MameMachineFixture<'a> = (
        &'a str,
        Option<&'a str>,
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
    );

    #[test]
    fn profile_ignored_support_files_do_not_become_payloads() {
        let profiles = launch_profiles::builtin_profiles();

        assert!(matches!(
            classify_profile_path(&profiles, Path::new("/media/fat/games/Saturn/boot.rom")),
            Some((profile, ProfilePathClass::Ignored { reason: launch_profiles::IgnoreReason::Bios, .. }))
                if profile.id == "saturn"
        ));
        assert!(matches!(
            classify_profile_path(&profiles, Path::new("/media/fat/games/AO486/boot1.rom")),
            Some((profile, ProfilePathClass::Ignored { reason: launch_profiles::IgnoreReason::Bios, .. }))
                if profile.id == "ao486"
        ));
    }

    #[test]
    fn raw_profile_payloads_generate_virtual_games() {
        let discoveries = vec![
            payload("/media/fat/games/NES/Super Mario Bros.nes"),
            payload("/media/fat/games/Saturn/Guardian Heroes.cue"),
        ];

        assert_eq!(unique_discovery_count(&discoveries), 2);
    }

    #[test]
    fn saturn_region_prefers_filename_markers() {
        let discovery = saturn_payload("/media/fat/games/Saturn/Nights into Dreams (USA).chd");

        assert_eq!(
            infer_region_metadata(&discovery),
            RegionInference {
                region: Some("usa"),
                confidence: "filename-high"
            }
        );
    }

    #[test]
    fn saturn_region_uses_folder_when_filename_has_no_marker() {
        let discovery = saturn_payload("/media/fat/games/Saturn/Japan/Princess Crown.chd");

        assert_eq!(
            infer_region_metadata(&discovery),
            RegionInference {
                region: Some("japan"),
                confidence: "folder-medium"
            }
        );
    }

    #[test]
    fn saturn_region_stays_unknown_without_evidence() {
        let discovery = saturn_payload("/media/fat/games/Saturn/Clockwork Knight.chd");

        assert_eq!(
            infer_region_metadata(&discovery),
            RegionInference {
                region: None,
                confidence: "unknown"
            }
        );
    }

    #[test]
    fn saturn_boot_header_extracts_product_and_area() {
        let mut header = [b' '; 256];
        header[0..15].copy_from_slice(b"SEGA SEGASATURN");
        header[0x20..0x2a].copy_from_slice(b"T-12345G  ");
        header[0x40..0x50].copy_from_slice(b"JTUE            ");

        let parsed = parse_saturn_boot_header(&header).expect("saturn header");

        assert_eq!(parsed.product_id.as_deref(), Some("T-12345G"));
        assert_eq!(parsed.region, Some("usa"));
    }

    #[test]
    fn rbf_cores_are_not_profile_candidates() {
        let profiles = launch_profiles::builtin_profiles();

        assert!(classify_profile_path(
            &profiles,
            Path::new("/media/fat/_Computer/AcornAtom_20251001.rbf")
        )
        .is_none());
        assert!(classify_profile_path(
            &profiles,
            Path::new("/media/fat/_LLAPI/NES_LLAPI_20251206.rbf")
        )
        .is_none());
    }

    #[test]
    fn profile_for_path_prefers_directory_after_games_component() {
        let profiles = launch_profiles::builtin_profiles();
        let profile = profile_for_path(
            &profiles,
            Path::new("/media/fat/collections/NES/games/NeoGeo/mslug3.neo"),
        )
        .expect("profile");

        assert_eq!(profile.id, "neogeo");
    }

    #[test]
    fn menu_mgl_launchers_are_not_profile_candidates() {
        let profiles = launch_profiles::builtin_profiles();

        assert!(
            classify_profile_path(&profiles, Path::new("/media/fat/_Computer/Amiga.mgl")).is_none()
        );
        assert!(
            classify_profile_path(&profiles, Path::new("/media/fat/_Console/Game Gear.mgl"))
                .is_none()
        );
    }

    #[test]
    fn dos_mgl_games_still_count_as_games() {
        let discoveries = vec![mgl(
            "/media/fat/_DOS Games/Doom (Ultimate).mgl",
            "/media/fat/_DOS Games/Doom (Ultimate).mgl",
        )];

        assert_eq!(unique_discovery_count(&discoveries), 1);
    }

    #[test]
    fn dos_mgl_discovery_uses_dos_system_without_payload_inference() {
        let root = unique_temp_dir("dos-mgl-profile");
        let dos_dir = root.join("_DOS Games");
        std::fs::create_dir_all(&dos_dir).expect("create dos dir");
        let path = dos_dir.join("Doom (Ultimate).mgl");
        std::fs::write(
            &path,
            r#"<mistergamelist><rbf>AO486</rbf><file delay="1" type="s"/></mistergamelist>"#,
        )
        .expect("write dos mgl fixture");
        let meta = std::fs::metadata(&path).expect("stat dos mgl fixture");
        let file = FoundFile {
            path: path.clone(),
            ext: "mgl".to_string(),
            size: meta.len(),
            mtime_secs: mtime_secs(&meta),
        };

        let profiles = launch_profiles::builtin_profiles();
        let profile = profile_for_path(&profiles, &path).expect("dos profile");
        let payload_rule = profile.payload_rules[0];
        let discovery = discovery_from_profile_file(&file, profile, &payload_rule, &profiles);

        assert_eq!(profile.id, "dos");
        assert_eq!(discovery.platform_id, "dos");
        assert_eq!(catalog_system_id_for_discovery(&discovery), "dos");
        assert_eq!(system_title_for_discovery(&discovery, "dos"), "DOS Games");
    }

    #[test]
    fn mra_metadata_parser_tolerates_attributes_and_entities() {
        let root = unique_temp_dir("mra-xml-metadata");
        std::fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("fixture.mra");
        std::fs::write(
            &path,
            r#"
            <misterromdescription>
                <name lang="en">Battle &amp; Chase</name>
                <rbf version="1">JTCPS2</rbf>
                <platform>Capcom Play System II</platform>
                <manufacturer>Capcom &quot;Co&quot;</manufacturer>
                <category>Driving</category>
                <catver>Racing / Chase</catver>
                <year>1997</year>
                <setname>batcir</setname>
                <parent>batcirj</parent>
            </misterromdescription>
            "#,
        )
        .expect("write mra fixture");

        let metadata = read_mra_metadata(&path).expect("read mra metadata");

        assert_eq!(metadata.name.as_deref(), Some("Battle & Chase"));
        assert_eq!(metadata.rbf.as_deref(), Some("JTCPS2"));
        assert_eq!(metadata.platform.as_deref(), Some("Capcom Play System II"));
        assert_eq!(metadata.manufacturer.as_deref(), Some("Capcom \"Co\""));
        assert_eq!(metadata.category.as_deref(), Some("Driving"));
        assert_eq!(metadata.catver.as_deref(), Some("Racing / Chase"));
        assert_eq!(metadata.year.as_deref(), Some("1997"));
        assert_eq!(metadata.setname.as_deref(), Some("batcir"));
        assert_eq!(metadata.parent.as_deref(), Some("batcirj"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mgl_metadata_parser_uses_file_path_not_unrelated_path_attribute() {
        let root = unique_temp_dir("mgl-xml-file-path");
        std::fs::create_dir_all(&root).expect("create temp root");
        let path = root.join("Fixture.mgl");
        std::fs::write(
            &path,
            r#"
            <mistergamelist>
                <metadata path="not/a/game.rom"/>
                <rbf>NES</rbf>
                <file delay="1" type="s" path='games/NES/Super Mario Bros.nes'/>
            </mistergamelist>
            "#,
        )
        .expect("write mgl fixture");

        let metadata = read_mgl_metadata(&path).expect("read mgl metadata");

        assert_eq!(metadata.rbf.as_deref(), Some("NES"));
        assert_eq!(
            metadata.file_path.as_deref(),
            Some("games/NES/Super Mario Bros.nes")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mgl_discovery_preserves_script_as_launch_ref() {
        let path =
            std::env::temp_dir().join(format!("mister-magik-mgl-test-{}.mgl", std::process::id()));
        std::fs::write(
            &path,
            r#"<mistergamelist><file delay="2" type="s" path="games/NES/Mario.nes"/></mistergamelist>"#,
        )
        .expect("write mgl fixture");
        let meta = std::fs::metadata(&path).expect("stat mgl fixture");
        let file = FoundFile {
            path: path.clone(),
            ext: "mgl".to_string(),
            size: meta.len(),
            mtime_secs: mtime_secs(&meta),
        };

        let profiles = launch_profiles::builtin_profiles();
        let profile = profiles
            .iter()
            .find(|profile| profile.id == "mgl")
            .expect("mgl profile");
        let payload_rule = profile.payload_rules[0];
        let discovery = discovery_from_profile_file(&file, profile, &payload_rule, &profiles);

        assert_eq!(discovery.source_path, path.display().to_string());
        assert_eq!(discovery.launch_ref, path.display().to_string());
        assert_eq!(discovery.platform_id, "nes");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn neogeo_mgl_discovery_uses_payload_setname() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-neogeo-mgl-test-{}.mgl",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"<mistergamelist><rbf>NeoGeo</rbf><file delay="2" type="s" path="/media/fat/games/NeoGeo/Neo Geo Mister FGPA Ultra Pack.zip/Neo Geo Mister FGPA Ultra Pack/ World A-Z/Metal Slug 3 (mslug3).neo"/></mistergamelist>"#,
        )
        .expect("write mgl fixture");
        let meta = std::fs::metadata(&path).expect("stat mgl fixture");
        let file = FoundFile {
            path: path.clone(),
            ext: "mgl".to_string(),
            size: meta.len(),
            mtime_secs: mtime_secs(&meta),
        };

        let profiles = launch_profiles::builtin_profiles();
        let profile = profiles
            .iter()
            .find(|profile| profile.id == "mgl")
            .expect("mgl profile");
        let payload_rule = profile.payload_rules[0];
        let discovery = discovery_from_profile_file(&file, profile, &payload_rule, &profiles);

        assert_eq!(discovery.platform_id, "neogeo");
        assert_eq!(discovery.setname.as_deref(), Some("mslug3"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn mgl_covered_payload_does_not_get_virtual_duplicate() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-mgl-dedupe-test-{}.mgl",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"<mistergamelist><file delay="1" type="f" path="games/NES/Mario.nes"/></mistergamelist>"#,
        )
        .expect("write mgl fixture");
        let discoveries = vec![
            mgl(&path.display().to_string(), &path.display().to_string()),
            payload("/media/fat/games/NES/Mario.nes"),
        ];

        assert_eq!(unique_discovery_count(&discoveries), 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn mra_files_remain_playable_launchers() {
        let discovery = GameDiscovery {
            source_path: "/media/fat/_Arcade/BIOS.mra".to_string(),
            launch_ref: "/media/fat/_Arcade/BIOS.mra".to_string(),
            source_kind: DiscoverySourceKind::Mra,
            title: "BIOS".to_string(),
            category: "Arcade".to_string(),
            platform_id: "arcade".to_string(),
            core_id: "arcade".to_string(),
            hardware_id: "arcade-unknown".to_string(),
            manufacturer: None,
            genre: None,
            year: None,
            setname: None,
            parent: None,
            confidence: DiscoveryConfidence::MraCore,
        };

        assert!(is_playable_discovery(&discovery));
    }

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
    fn disc_variant_scoring_does_not_treat_disc_ten_as_disc_one() {
        assert_eq!(
            first_disc_number_from_haystack("/media/fat/games/Saturn/Game Disc 1.chd"),
            Some(1)
        );
        assert_eq!(
            first_disc_number_from_haystack("/media/fat/games/Saturn/Game Disc 10.chd"),
            Some(10)
        );
        assert!(
            variant_score_from_haystack("/media/fat/games/Saturn/Game Disc 1.chd")
                > variant_score_from_haystack("/media/fat/games/Saturn/Game Disc 10.chd")
        );
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

    #[test]
    fn neogeo_zip_entries_generate_virtual_launches_and_system() {
        let root = unique_temp_dir("neogeo-zip-entries");
        let neogeo_dir = root.join("games/NEOGEO");
        std::fs::create_dir_all(&neogeo_dir).expect("create neogeo dir");
        let zip_path = neogeo_dir.join("Neo Geo Mister FGPA Ultra Pack.zip");
        write_stored_zip(
            &zip_path,
            &[
                (
                    "Neo Geo Mister FGPA Ultra Pack/ World A-Z/Neo Bomberman (neobombe).neo",
                    b"neo",
                ),
                ("Neo Geo Mister FGPA Ultra Pack/readme.txt", b"ignore"),
            ],
        );
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.containers.len(), 1);
        assert_eq!(scan.entries.len(), 1);
        assert!(scan.normal_files.is_empty());
        let discovery = scan
            .discoveries
            .iter()
            .find(|d| d.source_kind == DiscoverySourceKind::ArchiveEntry)
            .expect("archive entry discovery");
        assert_eq!(discovery.platform_id, "neogeo");
        assert!(discovery.launch_ref.ends_with(
            "Neo Geo Mister FGPA Ultra Pack.zip/Neo Geo Mister FGPA Ultra Pack/ World A-Z/Neo Bomberman (neobombe).neo"
        ));

        save_sqlite_scan(&db, &scan).expect("save sqlite");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");

        assert_eq!(loaded.rows, 1);
        assert_eq!(loaded.catalog.games[0].system_id.as_ref(), "neogeo");
        assert!(loaded.catalog.games[0].mra_path.starts_with("magik-plan:"));
        assert!(loaded
            .catalog
            .systems
            .iter()
            .any(|system| system.id == "neogeo" && system.count == 1));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn zip_central_directory_scans_entries_with_extra_and_comment_padding() {
        let root = unique_temp_dir("zip-central-padding");
        std::fs::create_dir_all(&root).expect("create temp root");
        let zip_path = root.join("games.zip");
        write_stored_zip_with_central_metadata(
            &zip_path,
            &[("World A-Z/Neo Bomberman (neobombe).neo", b"neo".as_slice())],
            b"extra",
            b"comment",
        );
        let meta = std::fs::metadata(&zip_path).expect("stat zip");
        let file = FoundFile {
            path: zip_path.clone(),
            ext: "zip".to_string(),
            size: meta.len(),
            mtime_secs: mtime_secs(&meta),
        };
        let profiles = launch_profiles::builtin_profiles();
        let profile = profiles
            .iter()
            .find(|profile| profile.id == "neogeo")
            .expect("neogeo profile");

        let entries = scan_zip_central_directory(&file, profile).expect("scan zip");

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].entry_path,
            "World A-Z/Neo Bomberman (neobombe).neo"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_ignores_gamelists_and_screenshot_media_dirs() {
        let root = unique_temp_dir("ignore-screenshot-media");
        let nes_dir = root.join("games/NES");
        let screenshot_dir = nes_dir.join("screenshot");
        std::fs::create_dir_all(&screenshot_dir).expect("create screenshot dir");
        std::fs::write(nes_dir.join("Mario.nes"), "rom").expect("write rom");
        std::fs::write(
            nes_dir.join("gamelist.xml"),
            "<game><path>./Mario.nes</path><image>./screenshot/Mario.png</image></game>",
        )
        .expect("write gamelist");
        std::fs::write(screenshot_dir.join("Not A Game.nes"), "media").expect("write media");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.normal_files.len(), 1);
        assert_eq!(scan.discoveries.len(), 1);
        assert_eq!(
            scan.normal_files[0].path,
            nes_dir.join("Mario.nes").display().to_string()
        );
        assert!(scan
            .discoveries
            .iter()
            .all(|discovery| !discovery.launch_ref.contains("gamelist.xml")
                && !discovery.launch_ref.contains("Not A Game.nes")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_uses_profile_game_dirs_instead_of_walking_every_games_child() {
        let root = unique_temp_dir("target-profile-game-dirs");
        let nes_dir = root.join("games/NES");
        let unrelated_dir = root.join("games/NotACoreProfile");
        std::fs::create_dir_all(&nes_dir).expect("create nes dir");
        std::fs::create_dir_all(&unrelated_dir).expect("create unrelated dir");
        std::fs::write(nes_dir.join("Mario.nes"), "rom").expect("write nes rom");
        std::fs::write(unrelated_dir.join("Ghost.nes"), "rom").expect("write unrelated rom");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.normal_files.len(), 1);
        assert_eq!(
            scan.normal_files[0].path,
            nes_dir.join("Mario.nes").display().to_string()
        );
        assert!(scan
            .discoveries
            .iter()
            .all(|discovery| !discovery.launch_ref.contains("Ghost.nes")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_prunes_arcade_media_and_cores_but_keeps_arcade_game_mras() {
        let root = unique_temp_dir("target-arcade-game-dirs");
        let arcade_dir = root.join("_Arcade");
        let media_dir = arcade_dir.join("media");
        let cores_dir = arcade_dir.join("cores");
        let alternatives_dir = arcade_dir.join("_alternatives/_Alt");
        std::fs::create_dir_all(&media_dir).expect("create media dir");
        std::fs::create_dir_all(&cores_dir).expect("create cores dir");
        std::fs::create_dir_all(&alternatives_dir).expect("create alternatives dir");
        std::fs::write(
            arcade_dir.join("Real Game.mra"),
            "<misterromdescription><name>Real Game</name><setname>realgame</setname></misterromdescription>",
        )
        .expect("write real mra");
        std::fs::write(
            alternatives_dir.join("Alt Game.mra"),
            "<misterromdescription><name>Alt Game</name><setname>altgame</setname></misterromdescription>",
        )
        .expect("write alt mra");
        std::fs::write(
            media_dir.join("Fake Screenshot.mra"),
            "<misterromdescription><name>Fake Screenshot</name></misterromdescription>",
        )
        .expect("write media fake");
        std::fs::write(cores_dir.join("Core.rbf"), "core").expect("write rbf");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.normal_files.len(), 2);
        let titles = scan
            .discoveries
            .iter()
            .map(|discovery| discovery.title.as_str())
            .collect::<Vec<_>>();
        assert!(titles.contains(&"Real Game"));
        assert!(titles.contains(&"Alt Game"));
        assert!(!titles.contains(&"Fake Screenshot"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn scanner_does_not_follow_symlinked_game_dirs() {
        let root = unique_temp_dir("ignore-symlinked-game-dir");
        let outside = unique_temp_dir("symlink-target-games");
        let games_dir = root.join("games");
        let linked_nes = games_dir.join("NES");
        std::fs::create_dir_all(&games_dir).expect("create games dir");
        std::fs::create_dir_all(&outside).expect("create symlink target");
        std::fs::write(outside.join("Mario.nes"), "rom").expect("write outside rom");
        std::os::unix::fs::symlink(&outside, &linked_nes).expect("create linked game dir");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert!(scan.normal_files.is_empty());
        assert!(scan.discoveries.is_empty());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[cfg(unix)]
    #[test]
    fn scanner_does_not_follow_symlinked_game_files() {
        let root = unique_temp_dir("ignore-symlinked-game-file");
        let outside = unique_temp_dir("symlink-target-file");
        let nes_dir = root.join("games/NES");
        std::fs::create_dir_all(&nes_dir).expect("create nes dir");
        std::fs::create_dir_all(&outside).expect("create symlink target dir");
        let outside_rom = outside.join("Mario.nes");
        std::fs::write(&outside_rom, "rom").expect("write outside rom");
        std::os::unix::fs::symlink(&outside_rom, nes_dir.join("Mario.nes"))
            .expect("create linked game file");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert!(scan.normal_files.is_empty());
        assert!(scan.discoveries.is_empty());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[cfg(unix)]
    #[test]
    fn collection_listing_helper_times_out() {
        use std::os::unix::fs::PermissionsExt;

        let root = unique_temp_dir("collection-listing-timeout");
        let helper = root.join("slow-7za.sh");
        std::fs::write(&helper, "#!/bin/sh\nsleep 2\n").expect("write helper");
        let mut permissions = std::fs::metadata(&helper)
            .expect("stat helper")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&helper, permissions).expect("chmod helper");
        let archive = root.join("AmigaVision.7z");
        std::fs::write(&archive, "fixture").expect("write archive fixture");
        let file = FoundFile {
            path: archive,
            ext: "7z".to_string(),
            size: 7,
            mtime_secs: 0,
        };
        let listing = CollectionListing {
            entry_path: "listings/games.txt",
            genre: "AmigaVision",
        };
        let start = Instant::now();

        let text =
            collection_listing_text_with_tool(&file, &listing, &helper, Duration::from_millis(75));

        assert!(text.is_none());
        assert!(start.elapsed() < Duration::from_secs(1));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_ignores_organized_alias_dirs() {
        let root = unique_temp_dir("ignore-organized-aliases");
        let arcade_dir = root.join("_Arcade");
        let organized_dir = arcade_dir.join("_Organized/_1 A-E");
        std::fs::create_dir_all(&organized_dir).expect("create organized dir");
        std::fs::write(
            arcade_dir.join("Diamond Run.mra"),
            "<misterromdescription><name>Diamond Run</name><setname>diamond</setname></misterromdescription>",
        )
        .expect("write source mra");
        std::fs::write(
            organized_dir.join("Diamond Run.mra"),
            "<misterromdescription><name>Diamond Run Alias</name><setname>diamond-alias</setname></misterromdescription>",
        )
        .expect("write organized alias mra");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: root.join("library.sqlite3"),
        };

        let scan = scan_library(&cfg);

        assert_eq!(scan.normal_files.len(), 1);
        assert_eq!(scan.discoveries.len(), 1);
        assert_eq!(scan.discoveries[0].title, "Diamond Run");
        assert_eq!(
            scan.normal_files[0].path,
            arcade_dir.join("Diamond Run.mra").display().to_string()
        );
        assert!(scan
            .discoveries
            .iter()
            .all(|discovery| !discovery.launch_ref.contains("_Organized")));
        let _ = std::fs::remove_dir_all(root);
    }

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

    fn chd_v5_header(sha1: [u8; 20]) -> [u8; 124] {
        let mut header = [0u8; 124];
        header[..8].copy_from_slice(b"MComprHD");
        header[8..12].copy_from_slice(&124u32.to_be_bytes());
        header[12..16].copy_from_slice(&5u32.to_be_bytes());
        header[56..60].copy_from_slice(&4096u32.to_be_bytes());
        header[60..64].copy_from_slice(&2448u32.to_be_bytes());
        header[64..84].copy_from_slice(&sha1);
        header
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
    fn amigavision_listing_entries_generate_visible_collection_games() {
        let root = unique_temp_dir("amigavision-listing");
        let db = root.join("library.sqlite3");
        let profiles = launch_profiles::builtin_profiles();
        let profile = profiles
            .iter()
            .find(|profile| profile.id == "amiga")
            .expect("amiga profile");
        let listing = profile.collection_rules[0].listings[0];
        let file = FoundFile {
            path: PathBuf::from("/media/fat/games/Amiga/AmigaVision-MiSTer.7z"),
            ext: "7z".to_string(),
            size: 5_208_842_481,
            mtime_secs: 1,
        };
        let discoveries = collection_discoveries_from_listing_text(
            &file,
            profile,
            &listing,
            "Agony\nAlien Breed\n",
        );

        assert_eq!(unique_discovery_count(&discoveries), 2);
        assert!(discoveries.iter().all(|discovery| discovery
            .launch_ref
            .starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX)));
        save_sqlite_scan(&db, &sqlite_scan_with_discoveries(discoveries)).expect("save sqlite");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");

        assert_eq!(loaded.rows, 2);
        assert!(loaded
            .catalog
            .games
            .iter()
            .all(|game| game.system_id.as_ref() == "amiga"));
        assert!(loaded
            .catalog
            .systems
            .iter()
            .any(|system| system.id == "amiga" && system.count == 2));
        assert!(loaded
            .catalog
            .games
            .iter()
            .all(|game| game.mra_path.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX)));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_collection_adds_launcher_entry() {
        let profiles = launch_profiles::builtin_profiles();
        let profile = profiles
            .iter()
            .find(|profile| profile.id == "amiga")
            .expect("amiga profile");
        let file = FoundFile {
            path: PathBuf::from("/media/fat/games/Amiga/AmigaVision-MiSTer.7z"),
            ext: "7z".to_string(),
            size: 5_208_842_481,
            mtime_secs: 1,
        };

        let discoveries =
            collection_discoveries_from_container(&file, profile, &profile.collection_rules[0]);

        assert!(discoveries.iter().any(|discovery| {
            discovery.title == "AmigaVision" && discovery.launch_ref == AMIGAVISION_LAUNCHER_REF
        }));
    }

    #[test]
    fn installed_amigavision_hdf_uses_launcher_and_listings() {
        let root = unique_temp_dir("amigavision-installed");
        let amiga_dir = root.join("games/Amiga");
        let listings_dir = amiga_dir.join("listings");
        std::fs::create_dir_all(&listings_dir).expect("create listings dir");
        std::fs::write(amiga_dir.join("AmigaVision.hdf"), "hdf").expect("write hdf");
        std::fs::write(amiga_dir.join("AmigaVision-Saves.hdf"), "saves").expect("write saves");
        std::fs::write(
            listings_dir.join("games.txt"),
            b"Agony (OCS)[en]\nAlien Breed (OCS)[en]\nInvalid \xff Title (OCS)[en]\n",
        )
        .expect("write games listing");
        std::fs::write(
            listings_dir.join("demos.txt"),
            "State of the Art (OCS)[demo]\n",
        )
        .expect("write demos listing");
        let db = root.join("library.sqlite3");
        let cfg = BenchConfig {
            roots: vec![root.display().to_string()],
            sqlite_path: db.clone(),
        };

        let scan = scan_library(&cfg);

        assert!(scan.normal_files.is_empty());
        assert_eq!(scan.ignored_files, 2);
        assert!(scan.discoveries.iter().any(|discovery| {
            discovery.title == "AmigaVision" && discovery.launch_ref == AMIGAVISION_LAUNCHER_REF
        }));
        assert_eq!(
            scan.discoveries
                .iter()
                .filter(|discovery| discovery
                    .launch_ref
                    .starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX))
                .count(),
            4
        );
        assert!(scan
            .discoveries
            .iter()
            .all(|discovery| !discovery.launch_ref.ends_with("AmigaVision.hdf")));

        save_sqlite_scan(&db, &scan).expect("save sqlite");
        let loaded =
            load_arcade_catalog_from_sqlite_at("/media/fat/_Arcade", &db).expect("load catalog");

        assert_eq!(loaded.rows, 5);
        assert!(loaded.catalog.games.iter().all(|game| {
            game.mra_path.as_ref() == AMIGAVISION_LAUNCHER_REF
                || game.mra_path.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX)
        }));
        assert!(loaded
            .catalog
            .systems
            .iter()
            .any(|system| system.id == "amiga" && system.count == 5));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_archive_itself_is_not_a_launch_ref() {
        assert!(!is_launcher_launch_ref(
            "/media/fat/games/Amiga/AmigaVision-MiSTer.7z"
        ));
        assert!(is_launcher_launch_ref(AMIGAVISION_LAUNCHER_REF));
        assert!(is_launcher_launch_ref(&media_metadata::amigavision_game_launch_ref(
            "4th & Inches (OCS)[en]"
        )));
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

    fn payload(path: &str) -> GameDiscovery {
        GameDiscovery {
            source_path: path.to_string(),
            launch_ref: path.to_string(),
            source_kind: DiscoverySourceKind::PayloadFile,
            title: title_from_path(path),
            category: "Unknown".to_string(),
            platform_id: "unknown".to_string(),
            core_id: "unknown".to_string(),
            hardware_id: "unknown".to_string(),
            manufacturer: None,
            genre: None,
            year: None,
            setname: None,
            parent: None,
            confidence: DiscoveryConfidence::PayloadPath,
        }
    }

    fn saturn_payload(path: &str) -> GameDiscovery {
        GameDiscovery {
            source_path: path.to_string(),
            launch_ref: path.to_string(),
            source_kind: DiscoverySourceKind::PayloadFile,
            title: title_from_path(path),
            category: "Console".to_string(),
            platform_id: "saturn".to_string(),
            core_id: "Saturn".to_string(),
            hardware_id: "saturn".to_string(),
            manufacturer: None,
            genre: None,
            year: None,
            setname: None,
            parent: None,
            confidence: DiscoveryConfidence::PayloadPath,
        }
    }

    fn catalog_row(title: &str, path: &str, setname: &str, parent: &str) -> CatalogRow {
        CatalogRow {
            game: ArcadeGameEntry {
                title: title.into(),
                mra_path: path.into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "arcade".into(),
            },
            source_kind: "mra".to_string(),
            setname: setname.to_string(),
            parent: parent.to_string(),
            family_key: None,
        }
    }

    fn catalog_launcher_row(title: &str, path: &str) -> CatalogRow {
        CatalogRow {
            game: ArcadeGameEntry {
                title: title.into(),
                mra_path: path.into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "unknown".into(),
            },
            source_kind: "mgl".to_string(),
            setname: String::new(),
            parent: String::new(),
            family_key: None,
        }
    }

    fn catalog_entry_row(title: &str, path: &str) -> CatalogRow {
        CatalogRow {
            game: ArcadeGameEntry {
                title: title.into(),
                mra_path: path.into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "amiga".into(),
            },
            source_kind: "catalog-entry".to_string(),
            setname: String::new(),
            parent: String::new(),
            family_key: None,
        }
    }

    fn write_stored_zip(path: &Path, entries: &[(&str, &[u8])]) {
        write_stored_zip_with_central_metadata(path, entries, &[], &[]);
    }

    fn write_stored_zip_with_central_metadata(
        path: &Path,
        entries: &[(&str, &[u8])],
        central_extra: &[u8],
        central_comment: &[u8],
    ) {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let local_offset = out.len() as u32;
            push_u32(&mut out, 0x0403_4b50);
            push_u16(&mut out, 20);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u16(&mut out, 0);
            push_u32(&mut out, 0);
            push_u32(&mut out, data.len() as u32);
            push_u32(&mut out, data.len() as u32);
            push_u16(&mut out, name.len() as u16);
            push_u16(&mut out, 0);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);

            push_u32(&mut central, 0x0201_4b50);
            push_u16(&mut central, 20);
            push_u16(&mut central, 20);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u32(&mut central, 0);
            push_u32(&mut central, data.len() as u32);
            push_u32(&mut central, data.len() as u32);
            push_u16(&mut central, name.len() as u16);
            push_u16(&mut central, central_extra.len() as u16);
            push_u16(&mut central, central_comment.len() as u16);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u32(&mut central, 0);
            push_u32(&mut central, local_offset);
            central.extend_from_slice(name.as_bytes());
            central.extend_from_slice(central_extra);
            central.extend_from_slice(central_comment);
        }
        let central_offset = out.len() as u32;
        let central_size = central.len() as u32;
        out.extend_from_slice(&central);
        let zip64_eocd_offset = out.len() as u64;
        push_u32(&mut out, 0x0606_4b50);
        push_u64(&mut out, 44);
        push_u16(&mut out, 45);
        push_u16(&mut out, 45);
        push_u32(&mut out, 0);
        push_u32(&mut out, 0);
        push_u64(&mut out, entries.len() as u64);
        push_u64(&mut out, entries.len() as u64);
        push_u64(&mut out, central_size as u64);
        push_u64(&mut out, central_offset as u64);
        push_u32(&mut out, 0x0706_4b50);
        push_u32(&mut out, 0);
        push_u64(&mut out, zip64_eocd_offset);
        push_u32(&mut out, 1);
        push_u32(&mut out, 0x0605_4b50);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, u16::MAX);
        push_u16(&mut out, u16::MAX);
        push_u32(&mut out, u32::MAX);
        push_u32(&mut out, u32::MAX);
        push_u16(&mut out, 0);
        std::fs::write(path, out).expect("write zip fixture");
    }

    fn write_mame_fixture_db(path: &Path, rows: &[MameMachineFixture<'_>]) {
        let conn = Connection::open(path).expect("open mame fixture");
        conn.execute_batch(
            r#"
            CREATE TABLE mame_machines (
                setname TEXT PRIMARY KEY,
                parent_setname TEXT,
                title TEXT NOT NULL,
                year TEXT,
                manufacturer TEXT
            ) WITHOUT ROWID;
            "#,
        )
        .expect("create mame fixture");
        let mut stmt = conn
            .prepare(
                "INSERT INTO mame_machines(setname,parent_setname,title,year,manufacturer)
                 VALUES (?1,?2,?3,?4,?5)",
            )
            .expect("prepare mame fixture insert");
        for (setname, parent, title, year, manufacturer) in rows {
            stmt.execute(params![setname, parent, title, year, manufacturer])
                .expect("insert mame fixture row");
        }
    }

    type SoftwareItemFixture<'a> = (
        &'a str,
        &'a str,
        Option<&'a str>,
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
    );

    fn write_mame_software_fixture_db(
        path: &Path,
        items: &[SoftwareItemFixture<'_>],
        hashes: &[(&str, &str, i64, u32)],
    ) {
        let conn = Connection::open(path).expect("open software fixture");
        conn.execute_batch(
            r#"
            CREATE TABLE mame_machines (
                setname TEXT PRIMARY KEY,
                parent_setname TEXT,
                title TEXT NOT NULL,
                year TEXT,
                manufacturer TEXT
            ) WITHOUT ROWID;
            CREATE TABLE mame_software_items (
                list_name TEXT NOT NULL,
                software_name TEXT NOT NULL,
                parent_name TEXT,
                description TEXT NOT NULL,
                year TEXT,
                publisher TEXT,
                region TEXT,
                source_version TEXT NOT NULL,
                PRIMARY KEY(list_name, software_name)
            ) WITHOUT ROWID;
            CREATE TABLE mame_software_hashes (
                list_name TEXT NOT NULL,
                software_name TEXT NOT NULL,
                part_name TEXT,
                rom_name TEXT,
                size INTEGER,
                crc32 TEXT,
                sha1 TEXT,
                data_area TEXT,
                disk_sha1 TEXT
            );
            "#,
        )
        .expect("create software fixture");
        let mut item_stmt = conn
            .prepare(
                "INSERT INTO mame_software_items(
                    list_name,software_name,parent_name,description,year,publisher,region,source_version
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,'fixture')",
            )
            .expect("prepare software fixture item insert");
        for (list, name, parent, description, year, publisher, region) in items {
            item_stmt
                .execute(params![
                    list,
                    name,
                    parent,
                    description,
                    year,
                    publisher,
                    region
                ])
                .expect("insert software fixture item");
        }
        let mut hash_stmt = conn
            .prepare(
                "INSERT INTO mame_software_hashes(list_name,software_name,size,crc32)
                 VALUES (?1,?2,?3,?4)",
            )
            .expect("prepare software fixture hash insert");
        for (list, name, size, crc) in hashes {
            hash_stmt
                .execute(params![list, name, size, format!("{crc:08x}")])
                .expect("insert software fixture hash");
        }
    }

    fn write_software_hash_cache_fixture(
        path: &Path,
        rows: &[(&str, &str, u64, i64, Option<&str>)],
    ) {
        let conn = Connection::open(path).expect("open software hash cache fixture");
        conn.execute_batch(
            r#"
            CREATE TABLE software_hash_cache (
                list_name TEXT NOT NULL,
                file_path TEXT NOT NULL,
                size INTEGER NOT NULL,
                mtime_secs INTEGER NOT NULL,
                software_name TEXT,
                PRIMARY KEY(list_name, file_path, size, mtime_secs)
            ) WITHOUT ROWID;
            "#,
        )
        .expect("create software hash cache fixture");
        let mut stmt = conn
            .prepare(
                "INSERT INTO software_hash_cache(list_name,file_path,size,mtime_secs,software_name)
                 VALUES (?1,?2,?3,?4,?5)",
            )
            .expect("prepare software hash cache fixture insert");
        for (list, path, size, mtime, software_name) in rows {
            stmt.execute(params![list, path, *size as i64, mtime, software_name])
                .expect("insert software hash cache fixture");
        }
    }

    fn push_u16(out: &mut Vec<u8>, value: u16) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u64(out: &mut Vec<u8>, value: u64) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn mgl(source_path: &str, launch_ref: &str) -> GameDiscovery {
        GameDiscovery {
            source_path: source_path.to_string(),
            launch_ref: launch_ref.to_string(),
            source_kind: DiscoverySourceKind::Mgl,
            title: title_from_path(source_path),
            category: "Unknown".to_string(),
            platform_id: "unknown".to_string(),
            core_id: "unknown".to_string(),
            hardware_id: "unknown".to_string(),
            manufacturer: None,
            genre: None,
            year: None,
            setname: None,
            parent: None,
            confidence: DiscoveryConfidence::PayloadPath,
        }
    }

    fn mra_discovery(idx: usize, title: &str) -> GameDiscovery {
        let path = format!("/media/fat/_Arcade/{title}.mra");
        GameDiscovery {
            source_path: path.clone(),
            launch_ref: path,
            source_kind: DiscoverySourceKind::Mra,
            title: title.to_string(),
            category: "Arcade".to_string(),
            platform_id: "arcade".to_string(),
            core_id: "arcade".to_string(),
            hardware_id: "arcade-unknown".to_string(),
            manufacturer: None,
            genre: None,
            year: None,
            setname: Some(format!("game{idx:05}")),
            parent: None,
            confidence: DiscoveryConfidence::MraCore,
        }
    }

    fn sqlite_scan_with_normal_files(paths: &[&str]) -> LibraryScan {
        LibraryScan {
            version: SCHEMA_VERSION,
            scanned_at_unix: 1,
            normal_files: paths
                .iter()
                .map(|path| LibraryPayloadFile {
                    path: path.to_string(),
                    profile_id: "mgl".to_string(),
                    size: 0,
                    mtime_secs: 0,
                    rule: PayloadRule {
                        extensions: &["mgl"],
                        mount: launch_profiles::MountSpec::launcher(),
                        disposition: PayloadDisposition::Playable,
                        provenance: launch_profiles::RuleProvenance::mgl(
                            "test fixture launcher payload",
                        ),
                    },
                })
                .collect(),
            containers: Vec::new(),
            entries: Vec::new(),
            ignored_files: 0,
            discoveries: Vec::new(),
            discover_us: 0,
            classify_us: 0,
        }
    }

    fn sqlite_scan_with_discoveries(discoveries: Vec<GameDiscovery>) -> LibraryScan {
        LibraryScan {
            version: SCHEMA_VERSION,
            scanned_at_unix: 1,
            normal_files: Vec::new(),
            containers: Vec::new(),
            entries: Vec::new(),
            ignored_files: 0,
            discoveries,
            discover_us: 0,
            classify_us: 0,
        }
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-{label}-{}-{}",
            std::process::id(),
            unix_now_secs()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    #[cfg(unix)]
    fn set_file_mtime_for_test(path: &Path, sec: i64, nsec: i64) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let c_path = CString::new(path.as_os_str().as_bytes()).expect("path cstring");
        let times = [
            libc::timespec {
                tv_sec: sec as libc::time_t,
                tv_nsec: nsec as libc::c_long,
            },
            libc::timespec {
                tv_sec: sec as libc::time_t,
                tv_nsec: nsec as libc::c_long,
            },
        ];
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(rc, 0, "utimensat failed for {}", path.display());
    }
}
