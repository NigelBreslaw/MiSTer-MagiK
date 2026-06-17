//! Whole-MiSTer library database scanning and loading.
//!
//! This is deliberately TOC/header-only for archives. Indexing must never
//! decompress full game libraries just to make the launcher searchable.

use crate::arcade_catalog::{self, ArcadeCatalog, ArcadeGameEntry};
use crate::launch_profiles::{
    self, CollectionListing, CollectionRule, IgnoreReason, LaunchProfile, MountKind,
    PayloadDisposition, PayloadRule, ProfilePathClass, RuleProvenance, RuleSourceKind,
};
use crate::preview_worker;
use rusqlite::{params, Connection, OpenFlags};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Instant;

const DEFAULT_ROOTS: &[&str] = &[
    "/media/fat/_Arcade",
    "/media/fat/_Games",
    "/media/fat/games",
    "/media/fat/_DOS Games",
    "/media/fat/_Console",
    "/media/fat/_Computer",
    "/media/fat/_YCArcade",
    "/media/fat/_YCConsole",
    "/media/fat/_YCComputer",
    "/media/fat/_LLAPI",
    "/media/fat/_Other",
    "/media/fat/_Utility",
];

pub const DEFAULT_SQLITE_PATH: &str = "/media/fat/mister-magik/library.sqlite3";
pub const DEFAULT_MAME_SQLITE_PATH: &str = "/media/fat/mister-magik/mame.sqlite3";
pub const DEFAULT_HBMAME_SQLITE_PATH: &str = "/media/fat/mister-magik/hbmame.sqlite3";

const MRA_PREFIX_BYTES: usize = 160 * 1024;
type FileFingerprint = BTreeMap<String, (u64, i64)>;
type ProgressCallback<'a> = Option<&'a mut dyn FnMut(&str, &str)>;
type DirectoryManifest = BTreeMap<String, DirectorySignature>;

const SCHEMA_VERSION: u32 = 25;
const MANIFEST_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const MANIFEST_HASH_PRIME: u64 = 0x0000_0100_0000_01b3;
const AMIGAVISION_GAME_LAUNCH_PREFIX: &str = "magik-amigavision:";
const AMIGAVISION_LAUNCHER_REF: &str = "magik-amigavision-launcher";
const AMIGAVISION_INSTALLED_LISTINGS: &[CollectionListing] = &[
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
    fn from_ext(ext: &str) -> Option<Self> {
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
    file_fingerprints: FileFingerprint,
    directory_manifest: DirectoryManifest,
    normal_files: Vec<LibraryPayloadFile>,
    containers: Vec<LibraryContainer>,
    entries: Vec<LibraryContainerEntry>,
    ignored_files: Vec<LibraryIgnoredFile>,
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

pub struct HbmameMetadataSummary {
    pub path: PathBuf,
    pub rows: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtualLaunchPlan {
    pub launch_ref: String,
    pub title: String,
    pub core_path: String,
    pub payload_path: String,
    pub mount_kind: String,
    pub mount_index: u8,
    pub mount_delay_secs: u8,
}

#[derive(Clone, Debug, PartialEq)]
struct DbFingerprint {
    normal_files: usize,
    containers: usize,
    entries: usize,
    discoveries: usize,
    mame_metadata: FileSignature,
    hbmame_metadata: FileSignature,
    file_fingerprints: FileFingerprint,
    container_fingerprints: BTreeMap<String, (u64, i64)>,
    directory_manifest: DirectoryManifest,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct FileSignature {
    size: u64,
    mtime_secs: i64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SoftwareHashCacheKey {
    list_name: String,
    file_path: String,
    size: u64,
    mtime_secs: i64,
}

#[derive(Clone, Debug, Default)]
struct SoftwareHashCache {
    entries: HashMap<SoftwareHashCacheKey, Option<String>>,
}

#[derive(Clone, Debug, Default)]
struct MameMachineMetadata {
    parent_setname: Option<String>,
    title: String,
    year: Option<String>,
    manufacturer: Option<String>,
}

#[derive(Default)]
struct ArcadeMachineMetadata {
    mame: HashMap<String, MameMachineMetadata>,
    hbmame: HashMap<String, MameMachineMetadata>,
}

#[derive(Clone, Debug, Default)]
struct MameSoftwareItemMetadata {
    parent_name: Option<String>,
    description: String,
    year: Option<String>,
    publisher: Option<String>,
    region: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct MameSoftwareMetadata {
    items: HashMap<(String, String), MameSoftwareItemMetadata>,
    hash_index: HashMap<(String, u64, u32), Vec<String>>,
    disk_index: HashMap<(String, String), Vec<String>>,
    title_index: HashMap<(String, String), Vec<String>>,
    family_members: HashMap<(String, String), Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SoftwareIdentity {
    list_name: String,
    software_name: String,
    family_id: String,
    metadata_title: Option<String>,
    year: Option<String>,
    manufacturer: Option<String>,
    region: Option<String>,
    source: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DirectorySignature {
    dir_size: u64,
    dir_mtime_secs: i64,
    child_count: u64,
    hash: u64,
}

#[derive(Clone, Debug)]
struct GameDiscovery {
    source_path: String,
    launch_ref: String,
    source_kind: DiscoverySourceKind,
    title: String,
    category: String,
    platform_id: String,
    core_id: String,
    hardware_id: String,
    manufacturer: Option<String>,
    genre: Option<String>,
    year: Option<u16>,
    setname: Option<String>,
    parent: Option<String>,
    image_path: Option<String>,
    has_image: bool,
    confidence: DiscoveryConfidence,
}

#[derive(Clone, Debug)]
struct LibraryIgnoredFile {
    path: String,
    profile_id: String,
    reason: IgnoreReason,
    provenance: RuleProvenance,
}

#[derive(Clone, Debug)]
struct LibraryPayloadFile {
    path: String,
    profile_id: String,
    rule: PayloadRule,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiscoverySourceKind {
    Mra,
    Mgl,
    PayloadFile,
    ArchiveEntry,
    CatalogEntry,
}

#[derive(Clone, Copy, Debug)]
enum DiscoveryConfidence {
    MraHardware,
    MraCore,
    PayloadPath,
    Extension,
    ArchiveToc,
    CatalogMetadata,
}

#[derive(Clone)]
struct FoundFile {
    path: PathBuf,
    ext: String,
    size: u64,
    mtime_secs: i64,
}

struct ArchiveScan {
    container: LibraryContainer,
    entries: Vec<LibraryContainerEntry>,
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
    let bench_changed_refresh = env_bool("MISTER_LIBRARY_BENCH_CHANGED_REFRESH");
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
            let (candidates, dirs, precount_us) = precount_discovery_candidates(&cfg.roots);
            println!(
                "library_scan_bench_tsv\t{label}\t{iteration}\tprecount_discovery\t{precount_us}\tcandidates={candidates}\tdirs={dirs}"
            );
        }

        let cold_t = Instant::now();
        let scan = scan_library(&cfg);
        let cold_us = cold_t.elapsed().as_micros() as u64;

        let import_t = Instant::now();
        let bytes = match save_sqlite_scan(&cfg.sqlite_path, &scan) {
            Ok(bytes) => bytes,
            Err(e) => {
                println!(
                    "library_scan_bench_tsv\t{label}\t{iteration}\timport_error\t{}\t{e}",
                    import_t.elapsed().as_micros()
                );
                continue;
            }
        };
        let import_us = import_t.elapsed().as_micros() as u64;

        let load_t = Instant::now();
        let loaded = load_arcade_catalog_from_sqlite("/media/fat/_Arcade");
        let (load_us, arcade_rows) = match loaded {
            Ok(load) => (load.us, load.rows),
            Err(e) => {
                eprintln!("library-scan-bench arcade load failed: {e}");
                (load_t.elapsed().as_micros() as u64, 0)
            }
        };

        let manifest_t = Instant::now();
        let manifest_changed = read_sqlite_fingerprint(&cfg.sqlite_path)
            .and_then(|fingerprint| {
                validate_or_rebuild_directory_manifest(&cfg.roots, &fingerprint)
                    .map(|current| current != fingerprint.directory_manifest)
            })
            .unwrap_or(true);
        let manifest_us = manifest_t.elapsed().as_micros() as u64;

        let changed_refresh = if bench_changed_refresh {
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
                    "library-scan-bench changed refresh setup failed at {}: {e}",
                    change_path.display()
                );
            }
            let changed_refresh_t = Instant::now();
            let summary = refresh_sqlite_database(&cfg, None);
            Some((changed_refresh_t.elapsed().as_micros() as u64, summary))
        } else {
            None
        };

        println!(
            "library_scan_bench_tsv\t{label}\t{iteration}\tcold_scan\t{cold_us}\tdiscover_us={}\tclassify_us={}\tnormal_files={}\tcontainers={}\tentries={}\tdiscoveries={}\tdirs={}",
            scan.discover_us,
            scan.classify_us,
            scan.normal_files.len(),
            scan.containers.len(),
            scan.entries.len(),
            unique_discovery_count(&scan.discoveries),
            scan.directory_manifest.len()
        );
        println!(
            "library_scan_bench_tsv\t{label}\t{iteration}\timport\t{import_us}\tbytes={bytes}"
        );
        println!(
            "library_scan_bench_tsv\t{label}\t{iteration}\tcached_arcade_load\t{load_us}\trows={arcade_rows}"
        );
        println!(
            "library_scan_bench_tsv\t{label}\t{iteration}\tno_change_manifest\t{manifest_us}\tchanged={manifest_changed}\tdirs={}",
            scan.directory_manifest.len()
        );
        if let Some((changed_refresh_us, changed_summary)) = changed_refresh {
            match changed_summary {
                Ok(summary) => println!(
                    "library_scan_bench_tsv\t{label}\t{iteration}\tchanged_refresh\t{changed_refresh_us}\tscan_us={}\tdiscover_us={}\tclassify_us={}\timport_us={}\tskipped={}\tdiscoveries={}",
                    summary.scan_us,
                    summary.discover_us,
                    summary.classify_us,
                    summary.import_us,
                    summary.skipped,
                    summary.discoveries
                ),
                Err(e) => println!(
                    "library_scan_bench_tsv\t{label}\t{iteration}\tchanged_refresh_error\t{changed_refresh_us}\t{e}"
                ),
            }
        }
    }
}

pub fn default_sqlite_path() -> PathBuf {
    std::env::var("MISTER_LIBRARY_SQLITE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_SQLITE_PATH))
}

pub fn default_mame_sqlite_path() -> PathBuf {
    std::env::var("MISTER_MAME_SQLITE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_MAME_SQLITE_PATH))
}

pub fn default_hbmame_sqlite_path() -> PathBuf {
    std::env::var("MISTER_HBMAME_SQLITE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_HBMAME_SQLITE_PATH))
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

    let metadata =
        std::fs::metadata(&path).map_err(|e| format!("stat {}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    if metadata.len() == 0 {
        return Err(format!("{} is empty", path.display()));
    }

    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
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
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open library db: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    let mut stmt = conn
        .prepare(
            "SELECT launch_plans.launch_ref,
                    games.title,
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
        core_path: row
            .get::<_, String>(2)
            .map_err(|e| format!("read core_path: {e}"))?,
        payload_path: row
            .get::<_, String>(3)
            .map_err(|e| format!("read payload_path: {e}"))?,
        mount_kind: row
            .get::<_, String>(4)
            .map_err(|e| format!("read mount_kind: {e}"))?,
        mount_index: row
            .get::<_, i64>(5)
            .map_err(|e| format!("read mount_index: {e}"))?
            .clamp(0, u8::MAX as i64) as u8,
        mount_delay_secs: row
            .get::<_, i64>(6)
            .map_err(|e| format!("read mount_delay_secs: {e}"))?
            .clamp(0, u8::MAX as i64) as u8,
    }))
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
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open library db: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
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
                image_path,
                has_image,
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
                    image_path,
                    has_image,
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
                image_path,
                has_image,
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
                image_path: row.get::<_, String>(2)?.into(),
                has_image: row.get::<_, i64>(3)? != 0,
                system_id: row.get::<_, String>(4)?.into(),
            })
        })
        .map_err(|e| format!("query {label}: {e}"))?;
    let mut games = Vec::new();
    for row in rows {
        games.push(row.map_err(|e| format!("read {label} row: {e}"))?);
    }
    Ok(games)
}

fn sqlite_table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|exists| exists != 0)
    .map_err(|e| format!("check sqlite table {table}: {e}"))
}

fn load_joined_launcher_catalog(conn: &Connection) -> Result<Vec<ArcadeGameEntry>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT games.title,
                    launch_plans.launch_ref,
                    COALESCE(games.image_path,''),
                    games.has_image,
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
                    image_path: row.get::<_, String>(2)?.into(),
                    has_image: row.get::<_, i64>(3)? != 0,
                    system_id: row.get::<_, String>(4)?.into(),
                },
                source_kind: row.get::<_, String>(5)?,
                setname: row.get::<_, String>(6)?,
                parent: row.get::<_, String>(7)?,
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
    if a.game.has_image != b.game.has_image {
        return a.game.has_image;
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

fn variant_score_from_haystack(haystack: &str) -> i32 {
    let mut score = 0;
    if contains_any(
        haystack,
        &[
            "(usa", "(us,", "(us)", "(u)", "/_usa/", " america", "american",
        ],
    ) {
        score += 1000;
    } else if contains_any(haystack, &["(japan", "(jp", "(j)", "/_japan/"]) {
        score += 900;
    } else if contains_any(haystack, &["(world", "(w,", "(w)", "/_world/"]) {
        score += 800;
    } else if contains_any(haystack, &["(europe", "(eu", "(e)", "/_europe/"]) {
        score += 700;
    }

    for bad in [
        "prototype",
        "bootleg",
        "[hack",
        " hack",
        "hbmame",
        "homebrew",
        "[hb]",
        "training",
        "unlocked",
        "free play",
        "low lag",
        "fix",
        "patched",
        "beta",
        "sample",
    ] {
        if haystack.contains(bad) {
            score -= 300;
        }
    }

    score
}

fn canonical_variant_title(title: &str) -> String {
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

pub fn refresh_default_sqlite_database(
    progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    let cfg = BenchConfig::production();
    refresh_sqlite_database(&cfg, progress)
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
    let conn = Connection::open_with_flags(library_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
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
    let mut machines: BTreeMap<String, (String, String, Option<String>, Option<String>)> =
        BTreeMap::new();
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

pub fn default_sqlite_preview_archive_fingerprint_unchanged() -> bool {
    let cfg = BenchConfig::production();
    read_sqlite_fingerprint(&cfg.sqlite_path)
        .as_ref()
        .is_some_and(preview_archive_fingerprint_unchanged)
}

fn refresh_sqlite_database(
    cfg: &BenchConfig,
    mut progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    let scan_t = Instant::now();
    if let Some(existing) = read_sqlite_fingerprint(&cfg.sqlite_path) {
        let preview_archive_unchanged = preview_archive_fingerprint_unchanged(&existing);
        let metadata_unchanged = metadata_fingerprints_unchanged(&existing);
        if should_refresh_preview_assets_before_manifest(
            preview_archive_unchanged,
            metadata_unchanged,
        ) {
            if let Some(report) = progress.as_mut() {
                report(
                    "Preview images changed",
                    "Updating screenshot availability without rescanning games...",
                );
            }
            let scan_us = scan_t.elapsed().as_micros() as u64;
            let import_t = Instant::now();
            if let Ok(bytes) = refresh_sqlite_preview_assets_from_env(&cfg.sqlite_path) {
                return Ok(LibraryRefreshSummary {
                    skipped: true,
                    scan_us,
                    discover_us: 0,
                    classify_us: 0,
                    import_us: import_t.elapsed().as_micros() as u64,
                    bytes,
                    normal_files: existing.normal_files,
                    containers: existing.containers,
                    entries: existing.entries,
                    discoveries: existing.discoveries,
                });
            }
            if let Some(report) = progress.as_mut() {
                report(
                    "Library changed",
                    "Preview metadata update failed; checking library before rebuild...",
                );
            }
        }
        if let Some(report) = progress.as_mut() {
            report("Checking library", "Looking for changed files...");
        }
        let current_manifest = validate_or_rebuild_directory_manifest(&cfg.roots, &existing);
        let scan_us = scan_t.elapsed().as_micros() as u64;
        match library_refresh_plan(
            &existing,
            current_manifest.as_ref(),
            preview_archive_unchanged,
            metadata_unchanged,
        ) {
            LibraryRefreshPlan::UseCachedDatabase => {
                if let Some(report) = progress.as_mut() {
                    report(
                        "Library unchanged",
                        &format!(
                            "{} directories checked; using cached database",
                            existing.directory_manifest.len()
                        ),
                    );
                }
                let bytes = std::fs::metadata(&cfg.sqlite_path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                return Ok(LibraryRefreshSummary {
                    skipped: true,
                    scan_us,
                    discover_us: 0,
                    classify_us: 0,
                    import_us: 0,
                    bytes,
                    normal_files: existing.normal_files,
                    containers: existing.containers,
                    entries: existing.entries,
                    discoveries: existing.discoveries,
                });
            }
            LibraryRefreshPlan::RefreshPreviewAssets => {
                if let Some(report) = progress.as_mut() {
                    report(
                        "Preview images changed",
                        "Updating screenshot availability without rescanning games...",
                    );
                }
                let import_t = Instant::now();
                if let Ok(bytes) = refresh_sqlite_preview_assets_from_env(&cfg.sqlite_path) {
                    return Ok(LibraryRefreshSummary {
                        skipped: true,
                        scan_us,
                        discover_us: 0,
                        classify_us: 0,
                        import_us: import_t.elapsed().as_micros() as u64,
                        bytes,
                        normal_files: existing.normal_files,
                        containers: existing.containers,
                        entries: existing.entries,
                        discoveries: existing.discoveries,
                    });
                }
                if let Some(report) = progress.as_mut() {
                    report(
                        "Library changed",
                        "Preview metadata update failed; rebuilding database...",
                    );
                }
            }
            LibraryRefreshPlan::RebuildDatabase => {}
        }
        if let Some(report) = progress.as_mut() {
            report(
                "Library changed",
                "Catalog metadata changed; rebuilding database...",
            );
        }
    } else {
        if let Some(report) = progress.as_mut() {
            report(
                "Indexing library",
                "No usable database fingerprint; full scan...",
            );
        }
    }

    let artifact = match progress.as_mut() {
        Some(report) => scan_library_artifact(&cfg, Some(&mut **report)),
        None => scan_library_artifact(&cfg, None),
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
    let mut summary = save_scan_artifact_to_sqlite(&cfg, artifact, progress)?;
    summary.scan_us = scan_us;
    Ok(summary)
}

fn should_refresh_preview_assets_before_manifest(
    preview_archive_unchanged: bool,
    metadata_unchanged: bool,
) -> bool {
    !preview_archive_unchanged && metadata_unchanged
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LibraryRefreshPlan {
    UseCachedDatabase,
    RefreshPreviewAssets,
    RebuildDatabase,
}

fn library_refresh_plan(
    existing: &DbFingerprint,
    current_manifest: Option<&DirectoryManifest>,
    preview_archive_unchanged: bool,
    metadata_unchanged: bool,
) -> LibraryRefreshPlan {
    if current_manifest != Some(&existing.directory_manifest) {
        return LibraryRefreshPlan::RebuildDatabase;
    }
    if !metadata_unchanged {
        return LibraryRefreshPlan::RebuildDatabase;
    }
    if preview_archive_unchanged {
        LibraryRefreshPlan::UseCachedDatabase
    } else {
        LibraryRefreshPlan::RefreshPreviewAssets
    }
}

struct BenchConfig {
    roots: Vec<String>,
    sqlite_path: PathBuf,
}

impl BenchConfig {
    fn from_env() -> Self {
        let roots = std::env::var("MISTER_LIBRARY_ROOTS")
            .ok()
            .map(|s| {
                s.split('|')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_else(|| DEFAULT_ROOTS.iter().map(|s| s.to_string()).collect());
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

fn env_bool(name: &str) -> bool {
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
    LibraryScanArtifact { scan, stats }
}

fn save_scan_artifact_to_sqlite(
    cfg: &BenchConfig,
    artifact: LibraryScanArtifact,
    progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    let import_t = Instant::now();
    let bytes = save_sqlite_scan_with_progress(&cfg.sqlite_path, &artifact.scan, progress)?;
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
                image_path: discovery.image_path.clone().unwrap_or_default().into(),
                has_image: discovery.has_image,
                system_id: system_id.into(),
            },
            source_kind: launch_kind_for_discovery(discovery).to_string(),
            setname,
            parent,
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
        let (family_id, _, _, _, _) = mame_identity_projection(&identity_id, arcade_metadata);
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

fn precount_discovery_candidates(roots: &[String]) -> (usize, usize, u64) {
    let started = Instant::now();
    let rx = discover_files_pipelined(roots.to_vec());
    let mut candidates = 0usize;
    let mut dirs = 0usize;
    while let Ok(event) = rx.recv() {
        match event {
            DiscoveryEvent::File(_) => candidates += 1,
            DiscoveryEvent::Done { manifest, .. } => {
                dirs = manifest.len();
                break;
            }
        }
    }
    (candidates, dirs, started.elapsed().as_micros() as u64)
}

fn scan_library_with_progress(
    cfg: &BenchConfig,
    mut progress: ProgressCallback<'_>,
) -> LibraryScan {
    let discover_t = Instant::now();
    let rx = discover_files_pipelined(cfg.roots.clone());
    let profiles = launch_profiles::builtin_profiles();
    let preview_images = PreviewImageIndex::from_env();
    let mut discover_us = 0;
    let mut file_fingerprints = FileFingerprint::new();
    let mut directory_manifest = DirectoryManifest::new();
    if let Some(report) = progress.as_mut() {
        report(
            "Classifying library",
            "Walking candidates and parsing metadata...",
        );
    }

    let mut normal_files = Vec::new();
    let mut containers = Vec::new();
    let mut entries = Vec::new();
    let mut ignored_files = Vec::new();
    let mut discoveries = Vec::new();
    let classify_t = Instant::now();
    let mut idx = 0usize;
    while let Ok(event) = rx.recv() {
        let f = match event {
            DiscoveryEvent::File(file) => file,
            DiscoveryEvent::Done {
                manifest,
                discover_us: us,
            } => {
                discover_us = us;
                directory_manifest = manifest;
                break;
            }
        };
        file_fingerprints.insert(f.path.display().to_string(), (f.size, f.mtime_secs));
        if idx.is_multiple_of(250) {
            if let Some(report) = progress.as_mut() {
                report(
                    "Classifying library",
                    &format!("Games found: {}", discoveries.len()),
                );
            }
        }
        idx += 1;
        match classify_profile_path(&profiles, &f.path) {
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
                if is_amigavision_save_media_path(&f.path) {
                    ignored_files.push(LibraryIgnoredFile {
                        path: f.path.display().to_string(),
                        profile_id: profile.id.to_string(),
                        reason: IgnoreReason::SaveMedia,
                        provenance: RuleProvenance::magik(
                            "AmigaVision-Saves.hdf is save/support media for the AmigaVision launcher environment",
                        ),
                    });
                    continue;
                }
                if let Some(installed) = installed_amigavision_discoveries_from_hdf(&f, profile) {
                    ignored_files.push(LibraryIgnoredFile {
                        path: f.path.display().to_string(),
                        profile_id: profile.id.to_string(),
                        reason: IgnoreReason::SupportArchive,
                        provenance: RuleProvenance::magik(
                            "AmigaVision.hdf is the launcher environment backing _Computer/Amiga.mgl, not a raw game payload",
                        ),
                    });
                    discoveries.extend(installed);
                    continue;
                }
                let mut has_archive_entries = false;
                if let Some(format) = ArchiveFormat::from_ext(&f.ext) {
                    let scan = scan_archive_toc(&f, format, profile);
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
                    rule: payload_rule,
                });
                let mut discovery =
                    discovery_from_profile_file(&f, profile, &payload_rule, &profiles);
                attach_preview_image(&mut discovery, &preview_images);
                discoveries.push(discovery);
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
                    rule: payload_rule,
                });
                ignored_files.push(LibraryIgnoredFile {
                    path: f.path.display().to_string(),
                    profile_id: profile.id.to_string(),
                    reason: IgnoreReason::SupportArchive,
                    provenance: RuleProvenance::magik(
                    "Attached media is indexed as payload support until a launcher references it",
                    ),
                });
            }
            Some((profile, ProfilePathClass::Collection { rule })) => {
                if let Some(format) = ArchiveFormat::from_ext(&f.ext) {
                    containers.push(scan_container_header(&f, format));
                }
                discoveries.extend(collection_discoveries_from_container(&f, profile, &rule));
            }
            Some((profile, ProfilePathClass::Ignored { reason, provenance })) => {
                ignored_files.push(LibraryIgnoredFile {
                    path: f.path.display().to_string(),
                    profile_id: profile.id.to_string(),
                    reason,
                    provenance,
                });
            }
            Some((_, ProfilePathClass::NotMatched)) | None => {}
        }
    }
    if let Ok(fingerprints) = preview_worker::preview_archive_fingerprints_from_env() {
        for (path, size, mtime_secs) in fingerprints {
            file_fingerprints.insert(path, (size, mtime_secs));
        }
    }
    if discover_us == 0 {
        discover_us = discover_t.elapsed().as_micros() as u64;
    }
    LibraryScan {
        version: SCHEMA_VERSION,
        scanned_at_unix: unix_now_secs(),
        file_fingerprints,
        directory_manifest,
        normal_files,
        containers,
        entries,
        ignored_files,
        discoveries,
        discover_us,
        classify_us: classify_t.elapsed().as_micros() as u64,
    }
}

fn classify_profile_path<'a>(
    profiles: &'a [LaunchProfile],
    path: &Path,
) -> Option<(&'a LaunchProfile, ProfilePathClass)> {
    let profile = profile_for_path(profiles, path)?;
    Some((profile, profile.classify_path(path)))
}

fn profile_for_path<'a>(profiles: &'a [LaunchProfile], path: &Path) -> Option<&'a LaunchProfile> {
    let mut previous_was_games = false;
    for component in path_components_str(path) {
        if previous_was_games {
            if let Some(profile) = launch_profiles::profile_for_game_dir(profiles, component) {
                return Some(profile);
            }
        }
        previous_was_games = component.eq_ignore_ascii_case("games");
    }

    profiles.iter().find(|profile| {
        path_components_str(path).any(|component| {
            profile
                .game_dirs
                .iter()
                .any(|dir| component.eq_ignore_ascii_case(dir))
        })
    })
}

fn path_components_str(path: &Path) -> impl Iterator<Item = &str> {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
}

fn validate_or_rebuild_directory_manifest(
    roots: &[String],
    existing: &DbFingerprint,
) -> Option<DirectoryManifest> {
    if existing.directory_manifest.is_empty() {
        return None;
    }
    for root in roots {
        if !existing.directory_manifest.contains_key(root) {
            return None;
        }
    }
    if directory_manifest_metadata_changed(&existing.directory_manifest) {
        return Some(DirectoryManifest::new());
    }
    // Directory metadata can miss same-second child edits on some filesystems,
    // so only use it as an early changed signal. The unchanged case still
    // rebuilds and compares child signatures before trusting the cached DB.
    let current = build_directory_manifest(roots, None);
    if current == existing.directory_manifest {
        Some(current)
    } else {
        Some(DirectoryManifest::new())
    }
}

fn preview_archive_fingerprint_unchanged(existing: &DbFingerprint) -> bool {
    match preview_worker::preview_archive_fingerprints_from_env() {
        Ok(current) => preview_archive_fingerprint_matches(existing, current),
        Err(_) => false,
    }
}

fn metadata_fingerprints_unchanged(existing: &DbFingerprint) -> bool {
    existing.mame_metadata == file_signature(&default_mame_sqlite_path())
        && existing.hbmame_metadata == file_signature(&default_hbmame_sqlite_path())
}

fn preview_archive_fingerprint_matches(
    existing: &DbFingerprint,
    current: Vec<(String, u64, i64)>,
) -> bool {
    let existing_archive_count = existing
        .file_fingerprints
        .keys()
        .filter(|path| is_preview_archive_fingerprint_path(path))
        .count();
    if existing_archive_count != current.len() {
        return false;
    }
    current.into_iter().all(|(path, size, mtime_secs)| {
        existing
            .file_fingerprints
            .get(&path)
            .is_some_and(|fingerprint| *fingerprint == (size, mtime_secs))
    })
}

fn is_preview_archive_fingerprint_path(path: &str) -> bool {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    name.ends_with(".mmraw") || name.ends_with(".mmlz4b")
}

fn directory_manifest_metadata_changed(existing: &DirectoryManifest) -> bool {
    for (dir, signature) in existing {
        let Ok(meta) = std::fs::metadata(dir) else {
            return true;
        };
        if !meta.is_dir() {
            return true;
        }
        if meta.len() != signature.dir_size || mtime_secs(&meta) != signature.dir_mtime_secs {
            return true;
        }
    }
    false
}

enum DiscoveryEvent {
    File(FoundFile),
    Done {
        manifest: DirectoryManifest,
        discover_us: u64,
    },
}

fn discover_files_pipelined(roots: Vec<String>) -> mpsc::Receiver<DiscoveryEvent> {
    let (tx, rx) = mpsc::sync_channel(256);
    std::thread::Builder::new()
        .name("library-walker".to_string())
        .spawn(move || {
            let t = Instant::now();
            let manifest = discover_files_streaming(&roots, &tx);
            let _ = tx.send(DiscoveryEvent::Done {
                manifest,
                discover_us: t.elapsed().as_micros() as u64,
            });
        })
        .expect("spawn library-walker");
    rx
}

#[derive(Clone, Copy, Debug)]
struct DirectorySignatureBuilder {
    dir_size: u64,
    dir_mtime_secs: i64,
    child_count: u64,
    hash: u64,
}

impl Default for DirectorySignatureBuilder {
    fn default() -> Self {
        Self {
            dir_size: 0,
            dir_mtime_secs: 0,
            child_count: 0,
            hash: MANIFEST_HASH_OFFSET,
        }
    }
}

impl DirectorySignatureBuilder {
    fn set_dir_metadata(&mut self, meta: &std::fs::Metadata) {
        self.dir_size = meta.len();
        self.dir_mtime_secs = mtime_secs(meta);
    }

    fn add_dir_child(&mut self, name: &str) {
        self.child_count += 1;
        self.hash ^= manifest_child_hash(b"d", name.as_bytes(), 0, 0);
    }

    fn add_file_child(&mut self, name: &str, size: u64, mtime_secs: i64) {
        self.child_count += 1;
        self.hash ^= manifest_child_hash(b"f", name.as_bytes(), size, mtime_secs);
    }

    fn finish(self) -> DirectorySignature {
        DirectorySignature {
            dir_size: self.dir_size,
            dir_mtime_secs: self.dir_mtime_secs,
            child_count: self.child_count,
            hash: self.hash,
        }
    }
}

fn manifest_child_hash(kind: &[u8], name: &[u8], size: u64, mtime_secs: i64) -> u64 {
    let mut hash = MANIFEST_HASH_OFFSET;
    for bytes in [kind, name, &size.to_le_bytes(), &mtime_secs.to_le_bytes()] {
        for b in bytes {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(MANIFEST_HASH_PRIME);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(MANIFEST_HASH_PRIME);
    }
    hash
}

fn discover_files_streaming(
    roots: &[String],
    tx: &mpsc::SyncSender<DiscoveryEvent>,
) -> DirectoryManifest {
    build_directory_manifest(roots, Some(tx))
}

fn build_directory_manifest(
    roots: &[String],
    tx: Option<&mpsc::SyncSender<DiscoveryEvent>>,
) -> DirectoryManifest {
    let mut manifest_builders = BTreeMap::<String, DirectorySignatureBuilder>::new();
    let profiles = launch_profiles::builtin_profiles();
    for root in roots {
        let path = Path::new(root);
        if path.is_dir() {
            let root_key = path.display().to_string();
            if let Ok(meta) = path.metadata() {
                manifest_builders
                    .entry(root_key)
                    .or_default()
                    .set_dir_metadata(&meta);
            } else {
                manifest_builders.entry(root_key).or_default();
            }
            for entry in walkdir::WalkDir::new(path)
                .follow_links(true)
                .into_iter()
                .filter_entry(|e| !should_ignore_path(e.path()))
                .filter_map(Result::ok)
            {
                let p = entry.path();
                if p == path {
                    continue;
                }
                if should_ignore_path(p) {
                    continue;
                }
                let parent = p.parent().unwrap_or(path).display().to_string();
                let name = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if entry.file_type().is_dir() {
                    let dir_key = p.display().to_string();
                    if let Ok(meta) = entry.metadata() {
                        manifest_builders
                            .entry(dir_key)
                            .or_default()
                            .set_dir_metadata(&meta);
                    } else {
                        manifest_builders.entry(dir_key).or_default();
                    }
                    manifest_builders
                        .entry(parent)
                        .or_default()
                        .add_dir_child(&name);
                    continue;
                }
                if !entry.file_type().is_file() {
                    continue;
                }
                let ext = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !is_index_candidate(&profiles, p, &ext) {
                    continue;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                let mtime_secs = mtime_secs(&meta);
                manifest_builders.entry(parent).or_default().add_file_child(
                    &name,
                    meta.len(),
                    mtime_secs,
                );
                let file = FoundFile {
                    path: p.to_path_buf(),
                    ext,
                    size: meta.len(),
                    mtime_secs,
                };
                if let Some(tx) = tx {
                    if tx.send(DiscoveryEvent::File(file)).is_err() {
                        break;
                    }
                }
            }
        }
    }
    manifest_builders
        .into_iter()
        .map(|(dir, sig)| (dir, sig.finish()))
        .collect()
}

fn scan_archive_toc(
    file: &FoundFile,
    format: ArchiveFormat,
    profile: &LaunchProfile,
) -> ArchiveScan {
    let t = Instant::now();
    let (status, entries) = match format {
        ArchiveFormat::Zip => match scan_zip_central_directory(file, profile) {
            Ok(entries) => (ArchiveScanStatus::Ok, entries),
            Err(e) => (ArchiveScanStatus::Error(e), Vec::new()),
        },
        ArchiveFormat::SevenZip | ArchiveFormat::Lha | ArchiveFormat::Lzh | ArchiveFormat::Rar => {
            (ArchiveScanStatus::Unsupported, Vec::new())
        }
        ArchiveFormat::Chd => (ArchiveScanStatus::HeaderOnly, Vec::new()),
    };
    ArchiveScan {
        container: LibraryContainer {
            file_path: file.path.display().to_string(),
            format,
            size: file.size,
            mtime_secs: file.mtime_secs,
            entry_count: entries.len() as u32,
            scan_status: status,
            scan_us: t.elapsed().as_micros() as u64,
        },
        entries,
    }
}

fn scan_container_header(file: &FoundFile, format: ArchiveFormat) -> LibraryContainer {
    LibraryContainer {
        file_path: file.path.display().to_string(),
        format,
        size: file.size,
        mtime_secs: file.mtime_secs,
        entry_count: 0,
        scan_status: ArchiveScanStatus::HeaderOnly,
        scan_us: 0,
    }
}

fn scan_zip_central_directory(
    file: &FoundFile,
    profile: &LaunchProfile,
) -> Result<Vec<LibraryContainerEntry>, String> {
    let mut f = File::open(&file.path).map_err(|e| format!("open zip: {e}"))?;
    let len = f.metadata().map_err(|e| format!("stat zip: {e}"))?.len();
    if len < 22 {
        return Err("zip too small".to_string());
    }

    let tail_len = len.min(66_000) as usize;
    f.seek(SeekFrom::End(-(tail_len as i64)))
        .map_err(|e| format!("seek zip tail: {e}"))?;
    let mut tail = vec![0u8; tail_len];
    f.read_exact(&mut tail)
        .map_err(|e| format!("read zip tail: {e}"))?;
    let Some(eocd) = find_eocd(&tail) else {
        return Err("zip EOCD not found".to_string());
    };

    let mut cd_entries = le_u16(&tail[eocd + 10..eocd + 12]) as usize;
    let mut cd_size = le_u32(&tail[eocd + 12..eocd + 16]) as u64;
    let mut cd_offset = le_u32(&tail[eocd + 16..eocd + 20]) as u64;
    if cd_offset == u32::MAX as u64 || cd_size == u32::MAX as u64 || cd_entries == u16::MAX as usize
    {
        let zip64 = read_zip64_central_directory_location(&mut f, &tail, eocd)?;
        cd_entries = zip64.entries;
        cd_size = zip64.size;
        cd_offset = zip64.offset;
    }
    if cd_offset + cd_size > len {
        return Err("zip central directory outside file".to_string());
    }
    f.seek(SeekFrom::Start(cd_offset))
        .map_err(|e| format!("seek zip central directory: {e}"))?;

    let mut entries = Vec::new();
    let mut remaining = cd_size;
    let mut scanned = 0usize;
    while remaining >= 46 && scanned < cd_entries {
        let entry_offset = cd_size - remaining;
        let mut header = [0u8; 46];
        f.read_exact(&mut header)
            .map_err(|e| format!("read zip central directory header: {e}"))?;
        remaining -= 46;
        if le_u32(&header[0..4]) != 0x0201_4b50 {
            return Err(format!("bad central directory signature at {entry_offset}"));
        }
        scanned += 1;
        let crc32 = le_u32(&header[16..20]);
        let compressed = le_u32(&header[20..24]) as u64;
        let uncompressed = le_u32(&header[24..28]) as u64;
        let name_len = le_u16(&header[28..30]) as u64;
        let extra_len = le_u16(&header[30..32]) as u64;
        let comment_len = le_u16(&header[32..34]) as u64;
        let trailing_len = extra_len + comment_len;
        if name_len + trailing_len > remaining {
            return Err("zip entry name outside central directory".to_string());
        }
        let mut name_buf = vec![0u8; name_len as usize];
        f.read_exact(&mut name_buf)
            .map_err(|e| format!("read zip entry name: {e}"))?;
        remaining -= name_len;
        if trailing_len > 0 {
            f.seek(SeekFrom::Current(trailing_len as i64))
                .map_err(|e| format!("skip zip entry metadata: {e}"))?;
            remaining -= trailing_len;
        }
        let name = String::from_utf8_lossy(&name_buf).into_owned();
        if !name.ends_with('/') && !name.starts_with("__MACOSX/") {
            if let Some(rule) = profile.classify_archive_entry(Path::new(&name)) {
                entries.push(LibraryContainerEntry {
                    file_path: file.path.display().to_string(),
                    entry_path: name.clone(),
                    normalized_title: normalize_title(&name),
                    profile_id: profile.id.to_string(),
                    rule,
                    compressed_size: Some(compressed),
                    uncompressed_size: Some(uncompressed),
                    crc32: Some(crc32),
                    launchable: true,
                    launch_ref: format!("{}/{}", file.path.display(), name),
                });
            }
        }
    }
    Ok(entries)
}

struct ZipCentralDirectoryLocation {
    entries: usize,
    size: u64,
    offset: u64,
}

fn read_zip64_central_directory_location(
    f: &mut File,
    tail: &[u8],
    eocd: usize,
) -> Result<ZipCentralDirectoryLocation, String> {
    let locator = tail[..eocd]
        .windows(4)
        .rposition(|bytes| bytes == [0x50, 0x4b, 0x06, 0x07])
        .ok_or_else(|| "zip64 EOCD locator not found".to_string())?;
    if locator + 20 > tail.len() {
        return Err("zip64 EOCD locator truncated".to_string());
    }
    let zip64_eocd_offset = le_u64(&tail[locator + 8..locator + 16]);
    f.seek(SeekFrom::Start(zip64_eocd_offset))
        .map_err(|e| format!("seek zip64 EOCD: {e}"))?;
    let mut record = [0u8; 56];
    f.read_exact(&mut record)
        .map_err(|e| format!("read zip64 EOCD: {e}"))?;
    if le_u32(&record[0..4]) != 0x0606_4b50 {
        return Err("zip64 EOCD signature not found".to_string());
    }
    let entries = usize::try_from(le_u64(&record[32..40]))
        .map_err(|_| "zip64 entry count too large to index".to_string())?;
    Ok(ZipCentralDirectoryLocation {
        entries,
        size: le_u64(&record[40..48]),
        offset: le_u64(&record[48..56]),
    })
}

fn collection_discoveries_from_container(
    file: &FoundFile,
    profile: &LaunchProfile,
    rule: &CollectionRule,
) -> Vec<GameDiscovery> {
    let mut out = Vec::new();
    if is_amigavision_archive_path(&file.path.display().to_string()) {
        out.push(amigavision_launcher_discovery(file, profile));
    }
    for listing in rule.listings {
        let text = match collection_listing_text(file, listing) {
            Some(text) => text,
            None => continue,
        };
        out.extend(collection_discoveries_from_listing_text(
            file, profile, listing, &text,
        ));
    }
    out
}

fn installed_amigavision_discoveries_from_hdf(
    file: &FoundFile,
    profile: &LaunchProfile,
) -> Option<Vec<GameDiscovery>> {
    if !is_amigavision_installed_hdf_path(&file.path) {
        return None;
    }
    let mut out = vec![amigavision_launcher_discovery(file, profile)];
    for listing in AMIGAVISION_INSTALLED_LISTINGS {
        let Some(listing_path) = installed_amigavision_listing_path(&file.path, listing) else {
            continue;
        };
        let Some(text) = read_lossy_text(&listing_path) else {
            continue;
        };
        out.extend(collection_discoveries_from_listing_text(
            file, profile, listing, &text,
        ));
    }
    Some(out)
}

fn installed_amigavision_listing_path(
    hdf_path: &Path,
    listing: &CollectionListing,
) -> Option<PathBuf> {
    let base = hdf_path.parent()?;
    let relative = listing
        .entry_path
        .strip_prefix("games/Amiga/")
        .unwrap_or(listing.entry_path);
    Some(base.join(relative))
}

fn read_lossy_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn amigavision_launcher_discovery(file: &FoundFile, profile: &LaunchProfile) -> GameDiscovery {
    GameDiscovery {
        source_path: file.path.display().to_string(),
        launch_ref: AMIGAVISION_LAUNCHER_REF.to_string(),
        source_kind: DiscoverySourceKind::CatalogEntry,
        title: "AmigaVision".to_string(),
        category: profile.category.to_string(),
        platform_id: profile.system_id.to_string(),
        core_id: profile.core_name.to_string(),
        hardware_id: profile.system_id.to_string(),
        manufacturer: Some("Commodore".to_string()),
        genre: Some("Launcher".to_string()),
        year: None,
        setname: None,
        parent: None,
        image_path: None,
        has_image: false,
        confidence: DiscoveryConfidence::CatalogMetadata,
    }
}

fn collection_listing_text(file: &FoundFile, listing: &CollectionListing) -> Option<String> {
    let tool = std::env::var("MISTER_7ZA").unwrap_or_else(|_| "/media/fat/linux/7za".to_string());
    let output = Command::new(tool)
        .args(["e", "-so"])
        .arg(&file.path)
        .arg(listing.entry_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn collection_discoveries_from_listing_text(
    file: &FoundFile,
    profile: &LaunchProfile,
    listing: &CollectionListing,
    text: &str,
) -> Vec<GameDiscovery> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|title| GameDiscovery {
            source_path: format!("{}::{}::{title}", file.path.display(), listing.entry_path),
            launch_ref: amigavision_game_launch_ref(title),
            source_kind: DiscoverySourceKind::CatalogEntry,
            title: title.to_string(),
            category: profile.category.to_string(),
            platform_id: profile.system_id.to_string(),
            core_id: profile.core_name.to_string(),
            hardware_id: profile.system_id.to_string(),
            manufacturer: Some("Commodore".to_string()),
            genre: Some(listing.genre.to_string()),
            year: None,
            setname: None,
            parent: None,
            image_path: None,
            has_image: false,
            confidence: DiscoveryConfidence::CatalogMetadata,
        })
        .collect()
}

fn normalize_match_path(path: &str) -> String {
    path.split("::")
        .next()
        .unwrap_or(path)
        .trim()
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

fn normalize_launch_path(path: &str) -> String {
    path.replace("/./", "/")
        .trim()
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

fn parenthesized_setname(path: &str) -> Option<String> {
    let stem = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path);
    let open = stem.rfind('(')?;
    let close = stem[open + 1..].find(')')? + open + 1;
    let value = stem[open + 1..close].trim();
    if value.is_empty() {
        None
    } else {
        Some(normalize_id(value))
    }
}

#[derive(Default)]
struct PreviewImageIndex {
    arcade_stems: Option<HashSet<String>>,
}

impl PreviewImageIndex {
    fn from_env() -> Self {
        let mut index = Self::default();
        if let Ok(archives) = preview_worker::preview_archive_indexes_from_env() {
            for archive in archives {
                if preview_asset_pack_platform(&archive.path) == "arcade" {
                    index.arcade_stems.get_or_insert_with(HashSet::new).extend(
                        archive
                            .entries
                            .into_iter()
                            .map(|stem| stem.to_ascii_lowercase()),
                    );
                }
            }
        }
        index
    }

    #[cfg(test)]
    fn arcade(stems: &[&str]) -> Self {
        Self {
            arcade_stems: Some(stems.iter().map(|stem| stem.to_ascii_lowercase()).collect()),
        }
    }

    fn has_arcade_stem(&self, stem: &str) -> bool {
        self.arcade_stems
            .as_ref()
            .is_some_and(|stems| stems.contains(&stem.to_ascii_lowercase()))
    }
}

fn attach_preview_image(discovery: &mut GameDiscovery, preview_images: &PreviewImageIndex) {
    if discovery.platform_id == "arcade" {
        let Some(setname) = discovery
            .setname
            .as_deref()
            .filter(|setname| !setname.trim().is_empty())
        else {
            return;
        };
        if !preview_images.has_arcade_stem(setname) {
            return;
        }
        discovery.image_path = Some(format!("/media/fat/_Arcade/media/screenshot/{setname}.png"));
        discovery.has_image = true;
    }
}

fn discovery_from_profile_file(
    file: &FoundFile,
    profile: &LaunchProfile,
    rule: &PayloadRule,
    profiles: &[LaunchProfile],
) -> GameDiscovery {
    if file.ext == "mra" {
        if let Some(mra) = read_mra_metadata(&file.path) {
            let core_id = mra
                .rbf
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(normalize_id)
                .unwrap_or_else(|| profile.core_name.to_string());
            let hardware_id = mra
                .platform
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(normalize_id)
                .unwrap_or_else(|| core_id.clone());
            return GameDiscovery {
                source_path: file.path.display().to_string(),
                launch_ref: file.path.display().to_string(),
                source_kind: DiscoverySourceKind::Mra,
                title: mra
                    .name
                    .unwrap_or_else(|| title_from_path(&file.path.display().to_string())),
                category: profile.category.to_string(),
                platform_id: profile.system_id.to_string(),
                core_id,
                hardware_id,
                manufacturer: mra.manufacturer,
                genre: mra.category.or(mra.catver),
                year: mra.year.and_then(|s| s.parse::<u16>().ok()),
                setname: mra.setname,
                parent: mra.parent,
                image_path: None,
                has_image: false,
                confidence: if mra.platform.is_some() {
                    DiscoveryConfidence::MraHardware
                } else {
                    DiscoveryConfidence::MraCore
                },
            };
        }
    }
    if file.ext == "mgl" {
        if let Some(mgl) = read_mgl_metadata(&file.path) {
            let payload_profile = mgl
                .file_path
                .as_deref()
                .and_then(|payload| profile_for_mgl_payload(profiles, &file.path, payload));
            let profile = payload_profile.unwrap_or(profile);
            let setname = if profile.system_id == "neogeo" {
                neogeo_mgl_setname(&file.path, mgl.file_path.as_deref())
            } else {
                None
            };
            return GameDiscovery {
                source_path: file.path.display().to_string(),
                launch_ref: file.path.display().to_string(),
                source_kind: DiscoverySourceKind::Mgl,
                title: title_from_path(&file.path.display().to_string()),
                category: profile.category.to_string(),
                platform_id: profile.system_id.to_string(),
                core_id: mgl
                    .rbf
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .map(normalize_id)
                    .unwrap_or_else(|| profile.core_name.to_string()),
                hardware_id: profile.system_id.to_string(),
                manufacturer: None,
                genre: None,
                year: None,
                setname,
                parent: None,
                image_path: None,
                has_image: false,
                confidence: DiscoveryConfidence::PayloadPath,
            };
        }
    }

    GameDiscovery {
        source_path: file.path.display().to_string(),
        launch_ref: file.path.display().to_string(),
        source_kind: DiscoverySourceKind::PayloadFile,
        title: title_from_path(&file.path.display().to_string()),
        category: profile.category.to_string(),
        platform_id: profile.system_id.to_string(),
        core_id: profile.core_name.to_string(),
        hardware_id: profile.system_id.to_string(),
        manufacturer: None,
        genre: None,
        year: None,
        setname: None,
        parent: None,
        image_path: None,
        has_image: false,
        confidence: profile_confidence(rule),
    }
}

fn discovery_from_profile_archive_entry(
    entry: &LibraryContainerEntry,
    profile: &LaunchProfile,
    rule: &PayloadRule,
) -> GameDiscovery {
    GameDiscovery {
        source_path: format!("{}::{}", entry.file_path, entry.entry_path),
        launch_ref: entry.launch_ref.clone(),
        source_kind: DiscoverySourceKind::ArchiveEntry,
        title: title_from_path(&entry.entry_path),
        category: profile.category.to_string(),
        platform_id: profile.system_id.to_string(),
        core_id: profile.core_name.to_string(),
        hardware_id: profile.system_id.to_string(),
        manufacturer: None,
        genre: None,
        year: None,
        setname: parenthesized_setname(&entry.entry_path),
        parent: None,
        image_path: None,
        has_image: false,
        confidence: match rule.provenance.kind {
            RuleSourceKind::MainSource | RuleSourceKind::Mgl | RuleSourceKind::Mra => {
                DiscoveryConfidence::ArchiveToc
            }
            RuleSourceKind::ConfStr | RuleSourceKind::MagikProfile => profile_confidence(rule),
        },
    }
}

fn profile_for_mgl_payload<'a>(
    profiles: &'a [LaunchProfile],
    mgl_path: &Path,
    payload: &str,
) -> Option<&'a LaunchProfile> {
    let path = resolve_mgl_payload_path(mgl_path, payload);
    profile_for_path(profiles, &path)
}

fn resolve_mgl_payload_path(mgl_path: &Path, payload: &str) -> PathBuf {
    if payload.starts_with('/') {
        PathBuf::from(payload)
    } else if payload.starts_with("games/") {
        PathBuf::from("/media/fat").join(payload)
    } else {
        mgl_path.parent().unwrap_or(Path::new("/")).join(payload)
    }
}

fn profile_confidence(rule: &PayloadRule) -> DiscoveryConfidence {
    match rule.provenance.kind {
        RuleSourceKind::Mra => DiscoveryConfidence::MraCore,
        RuleSourceKind::Mgl | RuleSourceKind::MainSource | RuleSourceKind::MagikProfile => {
            DiscoveryConfidence::PayloadPath
        }
        RuleSourceKind::ConfStr => DiscoveryConfidence::Extension,
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[derive(Default)]
struct MraMetadata {
    name: Option<String>,
    rbf: Option<String>,
    platform: Option<String>,
    manufacturer: Option<String>,
    category: Option<String>,
    catver: Option<String>,
    year: Option<String>,
    setname: Option<String>,
    parent: Option<String>,
}

#[derive(Default)]
struct MglMetadata {
    rbf: Option<String>,
    file_path: Option<String>,
}

fn read_mra_metadata(path: &Path) -> Option<MraMetadata> {
    let mut file = File::open(path).ok()?;
    let mut data = vec![0u8; MRA_PREFIX_BYTES];
    let n = file.read(&mut data).ok()?;
    data.truncate(n);
    let text = String::from_utf8_lossy(&data);
    Some(MraMetadata {
        name: tag_text(&text, "name"),
        rbf: tag_text(&text, "rbf"),
        platform: tag_text(&text, "platform"),
        manufacturer: tag_text(&text, "manufacturer"),
        category: tag_text(&text, "category"),
        catver: tag_text(&text, "catver"),
        year: tag_text(&text, "year"),
        setname: tag_text(&text, "setname"),
        parent: tag_text(&text, "parent"),
    })
}

fn read_mgl_metadata(path: &Path) -> Option<MglMetadata> {
    let mut file = File::open(path).ok()?;
    let mut data = String::new();
    file.read_to_string(&mut data).ok()?;
    Some(MglMetadata {
        rbf: tag_text(&data, "rbf"),
        file_path: attr_text(&data, "path"),
    })
}

fn neogeo_mgl_setname(mgl_path: &Path, payload_path: Option<&str>) -> Option<String> {
    payload_path
        .and_then(parenthesized_setname)
        .or_else(|| parenthesized_setname(&mgl_path.display().to_string()))
}

fn tag_text(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    let value = text[start..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(html_unescape_minimal(value))
    }
}

fn attr_text(text: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = text.find(&needle)? + needle.len();
    let end = text[start..].find('"')? + start;
    Some(html_unescape_minimal(text[start..end].trim()))
}

fn html_unescape_minimal(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn normalize_id(value: &str) -> String {
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

fn unique_discovery_count(discoveries: &[GameDiscovery]) -> usize {
    let covered_payloads = covered_payload_paths(discoveries);
    preferred_playable_discoveries_by_key(discoveries, &covered_payloads).len()
}

fn preferred_playable_discoveries_by_key<'a>(
    discoveries: &'a [GameDiscovery],
    covered_payloads: &HashSet<String>,
) -> BTreeMap<String, &'a GameDiscovery> {
    let mut out = BTreeMap::<String, &'a GameDiscovery>::new();
    for discovery in discoveries {
        if !is_playable_discovery_with_coverage(discovery, covered_payloads) {
            continue;
        }
        let key = discovery_unique_key(discovery);
        match out.get(&key).copied() {
            Some(existing) if prefer_discovery_variant(discovery, existing) => {
                out.insert(key, discovery);
            }
            None => {
                out.insert(key, discovery);
            }
            _ => {}
        }
    }
    out
}

fn prefer_discovery_variant(a: &GameDiscovery, b: &GameDiscovery) -> bool {
    let a_score = discovery_variant_score(a);
    let b_score = discovery_variant_score(b);
    if a_score != b_score {
        return a_score > b_score;
    }
    normalize_launch_path(&a.launch_ref) < normalize_launch_path(&b.launch_ref)
}

fn discovery_variant_score(discovery: &GameDiscovery) -> i32 {
    let haystack = format!(
        "{} {} {} {}",
        discovery.title,
        discovery.launch_ref,
        discovery.setname.as_deref().unwrap_or(""),
        discovery.parent.as_deref().unwrap_or("")
    )
    .to_ascii_lowercase();

    variant_score_from_haystack(&haystack)
}

fn confidence_str(confidence: DiscoveryConfidence) -> &'static str {
    match confidence {
        DiscoveryConfidence::MraHardware => "mra-hardware",
        DiscoveryConfidence::MraCore => "mra-core",
        DiscoveryConfidence::PayloadPath => "payload-path",
        DiscoveryConfidence::Extension => "extension",
        DiscoveryConfidence::ArchiveToc => "archive-toc",
        DiscoveryConfidence::CatalogMetadata => "catalog-metadata",
    }
}

fn save_sqlite_scan(path: &Path, scan: &LibraryScan) -> Result<u64, String> {
    save_sqlite_scan_with_progress(path, scan, None)
}

fn save_sqlite_scan_with_progress(
    path: &Path,
    scan: &LibraryScan,
    progress: ProgressCallback<'_>,
) -> Result<u64, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create sqlite dir: {e}"))?;
    }

    let build_tmp_path = sqlite_build_temp_path(path);
    let final_tmp_path = sqlite_temp_path(path);
    for tmp_path in [&build_tmp_path, &final_tmp_path] {
        match std::fs::remove_file(tmp_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("remove stale sqlite temp: {e}")),
        }
    }

    let software_hash_cache = SoftwareHashCache::load(path);
    if let Err(e) = write_sqlite_scan(&build_tmp_path, scan, progress, software_hash_cache) {
        let _ = std::fs::remove_file(&build_tmp_path);
        return Err(e);
    }
    sync_file_best_effort(&build_tmp_path, "sqlite build temp")?;
    if build_tmp_path != final_tmp_path {
        std::fs::copy(&build_tmp_path, &final_tmp_path)
            .map_err(|e| format!("copy sqlite temp into final dir: {e}"))?;
        let _ = std::fs::remove_file(&build_tmp_path);
    }
    sync_file_best_effort(&final_tmp_path, "sqlite temp")?;
    std::fs::rename(&final_tmp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&final_tmp_path);
        let _ = std::fs::remove_file(&build_tmp_path);
        format!("replace sqlite: {e}")
    })?;
    sync_parent_dir(path);
    std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("stat sqlite: {e}"))
}

fn sqlite_build_temp_path(path: &Path) -> PathBuf {
    let Some(build_dir) = std::env::var_os("MISTER_LIBRARY_SQLITE_BUILD_DIR").map(PathBuf::from)
    else {
        return sqlite_temp_path(path);
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("library.sqlite3");
    let _ = std::fs::create_dir_all(&build_dir);
    build_dir.join(format!(".{name}.build.{}", std::process::id()))
}

fn sqlite_temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("library.sqlite3");
    path.with_file_name(format!(".{name}.tmp.{}", std::process::id()))
}

fn sync_parent_dir(path: &Path) {
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

fn file_signature(path: &Path) -> FileSignature {
    std::fs::metadata(path)
        .map(|metadata| FileSignature {
            size: metadata.len(),
            mtime_secs: mtime_secs(&metadata),
        })
        .unwrap_or_default()
}

fn load_mame_machine_metadata(path: &Path) -> HashMap<String, MameMachineMetadata> {
    let Ok(conn) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return HashMap::new();
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT setname,parent_setname,title,year,manufacturer FROM mame_machines")
    else {
        return HashMap::new();
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            MameMachineMetadata {
                parent_setname: row.get(1)?,
                title: row.get(2)?,
                year: row.get(3)?,
                manufacturer: row.get(4)?,
            },
        ))
    }) else {
        return HashMap::new();
    };
    rows.filter_map(|row| row.ok()).collect()
}

fn load_mame_software_metadata(path: &Path) -> MameSoftwareMetadata {
    let Ok(conn) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return MameSoftwareMetadata::default();
    };
    if !sqlite_table_exists(&conn, "mame_software_items").unwrap_or(false) {
        return MameSoftwareMetadata::default();
    }
    let mut metadata = MameSoftwareMetadata::default();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT list_name,software_name,parent_name,description,year,publisher,region,source_version
         FROM mame_software_items",
    ) {
        if let Ok(rows) = stmt.query_map([], |row| {
            let _source_version = row.get::<_, String>(7)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                MameSoftwareItemMetadata {
                    parent_name: row.get(2)?,
                    description: row.get(3)?,
                    year: row.get(4)?,
                    publisher: row.get(5)?,
                    region: row.get(6)?,
                },
            ))
        }) {
            for row in rows.flatten() {
                let (list, name, item) = row;
                let title_key = canonical_variant_title(&item.description);
                metadata
                    .title_index
                    .entry((list.clone(), title_key))
                    .or_default()
                    .push(name.clone());
                let family = item
                    .parent_name
                    .as_deref()
                    .filter(|parent| !parent.trim().is_empty())
                    .unwrap_or(&name)
                    .to_string();
                metadata
                    .family_members
                    .entry((list.clone(), family))
                    .or_default()
                    .push(name.clone());
                metadata.items.insert((list, name), item);
            }
        }
    }
    if let Ok(mut stmt) = conn.prepare(
        "SELECT list_name,software_name,size,crc32
         FROM mame_software_hashes
         WHERE size IS NOT NULL AND crc32 IS NOT NULL",
    ) {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        }) {
            for (list, name, size, crc_hex) in rows.flatten() {
                let Ok(size) = u64::try_from(size) else {
                    continue;
                };
                let Some(crc) = parse_hex_u32(&crc_hex) else {
                    continue;
                };
                metadata
                    .hash_index
                    .entry((list, size, crc))
                    .or_default()
                    .push(name);
            }
        }
    }
    if let Ok(mut stmt) = conn.prepare(
        "SELECT list_name,software_name,disk_sha1
         FROM mame_software_hashes
         WHERE disk_sha1 IS NOT NULL",
    ) {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        }) {
            for (list, name, sha1) in rows.flatten() {
                metadata
                    .disk_index
                    .entry((list, sha1.to_ascii_lowercase()))
                    .or_default()
                    .push(name);
            }
        }
    }
    for members in metadata.family_members.values_mut() {
        members.sort();
        members.dedup();
    }
    metadata
}

fn load_arcade_machine_metadata(mame_path: &Path, hbmame_path: &Path) -> ArcadeMachineMetadata {
    ArcadeMachineMetadata {
        mame: load_mame_machine_metadata(mame_path),
        hbmame: load_mame_machine_metadata(hbmame_path),
    }
}

fn write_simple_mame_metadata_db(
    path: &Path,
    rows: &BTreeMap<String, (String, String, Option<String>, Option<String>)>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create metadata dir {}: {e}", parent.display()))?;
    }
    let tmp = sqlite_temp_path(path);
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove stale metadata temp {}: {e}", tmp.display())),
    }
    let mut conn =
        Connection::open(&tmp).map_err(|e| format!("open metadata temp {}: {e}", tmp.display()))?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode=OFF;
        PRAGMA synchronous=OFF;
        CREATE TABLE mame_machines (
            setname TEXT PRIMARY KEY,
            parent_setname TEXT,
            title TEXT NOT NULL,
            year TEXT,
            manufacturer TEXT
        ) WITHOUT ROWID;
        "#,
    )
    .map_err(|e| format!("create metadata schema: {e}"))?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("begin metadata tx: {e}"))?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO mame_machines(setname,parent_setname,title,year,manufacturer)
                 VALUES (?1,?2,?3,?4,?5)",
            )
            .map_err(|e| format!("prepare metadata insert: {e}"))?;
        for (setname, (parent, title, year, manufacturer)) in rows {
            stmt.execute(params![
                setname.as_str(),
                parent.as_str(),
                title.as_str(),
                year.as_deref(),
                manufacturer.as_deref()
            ])
            .map_err(|e| format!("insert metadata row {setname}: {e}"))?;
        }
    }
    tx.commit()
        .map_err(|e| format!("commit metadata tx: {e}"))?;
    sync_parent_dir(&tmp);
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("replace metadata db {}: {e}", path.display()))?;
    sync_parent_dir(path);
    Ok(())
}

fn mame_identity_for_discovery(discovery: &GameDiscovery) -> Option<String> {
    if discovery.platform_id != "arcade" && discovery.platform_id != "neogeo" {
        return None;
    }
    discovery
        .setname
        .as_deref()
        .map(str::trim)
        .filter(|setname| !setname.is_empty())
        .map(normalize_id)
}

fn software_list_for_platform(platform_id: &str) -> Option<&'static str> {
    match platform_id {
        "nes" => Some("nes"),
        "snes" => Some("snes"),
        "n64" => Some("n64"),
        "sms" => Some("sms"),
        "megadrive" => Some("megadriv"),
        "saturn" => Some("saturn"),
        _ => None,
    }
}

fn mame_software_identity_for_discovery(
    discovery: &GameDiscovery,
    metadata: &MameSoftwareMetadata,
    software_hash_cache: &mut SoftwareHashCache,
) -> Option<SoftwareIdentity> {
    mame_software_identity_for_discovery_with_hash_matcher(discovery, metadata, |discovery,
                                                                                 list_name,
                                                                                 metadata| {
        match_software_by_file_hash(discovery, list_name, metadata, software_hash_cache)
    })
}

fn mame_software_identity_for_discovery_with_hash_matcher(
    discovery: &GameDiscovery,
    metadata: &MameSoftwareMetadata,
    hash_matcher: impl FnOnce(&GameDiscovery, &str, &MameSoftwareMetadata) -> Option<String>,
) -> Option<SoftwareIdentity> {
    let list_name = software_list_for_platform(&discovery.platform_id)?;
    let title_key = canonical_variant_title(&discovery.title);
    if let Some(names) = metadata
        .title_index
        .get(&(list_name.to_string(), title_key))
        .filter(|names| !names.is_empty())
    {
        return software_identity_from_metadata(list_name, &names[0], metadata, "filename");
    }
    if let Some(software_name) = hash_matcher(discovery, list_name, metadata) {
        return software_identity_from_metadata(
            list_name,
            &software_name,
            metadata,
            "mame-software",
        );
    }
    None
}

fn software_identity_from_metadata(
    list_name: &str,
    software_name: &str,
    metadata: &MameSoftwareMetadata,
    source: &'static str,
) -> Option<SoftwareIdentity> {
    let item = metadata
        .items
        .get(&(list_name.to_string(), software_name.to_string()))?;
    let family = item
        .parent_name
        .as_deref()
        .filter(|parent| !parent.trim().is_empty())
        .unwrap_or(software_name)
        .to_string();
    Some(SoftwareIdentity {
        list_name: list_name.to_string(),
        software_name: software_name.to_string(),
        family_id: format!("{list_name}:{family}"),
        metadata_title: Some(item.description.clone()),
        year: item.year.clone(),
        manufacturer: item.publisher.clone(),
        region: item.region.clone(),
        source,
    })
}

fn match_software_by_file_hash(
    discovery: &GameDiscovery,
    list_name: &str,
    metadata: &MameSoftwareMetadata,
    software_hash_cache: &mut SoftwareHashCache,
) -> Option<String> {
    match_software_by_file_hash_with_cache(
        discovery,
        list_name,
        metadata,
        env_bool("MISTER_LIBRARY_SOFTWARE_HASH"),
        software_hash_cache,
    )
}

fn match_software_by_file_hash_with_cache(
    discovery: &GameDiscovery,
    list_name: &str,
    metadata: &MameSoftwareMetadata,
    full_rom_hashing_enabled: bool,
    software_hash_cache: &mut SoftwareHashCache,
) -> Option<String> {
    if !matches!(
        discovery.source_kind,
        DiscoverySourceKind::PayloadFile | DiscoverySourceKind::ArchiveEntry
    ) {
        return None;
    }
    let source_path = discovery
        .source_path
        .split("::")
        .next()
        .unwrap_or(&discovery.source_path);
    if list_name == "saturn" && path_ext(source_path).as_deref() == Some("chd") {
        if let Some(disk_sha1) = chd_raw_sha1(source_path) {
            let key = (list_name.to_string(), disk_sha1);
            if let Some(names) = metadata
                .disk_index
                .get(&key)
                .filter(|names| !names.is_empty())
            {
                return Some(names[0].clone());
            }
        }
        return None;
    }
    if list_name == "saturn" {
        return None;
    }
    if !full_rom_hashing_enabled {
        return None;
    }
    software_hash_cache.get_or_compute(list_name, source_path, || {
        match_software_by_full_rom_hash(source_path, list_name, metadata)
    })
}

fn match_software_by_full_rom_hash(
    source_path: &str,
    list_name: &str,
    metadata: &MameSoftwareMetadata,
) -> Option<String> {
    let bytes = std::fs::read(source_path).ok()?;
    for candidate in rom_hash_candidates(list_name, &bytes) {
        let crc = crc32(&candidate);
        let key = (list_name.to_string(), candidate.len() as u64, crc);
        if let Some(names) = metadata
            .hash_index
            .get(&key)
            .filter(|names| !names.is_empty())
        {
            return Some(names[0].clone());
        }
    }
    None
}

impl SoftwareHashCache {
    fn load(path: &Path) -> Self {
        let Ok(conn) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            return Self::default();
        };
        if !sqlite_table_exists(&conn, "software_hash_cache").unwrap_or(false) {
            return Self::default();
        }
        let Ok(mut stmt) = conn.prepare(
            "SELECT list_name,file_path,size,mtime_secs,software_name FROM software_hash_cache",
        ) else {
            return Self::default();
        };
        let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                SoftwareHashCacheKey {
                    list_name: row.get(0)?,
                    file_path: row.get(1)?,
                    size: row.get::<_, i64>(2)?.max(0) as u64,
                    mtime_secs: row.get(3)?,
                },
                row.get::<_, Option<String>>(4)?,
            ))
        }) else {
            return Self::default();
        };
        let mut cache = Self::default();
        for row in rows.flatten() {
            cache.entries.insert(row.0, row.1);
        }
        cache
    }

    fn get_or_compute(
        &mut self,
        list_name: &str,
        source_path: &str,
        compute: impl FnOnce() -> Option<String>,
    ) -> Option<String> {
        let Some(key) = software_hash_cache_key(list_name, source_path) else {
            return compute();
        };
        if let Some(cached) = self.entries.get(&key) {
            return cached.clone();
        }
        let computed = compute();
        self.entries.insert(key, computed.clone());
        computed
    }
}

fn software_hash_cache_key(list_name: &str, source_path: &str) -> Option<SoftwareHashCacheKey> {
    let signature = file_signature(Path::new(source_path));
    if signature.size == 0 && signature.mtime_secs == 0 {
        return None;
    }
    Some(SoftwareHashCacheKey {
        list_name: list_name.to_string(),
        file_path: source_path.to_string(),
        size: signature.size,
        mtime_secs: signature.mtime_secs,
    })
}

fn chd_raw_sha1(path: &str) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut header = [0u8; 124];
    file.read_exact(&mut header).ok()?;
    chd_raw_sha1_from_header(&header)
}

fn chd_raw_sha1_from_header(header: &[u8]) -> Option<String> {
    if header.len() < 124 || &header[..8] != b"MComprHD" {
        return None;
    }
    let length = be_u32(&header[8..12]) as usize;
    let version = be_u32(&header[12..16]);
    let range = match version {
        3 if length == 120 => 80..100,
        4 if length == 108 => 88..108,
        5 if length == 124 => 64..84,
        _ => return None,
    };
    Some(hex_lower(&header[range]))
}

fn rom_hash_candidates(list_name: &str, bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    match list_name {
        "nes" => {
            if bytes.len() > 16 && &bytes[..4] == b"NES\x1a" {
                out.push(bytes[16..].to_vec());
            }
            out.push(bytes.to_vec());
        }
        "snes" => {
            if bytes.len() > 512 {
                out.push(bytes[512..].to_vec());
            }
            out.push(bytes.to_vec());
        }
        "n64" => {
            out.push(bytes.to_vec());
            out.push(swap_pairs(bytes));
            out.push(swap_words(bytes));
            out.push(reverse_words(bytes));
        }
        "sms" | "megadriv" => out.push(bytes.to_vec()),
        _ => out.push(bytes.to_vec()),
    }
    out.dedup();
    out
}

fn swap_pairs(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for chunk in out.chunks_exact_mut(2) {
        chunk.swap(0, 1);
    }
    out
}

fn swap_words(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for chunk in out.chunks_exact_mut(4) {
        chunk.swap(0, 2);
        chunk.swap(1, 3);
    }
    out
}

fn reverse_words(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for chunk in out.chunks_exact_mut(4) {
        chunk.reverse();
    }
    out
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn parse_hex_u32(value: &str) -> Option<u32> {
    u32::from_str_radix(value.trim(), 16).ok()
}

fn mame_identity_projection<'a>(
    identity_id: &str,
    metadata: &'a ArcadeMachineMetadata,
) -> (
    String,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    &'static str,
) {
    if let Some(machine) = metadata.mame.get(identity_id) {
        let family_id = machine
            .parent_setname
            .as_deref()
            .filter(|parent| !parent.trim().is_empty())
            .unwrap_or(identity_id)
            .to_string();
        (
            family_id,
            Some(machine.title.as_str()),
            machine.year.as_deref(),
            machine.manufacturer.as_deref(),
            "mame",
        )
    } else if let Some(machine) = metadata.hbmame.get(identity_id) {
        let family_id = machine
            .parent_setname
            .as_deref()
            .filter(|parent| !parent.trim().is_empty())
            .unwrap_or(identity_id)
            .to_string();
        (
            family_id,
            Some(machine.title.as_str()),
            machine.year.as_deref(),
            machine.manufacturer.as_deref(),
            "hbmame",
        )
    } else {
        (identity_id.to_string(), None, None, None, "setname")
    }
}

fn materialize_arcade_ui_projections(tx: &rusqlite::Transaction<'_>) -> Result<(), String> {
    tx.execute_batch(
        r#"
        INSERT INTO ui_arcade_variants(
            family_id,
            variant_ordinal,
            launchable_id,
            title,
            sort_title,
            launch_ref,
            image_path,
            has_image,
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
                COALESCE(g.image_path, '') AS discovery_image_path,
                g.has_image AS discovery_has_image,
                l.system_id AS system_id,
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
                END AS is_parent,
                exact.pack_id AS exact_pack_id,
                exact.asset_key AS exact_asset_key,
                parent.pack_id AS parent_pack_id,
                parent.asset_key AS parent_asset_key,
                sibling.pack_id AS sibling_pack_id,
                sibling.asset_key AS sibling_asset_key
            FROM launchables l
            JOIN games g ON g.game_id = l.launchable_id
            LEFT JOIN launchable_identities i
              ON i.launchable_id = l.launchable_id
             AND i.namespace = 'mame'
            LEFT JOIN asset_entries exact
              ON exact.identity_namespace = 'mame'
             AND exact.identity_id = i.identity_id
            LEFT JOIN asset_entries parent
              ON parent.identity_namespace = 'mame'
             AND parent.identity_id = i.family_id
            LEFT JOIN (
                SELECT family_id, MIN(pack_id) AS pack_id, MIN(asset_key) AS asset_key
                FROM asset_entries
                WHERE identity_namespace = 'mame'
                GROUP BY family_id
            ) sibling
              ON sibling.family_id = COALESCE(i.family_id, l.launchable_id)
            WHERE l.system_id IN ('arcade','neogeo')
              AND l.launch_ref != ''
        ),
        resolved AS (
            SELECT
                *,
                COALESCE(exact_pack_id, parent_pack_id, sibling_pack_id) AS asset_pack_id,
                COALESCE(exact_asset_key, parent_asset_key, sibling_asset_key) AS asset_key,
                CASE
                    WHEN exact_asset_key IS NOT NULL THEN 'exact'
                    WHEN parent_asset_key IS NOT NULL THEN 'parent'
                    WHEN sibling_asset_key IS NOT NULL THEN 'sibling'
                    ELSE 'none'
                END AS asset_link_reason
            FROM candidates
        ),
        ranked AS (
            SELECT
                *,
                row_number() OVER (
                    PARTITION BY family_id
                    ORDER BY is_parent DESC,
                             CASE WHEN asset_key IS NOT NULL THEN 1 ELSE discovery_has_image END DESC,
                             sort_title ASC,
                             launch_ref ASC
                ) AS family_rank,
                row_number() OVER (
                    PARTITION BY family_id
                    ORDER BY is_parent DESC,
                             CASE WHEN asset_key IS NOT NULL THEN 1 ELSE discovery_has_image END DESC,
                             sort_title ASC,
                             launch_ref ASC
                ) - 1 AS variant_ordinal
            FROM resolved
        )
        SELECT
            family_id,
            variant_ordinal,
            launchable_id,
            title,
            sort_title,
            launch_ref,
            CASE
                WHEN asset_key IS NOT NULL THEN '/media/fat/_Arcade/media/screenshot/' || asset_key || '.png'
                ELSE discovery_image_path
            END,
            CASE
                WHEN asset_key IS NOT NULL THEN 1
                ELSE discovery_has_image
            END,
            system_id,
            identity_id,
            parent_setname,
            asset_pack_id,
            asset_key,
            asset_link_reason,
            CASE WHEN family_rank = 1 THEN 1 ELSE 0 END,
            CASE
                WHEN family_rank = 1 AND is_parent = 1 THEN 'installed-parent'
                WHEN family_rank = 1 THEN 'deterministic-child'
                ELSE 'variant'
            END
        FROM ranked
        ORDER BY family_id, variant_ordinal;

        INSERT INTO ui_arcade_preferred(
            ordinal,
            launchable_id,
            title,
            sort_title,
            launch_ref,
            image_path,
            has_image,
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
            image_path,
            has_image,
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
    )
    .map_err(|e| format!("materialize arcade ui projections: {e}"))
}

fn register_preview_asset_packs(
    tx: &rusqlite::Transaction<'_>,
    mame_metadata: &HashMap<String, MameMachineMetadata>,
    software_metadata: &MameSoftwareMetadata,
    indexes: &[preview_worker::PreviewArchiveIndex],
) -> Result<(), String> {
    let mut pack_stmt = tx
        .prepare(
            "INSERT INTO asset_packs(pack_id,platform_id,asset_type,local_path,codec,version)
             VALUES (?1,?2,?3,?4,?5,?6)",
        )
        .map_err(|e| format!("prepare preview asset pack insert: {e}"))?;
    let mut entry_stmt = tx
        .prepare(
            "INSERT INTO asset_entries(pack_id,asset_key,identity_namespace,identity_id,family_id,width,height)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
        )
        .map_err(|e| format!("prepare preview asset entry insert: {e}"))?;
    for (idx, index) in indexes.iter().enumerate() {
        let platform = preview_asset_pack_platform(&index.path);
        let pack_id = if idx == 0 {
            format!("{platform}-screenshot-v1")
        } else {
            format!("{platform}-screenshot-v1-{idx}")
        };
        pack_stmt
            .execute(params![
                pack_id.as_str(),
                platform,
                "screenshot",
                index.path.as_str(),
                index.codec,
                "v1"
            ])
            .map_err(|e| format!("insert preview asset pack: {e}"))?;
        for entry in &index.entries {
            let Some((asset_key, namespace, identity_id, family_id)) =
                preview_asset_entry_identity(platform, entry, mame_metadata, software_metadata)
            else {
                continue;
            };
            entry_stmt
                .execute(params![
                    pack_id.as_str(),
                    asset_key.as_str(),
                    namespace,
                    identity_id.as_str(),
                    family_id.as_str(),
                    Option::<i64>::None,
                    Option::<i64>::None
                ])
                .map_err(|e| format!("insert preview asset entry: {e}"))?;
        }
    }
    Ok(())
}

fn preview_asset_entry_identity(
    platform: &str,
    entry: &str,
    mame_metadata: &HashMap<String, MameMachineMetadata>,
    software_metadata: &MameSoftwareMetadata,
) -> Option<(String, &'static str, String, String)> {
    if let Some((list_name, software_name)) = parse_software_asset_key(entry) {
        if software_list_for_platform(platform).is_some_and(|expected| expected != list_name) {
            return None;
        }
        let identity_id = format!("{list_name}:{software_name}");
        let family_id = software_metadata
            .items
            .get(&(list_name.clone(), software_name.clone()))
            .and_then(|item| item.parent_name.as_deref())
            .filter(|parent| !parent.trim().is_empty())
            .map(|parent| format!("{list_name}:{parent}"))
            .unwrap_or_else(|| identity_id.clone());
        return Some((entry.to_string(), "mame-software", identity_id, family_id));
    }
    if software_list_for_platform(platform).is_some() {
        return None;
    }
    let identity_id = normalize_id(entry);
    let family_id = mame_metadata
        .get(&identity_id)
        .and_then(|machine| machine.parent_setname.as_deref())
        .filter(|parent| !parent.trim().is_empty())
        .unwrap_or(identity_id.as_str())
        .to_string();
    Some((identity_id.clone(), "mame", identity_id, family_id))
}

fn preview_asset_pack_platform(path: &str) -> &'static str {
    let path = path.to_ascii_lowercase();
    if path.contains("neogeo") {
        "neogeo"
    } else if path.contains("snes-screenshots") {
        "snes"
    } else if path.contains("nes-screenshots") {
        "nes"
    } else if path.contains("n64-screenshots") {
        "n64"
    } else if path.contains("sms-screenshots") {
        "sms"
    } else if path.contains("megadrive-screenshots") {
        "megadrive"
    } else if path.contains("saturn") {
        "saturn"
    } else {
        "arcade"
    }
}

#[cfg(test)]
fn software_asset_key(list_name: &str, software_name: &str) -> String {
    format!("mame-software__{list_name}__{software_name}")
}

fn parse_software_asset_key(key: &str) -> Option<(String, String)> {
    let mut parts = key.split("__");
    if parts.next()? != "mame-software" {
        return None;
    }
    let list_name = parts.next()?.trim();
    let software_name = parts.next()?.trim();
    if list_name.is_empty() || software_name.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((list_name.to_string(), software_name.to_string()))
}

fn console_preview_assets(
    indexes: &[preview_worker::PreviewArchiveIndex],
) -> HashMap<String, String> {
    let mut assets = HashMap::new();
    for index in indexes
        .iter()
        .filter(|index| preview_asset_pack_platform(&index.path) != "arcade")
    {
        let platform = preview_asset_pack_platform(&index.path);
        for entry in &index.entries {
            if let Some((list_name, software_name)) = parse_software_asset_key(entry) {
                if software_list_for_platform(platform)
                    .is_some_and(|expected| expected != list_name)
                {
                    continue;
                }
                assets
                    .entry(format!("{list_name}:{software_name}"))
                    .or_insert_with(|| entry.to_string());
            }
        }
    }
    assets
}

fn console_preview_image_path(
    identity: &SoftwareIdentity,
    software_metadata: &MameSoftwareMetadata,
    assets: &HashMap<String, String>,
) -> Option<String> {
    let exact = format!("{}:{}", identity.list_name, identity.software_name);
    if let Some(asset_key) = assets.get(&exact) {
        return Some(format!(
            "/media/fat/mister-magik/assets/media/{asset_key}.png"
        ));
    }
    let family_name = identity.family_id.split_once(':')?.1;
    let parent = format!("{}:{family_name}", identity.list_name);
    if let Some(asset_key) = assets.get(&parent) {
        return Some(format!(
            "/media/fat/mister-magik/assets/media/{asset_key}.png"
        ));
    }
    let family_key = (identity.list_name.clone(), family_name.to_string());
    for sibling in software_metadata
        .family_members
        .get(&family_key)
        .into_iter()
        .flatten()
    {
        let key = format!("{}:{sibling}", identity.list_name);
        if let Some(asset_key) = assets.get(&key) {
            return Some(format!(
                "/media/fat/mister-magik/assets/media/{asset_key}.png"
            ));
        }
    }
    None
}

fn write_sqlite_scan(
    path: &Path,
    scan: &LibraryScan,
    progress: ProgressCallback<'_>,
    software_hash_cache: SoftwareHashCache,
) -> Result<(), String> {
    let preview_asset_packs = preview_worker::preview_archive_indexes_from_env()
        .map_err(|e| format!("preview archive index: {e}"))?;
    write_sqlite_scan_with_sources(
        path,
        scan,
        &default_mame_sqlite_path(),
        &default_hbmame_sqlite_path(),
        &preview_asset_packs,
        progress,
        software_hash_cache,
    )
}

fn refresh_sqlite_preview_assets_from_env(path: &Path) -> Result<u64, String> {
    let preview_asset_packs = preview_worker::preview_archive_indexes_from_env()
        .map_err(|e| format!("preview archive index: {e}"))?;
    let preview_fingerprints = preview_worker::preview_archive_fingerprints_from_env()
        .map_err(|e| format!("preview archive fingerprint: {e}"))?;
    refresh_sqlite_preview_assets(path, &preview_asset_packs, preview_fingerprints)
}

fn refresh_sqlite_preview_assets(
    path: &Path,
    preview_asset_packs: &[preview_worker::PreviewArchiveIndex],
    preview_fingerprints: Vec<(String, u64, i64)>,
) -> Result<u64, String> {
    let mut conn = Connection::open(path).map_err(|e| format!("open sqlite: {e}"))?;
    for table in [
        "asset_packs",
        "asset_entries",
        "ui_arcade_preferred",
        "ui_arcade_variants",
        "launcher_catalog",
        "file_fingerprints",
    ] {
        if !sqlite_table_exists(&conn, table)? {
            return Err(format!("sqlite missing {table} table"));
        }
    }
    let mame_metadata = load_mame_machine_metadata(&default_mame_sqlite_path());
    let software_metadata = load_mame_software_metadata(&default_mame_sqlite_path());
    let tx = conn
        .transaction()
        .map_err(|e| format!("begin sqlite tx: {e}"))?;
    let preserved_launcher_rows = {
        let mut stmt = tx
            .prepare(
                "SELECT title,sort_title,launch_ref,image_path,has_image,system_id
                 FROM launcher_catalog
                 WHERE system_id NOT IN ('arcade','neogeo')
                 ORDER BY ordinal",
            )
            .map_err(|e| format!("prepare preserved launcher query: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(|e| format!("query preserved launcher rows: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("read preserved launcher row: {e}"))?;
        rows
    };

    tx.execute("DELETE FROM asset_entries", [])
        .map_err(|e| format!("delete asset entries: {e}"))?;
    tx.execute("DELETE FROM asset_packs", [])
        .map_err(|e| format!("delete asset packs: {e}"))?;
    tx.execute("DELETE FROM ui_arcade_variants", [])
        .map_err(|e| format!("delete arcade variants: {e}"))?;
    tx.execute("DELETE FROM ui_arcade_preferred", [])
        .map_err(|e| format!("delete arcade preferred: {e}"))?;

    register_preview_asset_packs(&tx, &mame_metadata, &software_metadata, preview_asset_packs)?;
    materialize_arcade_ui_projections(&tx)?;

    tx.execute("DELETE FROM launcher_catalog", [])
        .map_err(|e| format!("delete launcher catalog: {e}"))?;
    tx.execute(
        "INSERT INTO launcher_catalog(ordinal,title,sort_title,launch_ref,image_path,has_image,system_id)
         SELECT ordinal,title,sort_title,launch_ref,image_path,has_image,system_id
         FROM ui_arcade_preferred
         ORDER BY ordinal",
        [],
    )
    .map_err(|e| format!("insert refreshed arcade launcher catalog: {e}"))?;
    let ordinal_offset = tx
        .query_row("SELECT count(*) FROM launcher_catalog", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|e| format!("query launcher catalog offset: {e}"))?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO launcher_catalog(ordinal,title,sort_title,launch_ref,image_path,has_image,system_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(|e| format!("prepare preserved launcher insert: {e}"))?;
        for (idx, row) in preserved_launcher_rows.iter().enumerate() {
            stmt.execute(params![
                ordinal_offset + idx as i64,
                row.0.as_str(),
                row.1.as_str(),
                row.2.as_str(),
                row.3.as_str(),
                row.4,
                row.5.as_str()
            ])
            .map_err(|e| format!("insert preserved launcher row: {e}"))?;
        }
    }
    refresh_console_launcher_images(&tx)?;

    let existing_preview_fingerprints = {
        let mut stmt = tx
            .prepare("SELECT file_path FROM file_fingerprints")
            .map_err(|e| format!("prepare file fingerprint query: {e}"))?;
        let paths = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("query file fingerprints: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("read file fingerprint row: {e}"))?
            .into_iter()
            .filter(|path| is_preview_archive_fingerprint_path(path))
            .collect::<Vec<_>>();
        paths
    };
    {
        let mut stmt = tx
            .prepare("DELETE FROM file_fingerprints WHERE file_path=?1")
            .map_err(|e| format!("prepare preview fingerprint delete: {e}"))?;
        for path in existing_preview_fingerprints {
            stmt.execute(params![path])
                .map_err(|e| format!("delete preview fingerprint: {e}"))?;
        }
    }
    for (path, size, mtime_secs) in preview_fingerprints {
        tx.execute(
            "INSERT OR REPLACE INTO file_fingerprints(file_path,size,mtime_secs) VALUES (?1,?2,?3)",
            params![path, size as i64, mtime_secs],
        )
        .map_err(|e| format!("insert preview fingerprint: {e}"))?;
    }
    tx.commit()
        .map_err(|e| format!("commit preview asset refresh: {e}"))?;
    Ok(std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
}

fn refresh_console_launcher_images(tx: &rusqlite::Transaction<'_>) -> Result<(), String> {
    tx.execute_batch(
        r#"
        UPDATE launcher_catalog
        SET
            image_path = COALESCE((
                SELECT '/media/fat/mister-magik/assets/media/' || resolved.asset_key || '.png'
                FROM (
                    SELECT exact.asset_key AS asset_key, 0 AS rank
                    FROM launchables l
                    JOIN launchable_identities i
                      ON i.launchable_id = l.launchable_id
                     AND i.namespace = 'mame-software'
                    JOIN asset_entries exact
                      ON exact.identity_namespace = 'mame-software'
                     AND exact.identity_id = i.identity_id
                    WHERE l.launch_ref = launcher_catalog.launch_ref
                    UNION ALL
                    SELECT parent.asset_key AS asset_key, 1 AS rank
                    FROM launchables l
                    JOIN launchable_identities i
                      ON i.launchable_id = l.launchable_id
                     AND i.namespace = 'mame-software'
                    JOIN asset_entries parent
                      ON parent.identity_namespace = 'mame-software'
                     AND parent.identity_id = i.family_id
                    WHERE l.launch_ref = launcher_catalog.launch_ref
                    UNION ALL
                    SELECT sibling.asset_key AS asset_key, 2 AS rank
                    FROM launchables l
                    JOIN launchable_identities i
                      ON i.launchable_id = l.launchable_id
                     AND i.namespace = 'mame-software'
                    JOIN asset_entries sibling
                      ON sibling.identity_namespace = 'mame-software'
                     AND sibling.family_id = i.family_id
                    WHERE l.launch_ref = launcher_catalog.launch_ref
                    ORDER BY rank, asset_key
                    LIMIT 1
                ) resolved
            ), ''),
            has_image = CASE
                WHEN EXISTS (
                    SELECT 1
                    FROM launchables l
                    JOIN launchable_identities i
                      ON i.launchable_id = l.launchable_id
                     AND i.namespace = 'mame-software'
                    JOIN asset_entries a
                      ON a.identity_namespace = 'mame-software'
                     AND (a.identity_id = i.identity_id OR a.identity_id = i.family_id OR a.family_id = i.family_id)
                    WHERE l.launch_ref = launcher_catalog.launch_ref
                )
                THEN 1
                ELSE 0
            END
        WHERE system_id IN ('nes','snes','n64','sms','megadrive','saturn');
        "#,
    )
    .map_err(|e| format!("refresh console launcher images: {e}"))
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
        mame_sqlite_path,
        &PathBuf::new(),
        &[],
        None,
        SoftwareHashCache::load(path),
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
        mame_sqlite_path,
        hbmame_sqlite_path,
        &[],
        None,
        SoftwareHashCache::load(path),
    )
}

#[cfg(test)]
fn write_sqlite_scan_with_mame_and_preview_pack(
    path: &Path,
    scan: &LibraryScan,
    mame_sqlite_path: &Path,
    preview_asset_pack: &preview_worker::PreviewArchiveIndex,
) -> Result<(), String> {
    write_sqlite_scan_with_sources(
        path,
        scan,
        mame_sqlite_path,
        &PathBuf::new(),
        std::slice::from_ref(preview_asset_pack),
        None,
        SoftwareHashCache::load(path),
    )
}

fn write_sqlite_scan_with_sources(
    path: &Path,
    scan: &LibraryScan,
    mame_sqlite_path: &Path,
    hbmame_sqlite_path: &Path,
    preview_asset_packs: &[preview_worker::PreviewArchiveIndex],
    mut progress: ProgressCallback<'_>,
    mut software_hash_cache: SoftwareHashCache,
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
        CREATE TABLE files (
            path TEXT PRIMARY KEY,
            size INTEGER NOT NULL,
            mtime_secs INTEGER NOT NULL,
            extension TEXT NOT NULL,
            role TEXT NOT NULL,
            profile_id TEXT
        ) WITHOUT ROWID;
        CREATE TABLE launchers (
            launcher_id TEXT PRIMARY KEY,
            file_path TEXT NOT NULL,
            launcher_kind TEXT NOT NULL,
            profile_id TEXT,
            title TEXT NOT NULL,
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
        CREATE TABLE ignored_files (
            file_path TEXT NOT NULL,
            profile_id TEXT,
            reason TEXT NOT NULL,
            source_kind TEXT NOT NULL,
            source_detail TEXT NOT NULL,
            PRIMARY KEY(file_path, profile_id, reason)
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
            year INTEGER,
            image_path TEXT,
            has_image INTEGER NOT NULL
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
        CREATE TABLE asset_packs (
            pack_id TEXT PRIMARY KEY,
            platform_id TEXT NOT NULL,
            asset_type TEXT NOT NULL,
            local_path TEXT NOT NULL,
            codec TEXT,
            version TEXT
        ) WITHOUT ROWID;
        CREATE TABLE asset_entries (
            pack_id TEXT NOT NULL,
            asset_key TEXT NOT NULL,
            identity_namespace TEXT,
            identity_id TEXT,
            family_id TEXT,
            width INTEGER,
            height INTEGER,
            PRIMARY KEY(pack_id, asset_key)
        ) WITHOUT ROWID;
        CREATE TABLE ui_arcade_preferred (
            ordinal INTEGER PRIMARY KEY,
            launchable_id TEXT NOT NULL,
            title TEXT NOT NULL,
            sort_title TEXT NOT NULL,
            launch_ref TEXT NOT NULL,
            image_path TEXT NOT NULL,
            has_image INTEGER NOT NULL,
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
            image_path TEXT NOT NULL,
            has_image INTEGER NOT NULL,
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
            image_path TEXT NOT NULL,
            has_image INTEGER NOT NULL,
            system_id TEXT NOT NULL
        );
        CREATE TABLE region_metadata (
            game_id TEXT PRIMARY KEY,
            inferred_region TEXT,
            confidence TEXT NOT NULL,
            override_region TEXT
        ) WITHOUT ROWID;
        CREATE VIRTUAL TABLE games_fts USING fts5(
            game_id UNINDEXED,
            title,
            launch_ref,
            system_id,
            core_id,
            hardware_id
        );
        CREATE TABLE meta (
            key TEXT PRIMARY KEY,
            value INTEGER NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE file_fingerprints (
            file_path TEXT PRIMARY KEY,
            size INTEGER NOT NULL,
            mtime_secs INTEGER NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE container_fingerprints (
            file_path TEXT PRIMARY KEY,
            size INTEGER NOT NULL,
            mtime_secs INTEGER NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE directory_manifest (
            dir_path TEXT PRIMARY KEY,
            dir_size INTEGER NOT NULL,
            dir_mtime_secs INTEGER NOT NULL,
            child_count INTEGER NOT NULL,
            hash INTEGER NOT NULL
        ) WITHOUT ROWID;
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
    .map_err(|e| format!("create sqlite schema: {e}"))?;
    report_library_import_timing("schema", schema_t, "tables=22");

    let metadata_t = Instant::now();
    let mame_signature = file_signature(mame_sqlite_path);
    let hbmame_signature = file_signature(hbmame_sqlite_path);
    let mame_metadata = load_mame_machine_metadata(mame_sqlite_path);
    let software_metadata = load_mame_software_metadata(mame_sqlite_path);
    let console_assets = console_preview_assets(preview_asset_packs);
    let arcade_metadata = load_arcade_machine_metadata(mame_sqlite_path, hbmame_sqlite_path);
    report_library_import_timing(
        "metadata_load",
        metadata_t,
        format!(
            "mame={} hbmame={} software_lists={} preview_packs={}",
            arcade_metadata.mame.len(),
            arcade_metadata.hbmame.len(),
            software_metadata.items.len(),
            preview_asset_packs.len()
        ),
    );
    let tx_t = Instant::now();
    let tx = conn
        .transaction()
        .map_err(|e| format!("begin sqlite tx: {e}"))?;
    register_preview_asset_packs(&tx, &mame_metadata, &software_metadata, preview_asset_packs)?;
    report_library_import_timing("begin_tx_asset_packs", tx_t, "");
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
        let normal_paths = scan
            .normal_files
            .iter()
            .map(|payload| payload.path.clone())
            .collect::<HashSet<_>>();
        let container_paths = scan
            .containers
            .iter()
            .map(|container| container.file_path.clone())
            .collect::<HashSet<_>>();
        let mut stmt = tx
            .prepare(
                "INSERT INTO files(path,size,mtime_secs,extension,role,profile_id)
                 VALUES (?1,?2,?3,?4,?5,?6)",
            )
            .map_err(|e| format!("prepare file fact insert: {e}"))?;
        for (path, (size, mtime_secs)) in &scan.file_fingerprints {
            let role = if container_paths.contains(path) {
                "container"
            } else if normal_paths.contains(path) {
                "candidate"
            } else {
                "fact"
            };
            stmt.execute(params![
                path.as_str(),
                *size as i64,
                *mtime_secs,
                path_ext(path).unwrap_or_default(),
                role,
                Option::<&str>::None
            ])
            .map_err(|e| format!("insert file fact: {e}"))?;
        }
        report_library_import_timing(
            "insert_files",
            stage_t,
            format!("rows={}", scan.file_fingerprints.len()),
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
            let (size, mtime_secs) = scan.file_fingerprints.get(path).copied().unwrap_or((0, 0));
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
                size as i64,
                mtime_secs,
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
            format!("normal_files={} entries={}", scan.normal_files.len(), scan.entries.len()),
        );
    }
    {
        let stage_t = Instant::now();
        let mut stmt = tx
            .prepare(
                "INSERT INTO ignored_files(file_path,profile_id,reason,source_kind,source_detail)
                 VALUES (?1,?2,?3,?4,?5)",
            )
            .map_err(|e| format!("prepare ignored file insert: {e}"))?;
        for ignored in &scan.ignored_files {
            stmt.execute(params![
                ignored.path.as_str(),
                ignored.profile_id.as_str(),
                ignore_reason_str(ignored.reason),
                source_kind_name(ignored.provenance.kind),
                ignored.provenance.detail
            ])
            .map_err(|e| format!("insert ignored file: {e}"))?;
        }
        report_library_import_timing(
            "insert_ignored_files",
            stage_t,
            format!("rows={}", scan.ignored_files.len()),
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
                "INSERT INTO games(game_id,title,sort_title,system_id,manufacturer,genre,year,image_path,has_image)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
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
        let mut fts_stmt = tx
            .prepare(
                "INSERT INTO games_fts(game_id,title,launch_ref,system_id,core_id,hardware_id)
                 VALUES (?1,?2,?3,?4,?5,?6)",
            )
            .map_err(|e| format!("prepare game fts insert: {e}"))?;
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
            let software_identity =
                mame_software_identity_for_discovery(
                    discovery,
                    &software_metadata,
                    &mut software_hash_cache,
                );
            let software_image_path = software_identity.as_ref().and_then(|identity| {
                console_preview_image_path(identity, &software_metadata, &console_assets)
            });
            let game_image_path = software_image_path
                .as_deref()
                .or(discovery.image_path.as_deref());
            let game_has_image = software_image_path.is_some() || discovery.has_image;
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
                    discovery.year.map(|n| n as i64),
                    game_image_path,
                    if game_has_image { 1 } else { 0 }
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
            if is_launcher_launch_ref(&plan_launch_ref) {
                if system_id != "arcade" && system_id != "neogeo" {
                    launcher_rows.push(CatalogRow {
                        game: ArcadeGameEntry {
                            title: discovery.title.clone().into(),
                            mra_path: plan_launch_ref.clone().into(),
                            image_path: game_image_path.unwrap_or_default().into(),
                            has_image: game_has_image,
                            system_id: system_id.clone().into(),
                        },
                        source_kind: launch_kind_for_discovery(discovery).to_string(),
                        setname: discovery.setname.clone().unwrap_or_default(),
                        parent: discovery.parent.clone().unwrap_or_default(),
                    });
                }
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
                let (family_id, title, year, manufacturer, source) =
                    mame_identity_projection(&identity_id, &arcade_metadata);
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
            let region = infer_region_metadata(discovery);
            let region = if let Some(identity) = software_identity.as_ref() {
                if let Some(region) = identity.region.as_deref().and_then(canonical_region_static) {
                    RegionInference {
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
            fts_stmt
                .execute(params![
                    key.as_str(),
                    discovery.title.as_str(),
                    plan_launch_ref.as_str(),
                    system_id.as_str(),
                    discovery.core_id.as_str(),
                    discovery.hardware_id.as_str()
                ])
                .map_err(|e| format!("insert game fts: {e}"))?;
            let written = idx + 1;
            if written % 1000 == 0 || written == discovery_total {
                report_library_import_timing(
                    "insert_games_chunk",
                    chunk_t,
                    format!("from={} to={} total={discovery_total}", chunk_start, written),
                );
                chunk_t = Instant::now();
                chunk_start = written;
            }
        }
        report_sqlite_import_progress(&mut progress, discovery_total, discovery_total);
        drop(fts_stmt);
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
        materialize_arcade_ui_projections(&tx)?;
        report_library_import_timing("materialize_arcade_ui", projection_t, "");
        let launcher_arcade_t = Instant::now();
        tx.execute(
            "INSERT INTO launcher_catalog(ordinal,title,sort_title,launch_ref,image_path,has_image,system_id)
             SELECT ordinal,title,sort_title,launch_ref,image_path,has_image,system_id
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
                "INSERT INTO launcher_catalog(ordinal,title,sort_title,launch_ref,image_path,has_image,system_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(|e| format!("prepare launcher catalog insert: {e}"))?;
        for (idx, game) in launcher_games.iter().enumerate() {
            launcher_stmt
                .execute(params![
                    ordinal_offset + idx as i64,
                    game.title.as_ref(),
                    normalize_title(&game.title),
                    game.mra_path.as_ref(),
                    game.image_path.as_ref(),
                    if game.has_image { 1 } else { 0 },
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
            .prepare(
                "INSERT INTO launchers(launcher_id,file_path,launcher_kind,profile_id,title,source_kind,source_detail)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(|e| format!("prepare launcher insert: {e}"))?;
        let mut seen = HashSet::<String>::new();
        for discovery in &scan.discoveries {
            if !matches!(
                discovery.source_kind,
                DiscoverySourceKind::Mra | DiscoverySourceKind::Mgl
            ) {
                continue;
            }
            if !seen.insert(discovery.launch_ref.clone()) {
                continue;
            }
            stmt.execute(params![
                format!("launcher:{}", discovery.launch_ref),
                discovery.launch_ref.as_str(),
                source_kind_str(discovery.source_kind),
                Option::<&str>::None,
                discovery.title.as_str(),
                source_kind_str(discovery.source_kind),
                "legacy launcher discovery before profile classification"
            ])
            .map_err(|e| format!("insert launcher: {e}"))?;
        }
        report_library_import_timing("insert_launchers", stage_t, "");
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
        report_library_import_timing("insert_meta", stage_t, "rows=10");
    }
    {
        let stage_t = Instant::now();
        let mut stmt = tx
            .prepare("INSERT INTO file_fingerprints(file_path,size,mtime_secs) VALUES (?1,?2,?3)")
            .map_err(|e| format!("prepare file fingerprint insert: {e}"))?;
        for (path, (size, mtime_secs)) in &scan.file_fingerprints {
            stmt.execute(params![path.as_str(), *size as i64, *mtime_secs])
                .map_err(|e| format!("insert file fingerprint: {e}"))?;
        }
        report_library_import_timing(
            "insert_file_fingerprints",
            stage_t,
            format!("rows={}", scan.file_fingerprints.len()),
        );
    }
    {
        let stage_t = Instant::now();
        let mut stmt = tx
            .prepare(
                "INSERT INTO container_fingerprints(file_path,size,mtime_secs) VALUES (?1,?2,?3)",
            )
            .map_err(|e| format!("prepare container fingerprint insert: {e}"))?;
        for container in &scan.containers {
            stmt.execute(params![
                container.file_path.as_str(),
                container.size as i64,
                container.mtime_secs
            ])
            .map_err(|e| format!("insert fingerprint: {e}"))?;
        }
        report_library_import_timing(
            "insert_container_fingerprints",
            stage_t,
            format!("rows={}", scan.containers.len()),
        );
    }
    {
        let stage_t = Instant::now();
        let mut stmt = tx
            .prepare("INSERT INTO directory_manifest(dir_path,dir_size,dir_mtime_secs,child_count,hash) VALUES (?1,?2,?3,?4,?5)")
            .map_err(|e| format!("prepare directory manifest insert: {e}"))?;
        for (dir, sig) in &scan.directory_manifest {
            stmt.execute(params![
                dir.as_str(),
                sig.dir_size as i64,
                sig.dir_mtime_secs,
                sig.child_count as i64,
                sig.hash as i64
            ])
            .map_err(|e| format!("insert directory manifest: {e}"))?;
        }
        report_library_import_timing(
            "insert_directory_manifest",
            stage_t,
            format!("rows={}", scan.directory_manifest.len()),
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
        for (key, software_name) in &software_hash_cache.entries {
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
            format!("rows={}", software_hash_cache.entries.len()),
        );
    }
    let commit_t = Instant::now();
    tx.commit().map_err(|e| format!("commit sqlite tx: {e}"))?;
    report_library_import_timing("commit", commit_t, "");
    report_library_import_timing("total", total_t, format!("path={}", path.display()));
    Ok(())
}

fn report_library_import_timing(
    stage: &str,
    started: Instant,
    detail: impl std::fmt::Display,
) {
    println!(
        "library_import_timing\t{stage}\t{}\t{detail}",
        started.elapsed().as_micros()
    );
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

fn read_sqlite_fingerprint(path: &Path) -> Option<DbFingerprint> {
    let conn = Connection::open(path).ok()?;
    if sqlite_meta_usize(&conn, "version")? != SCHEMA_VERSION as usize {
        return None;
    }
    let mut file_fingerprints = BTreeMap::new();
    let mut file_stmt = conn
        .prepare("SELECT file_path,size,mtime_secs FROM file_fingerprints")
        .ok()?;
    let file_rows = file_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .ok()?;
    for row in file_rows {
        let (path, size, mtime) = row.ok()?;
        file_fingerprints.insert(path, (size.max(0) as u64, mtime));
    }

    let mut container_fingerprints = BTreeMap::new();
    let mut stmt = conn
        .prepare("SELECT file_path,size,mtime_secs FROM container_fingerprints")
        .ok()?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .ok()?;
    for row in rows {
        let (path, size, mtime) = row.ok()?;
        container_fingerprints.insert(path, (size.max(0) as u64, mtime));
    }
    let mut directory_manifest = BTreeMap::new();
    let mut dir_stmt = conn
        .prepare("SELECT dir_path,dir_size,dir_mtime_secs,child_count,hash FROM directory_manifest")
        .ok()?;
    let dir_rows = dir_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .ok()?;
    for row in dir_rows {
        let (path, dir_size, dir_mtime, child_count, hash) = row.ok()?;
        directory_manifest.insert(
            path,
            DirectorySignature {
                dir_size: dir_size.max(0) as u64,
                dir_mtime_secs: dir_mtime,
                child_count: child_count.max(0) as u64,
                hash: hash as u64,
            },
        );
    }
    Some(DbFingerprint {
        normal_files: sqlite_meta_usize(&conn, "normal_files")?,
        containers: sqlite_meta_usize(&conn, "containers")?,
        entries: sqlite_meta_usize(&conn, "entries")?,
        discoveries: sqlite_meta_usize(&conn, "discoveries")?,
        mame_metadata: FileSignature {
            size: sqlite_meta_i64(&conn, "mame_metadata_size")?.max(0) as u64,
            mtime_secs: sqlite_meta_i64(&conn, "mame_metadata_mtime")?,
        },
        hbmame_metadata: FileSignature {
            size: sqlite_meta_i64(&conn, "hbmame_metadata_size")?.max(0) as u64,
            mtime_secs: sqlite_meta_i64(&conn, "hbmame_metadata_mtime")?,
        },
        file_fingerprints,
        container_fingerprints,
        directory_manifest,
    })
}

fn sqlite_meta_i64(conn: &Connection, key: &str) -> Option<i64> {
    conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| {
        r.get::<_, i64>(0)
    })
    .ok()
}

fn sqlite_meta_usize(conn: &Connection, key: &str) -> Option<usize> {
    conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| {
        r.get::<_, i64>(0)
    })
    .ok()
    .map(|n| n.max(0) as usize)
}

fn discovery_unique_key(d: &GameDiscovery) -> String {
    match d.source_kind {
        DiscoverySourceKind::Mra => {
            if let Some(setname) = d.setname.as_deref().filter(|s| !s.trim().is_empty()) {
                format!("mra:set:{setname}")
            } else {
                format!("mra:title:{}:{}", d.hardware_id, normalize_id(&d.title))
            }
        }
        DiscoverySourceKind::Mgl => format!("payload:{}", d.launch_ref),
        DiscoverySourceKind::PayloadFile => format!("payload:{}", d.launch_ref),
        DiscoverySourceKind::ArchiveEntry => format!("archive:{}", d.launch_ref),
        DiscoverySourceKind::CatalogEntry => format!("catalog:{}:{}", d.launch_ref, d.title),
    }
}

#[cfg(test)]
fn is_playable_discovery(d: &GameDiscovery) -> bool {
    is_playable_discovery_with_coverage(d, &HashSet::new())
}

fn is_playable_discovery_with_coverage(
    d: &GameDiscovery,
    covered_payloads: &HashSet<String>,
) -> bool {
    match d.source_kind {
        DiscoverySourceKind::Mra => true,
        DiscoverySourceKind::Mgl => is_launcher_launch_ref(&d.launch_ref),
        DiscoverySourceKind::PayloadFile | DiscoverySourceKind::ArchiveEntry => {
            !covered_payloads.contains(&normalize_launch_path(&d.launch_ref))
        }
        DiscoverySourceKind::CatalogEntry => is_launcher_launch_ref(&d.launch_ref),
    }
}

fn covered_payload_paths(discoveries: &[GameDiscovery]) -> HashSet<String> {
    let mut covered = HashSet::new();
    for discovery in discoveries {
        if discovery.source_kind != DiscoverySourceKind::Mgl {
            continue;
        }
        let path = Path::new(&discovery.source_path);
        let Some(mgl) = read_mgl_metadata(path) else {
            continue;
        };
        let Some(payload) = mgl.file_path.as_deref() else {
            continue;
        };
        let resolved = resolve_mgl_payload_path(path, payload);
        covered.insert(normalize_launch_path(&resolved.display().to_string()));
    }
    covered
}

fn launch_kind_for_discovery(discovery: &GameDiscovery) -> &'static str {
    match discovery.source_kind {
        DiscoverySourceKind::Mra => "mra",
        DiscoverySourceKind::Mgl => "mgl",
        DiscoverySourceKind::PayloadFile | DiscoverySourceKind::ArchiveEntry => "virtual-mgl",
        DiscoverySourceKind::CatalogEntry => "catalog-entry",
    }
}

fn launch_ref_for_discovery(game_id: &str, discovery: &GameDiscovery) -> String {
    match discovery.source_kind {
        DiscoverySourceKind::Mra | DiscoverySourceKind::Mgl | DiscoverySourceKind::CatalogEntry => {
            discovery.launch_ref.clone()
        }
        DiscoverySourceKind::PayloadFile | DiscoverySourceKind::ArchiveEntry => {
            virtual_launch_ref(game_id)
        }
    }
}

fn virtual_launch_ref(game_id: &str) -> String {
    format!("magik-plan:{game_id}")
}

fn amigavision_game_launch_ref(title: &str) -> String {
    format!(
        "{AMIGAVISION_GAME_LAUNCH_PREFIX}{}",
        encode_launch_component(title)
    )
}

fn encode_launch_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn profile_id_for_discovery(discovery: &GameDiscovery) -> Option<&str> {
    if discovery.platform_id == "unknown" || discovery.platform_id.is_empty() {
        None
    } else {
        Some(discovery.platform_id.as_str())
    }
}

fn is_launcher_launch_ref(path: &str) -> bool {
    if path.starts_with("magik-plan:")
        || path.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX)
        || path == AMIGAVISION_LAUNCHER_REF
    {
        return true;
    }
    match path_ext(path).as_deref() {
        Some("mra" | "mgl") => !path.contains("::"),
        _ => false,
    }
}

fn is_amigavision_archive_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/games/amiga/") && lower.contains("amigavision") && lower.ends_with(".7z")
}

fn is_amigavision_installed_hdf_path(path: &Path) -> bool {
    normalize_match_path(&path.display().to_string()).ends_with("/games/amiga/amigavision.hdf")
}

fn is_amigavision_save_media_path(path: &Path) -> bool {
    normalize_match_path(&path.display().to_string())
        .ends_with("/games/amiga/amigavision-saves.hdf")
}

fn is_amigavision_listing_path(path: &Path) -> bool {
    let path = normalize_match_path(&path.display().to_string());
    path.ends_with("/games/amiga/listings/games.txt")
        || path.ends_with("/games/amiga/listings/demos.txt")
}

fn source_kind_str(kind: DiscoverySourceKind) -> &'static str {
    match kind {
        DiscoverySourceKind::Mra => "mra",
        DiscoverySourceKind::Mgl => "mgl",
        DiscoverySourceKind::PayloadFile => "payload",
        DiscoverySourceKind::ArchiveEntry => "archive-entry",
        DiscoverySourceKind::CatalogEntry => "catalog-entry",
    }
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

fn ignore_reason_str(reason: IgnoreReason) -> &'static str {
    match reason {
        IgnoreReason::Bios => "bios",
        IgnoreReason::CueTrack => "cue-track",
        IgnoreReason::CoreBinary => "core-binary",
        IgnoreReason::SaveMedia => "save-media",
        IgnoreReason::SupportArchive => "support-archive",
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RegionInference {
    region: Option<&'static str>,
    confidence: &'static str,
}

fn infer_region_metadata(discovery: &GameDiscovery) -> RegionInference {
    if discovery.platform_id != "saturn" {
        return RegionInference {
            region: None,
            confidence: "unknown",
        };
    }

    if let Some(region) = region_from_saturn_boot_header_file(&discovery.source_path) {
        return RegionInference {
            region: Some(region),
            confidence: "disc-header",
        };
    }
    if let Some(region) = region_from_filename(&discovery.source_path) {
        return RegionInference {
            region: Some(region),
            confidence: "filename-high",
        };
    }
    if let Some(region) = region_from_folder(&discovery.source_path) {
        return RegionInference {
            region: Some(region),
            confidence: "folder-medium",
        };
    }
    if let Some(region) = region_from_text(&discovery.title) {
        return RegionInference {
            region: Some(region),
            confidence: "metadata-low",
        };
    }

    RegionInference {
        region: None,
        confidence: "unknown",
    }
}

fn region_from_saturn_boot_header_file(path: &str) -> Option<&'static str> {
    let path = path.split("::").next().unwrap_or(path);
    let mut file = File::open(path).ok()?;
    let mut header = [0u8; 256];
    file.read_exact(&mut header).ok()?;
    parse_saturn_boot_header(&header)?.region
}

fn region_from_filename(path: &str) -> Option<&'static str> {
    let stem = Path::new(path.split("::").next().unwrap_or(path))
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path);
    region_from_text(stem)
}

fn region_from_folder(path: &str) -> Option<&'static str> {
    Path::new(path.split("::").next().unwrap_or(path))
        .parent()?
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .rev()
        .find_map(region_from_text)
}

fn region_from_text(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    let token = lower.trim_matches(|ch: char| !ch.is_ascii_alphanumeric());
    if matches!(token, "usa" | "us" | "u") {
        return Some("usa");
    }
    if matches!(token, "europe" | "eu" | "e") {
        return Some("europe");
    }
    if matches!(token, "japan" | "jp" | "j") {
        return Some("japan");
    }
    if matches!(token, "world" | "w") {
        return Some("world");
    }
    if contains_any(
        &lower,
        &["(usa", "(us)", "(u)", "[usa", "[us]", " usa", " ntsc-u"],
    ) {
        Some("usa")
    } else if contains_any(
        &lower,
        &[
            "(europe", "(eu", "(e)", "[europe", "[eu]", " europe", " pal",
        ],
    ) {
        Some("europe")
    } else if contains_any(
        &lower,
        &[
            "(japan", "(jp", "(j)", "[japan", "[jp]", " japan", " ntsc-j",
        ],
    ) {
        Some("japan")
    } else if contains_any(&lower, &["(world", "(w)", "[world", " world"]) {
        Some("world")
    } else {
        None
    }
}

fn canonical_region_static(region: &str) -> Option<&'static str> {
    match region.trim().to_ascii_lowercase().as_str() {
        "usa" | "us" => Some("usa"),
        "europe" | "eu" => Some("europe"),
        "japan" | "jp" => Some("japan"),
        "korea" | "kr" => Some("korea"),
        "world" => Some("world"),
        "unknown" => Some("unknown"),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SaturnBootHeader {
    product_id: Option<String>,
    region: Option<&'static str>,
}

fn parse_saturn_boot_header(bytes: &[u8]) -> Option<SaturnBootHeader> {
    if bytes.len() < 0x50 || !bytes.starts_with(b"SEGA SEGASATURN") {
        return None;
    }
    let product_id = ascii_trim(&bytes[0x20..0x2a]);
    let area = String::from_utf8_lossy(&bytes[0x40..0x50]).to_ascii_uppercase();
    let region = if area.contains('U') {
        Some("usa")
    } else if area.contains('E') {
        Some("europe")
    } else if area.contains('J') {
        Some("japan")
    } else if area.contains('K') {
        Some("korea")
    } else {
        None
    };
    Some(SaturnBootHeader { product_id, region })
}

fn ascii_trim(bytes: &[u8]) -> Option<String> {
    let value = String::from_utf8_lossy(bytes)
        .trim_matches(|ch: char| ch.is_ascii_whitespace() || ch == '\0')
        .to_string();
    (!value.is_empty()).then_some(value)
}

fn catalog_system_id_for_discovery(discovery: &GameDiscovery) -> String {
    if discovery.platform_id == "arcade"
        || (discovery.category == "Arcade" && discovery.source_kind == DiscoverySourceKind::Mra)
    {
        "arcade".to_string()
    } else if discovery.platform_id.is_empty() {
        "unknown".to_string()
    } else {
        discovery.platform_id.clone()
    }
}

fn system_title_for_discovery(_discovery: &GameDiscovery, system_id: &str) -> String {
    arcade_catalog::system_title(system_id)
}

fn is_index_candidate(profiles: &[LaunchProfile], path: &Path, _ext: &str) -> bool {
    matches!(
        classify_profile_path(profiles, path),
        Some((
            _,
            ProfilePathClass::Payload { .. }
                | ProfilePathClass::Collection { .. }
                | ProfilePathClass::Ignored { .. }
        ))
    ) || is_amigavision_listing_path(path)
}

fn should_ignore_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy().to_ascii_lowercase();
    if path_str.contains("/.____padding_file/") || path_str.contains("/__macosx/") {
        return true;
    }
    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s.starts_with("._")
            || s == ".____padding_file"
            || s.eq_ignore_ascii_case("images")
            || s.eq_ignore_ascii_case("manuals")
            || s.eq_ignore_ascii_case("screenshot")
            || s.eq_ignore_ascii_case("screenshots")
            || s.eq_ignore_ascii_case("screenshot-magik")
            || s.eq_ignore_ascii_case("_organized")
            || s.eq_ignore_ascii_case("boxart")
    })
}

fn normalize_title(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_ascii_lowercase()
}

fn title_from_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .trim()
        .to_string()
}

fn path_ext(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

fn find_eocd(buf: &[u8]) -> Option<usize> {
    if buf.len() < 22 {
        return None;
    }
    (0..=buf.len() - 22)
        .rev()
        .find(|&idx| buf[idx..idx + 4] == [0x50, 0x4b, 0x05, 0x06])
}

fn le_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn be_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn le_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
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

    #[test]
    fn profile_ignored_support_files_do_not_become_payloads() {
        let profiles = launch_profiles::builtin_profiles();

        assert!(matches!(
            classify_profile_path(&profiles, Path::new("/media/fat/games/Saturn/boot.rom")),
            Some((profile, ProfilePathClass::Ignored { reason: IgnoreReason::Bios, .. }))
                if profile.id == "saturn"
        ));
        assert!(matches!(
            classify_profile_path(&profiles, Path::new("/media/fat/games/AO486/boot1.rom")),
            Some((profile, ProfilePathClass::Ignored { reason: IgnoreReason::Bios, .. }))
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
            image_path: None,
            has_image: false,
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
        assert!(!scan.discoveries[0].has_image);
        assert!(scan
            .file_fingerprints
            .contains_key(&nes_dir.join("Mario.nes").display().to_string()));
        assert!(!scan
            .file_fingerprints
            .contains_key(&nes_dir.join("gamelist.xml").display().to_string()));
        assert!(!scan.directory_manifest.keys().any(|path| Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "screenshot")));
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
        assert!(scan
            .file_fingerprints
            .contains_key(&arcade_dir.join("Diamond Run.mra").display().to_string()));
        assert!(!scan
            .file_fingerprints
            .contains_key(&organized_dir.join("Diamond Run.mra").display().to_string()));
        assert!(!scan.directory_manifest.keys().any(|path| Path::new(path)
            .components()
            .any(|component| component.as_os_str() == "_Organized")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn preview_images_use_archive_stems_without_screenshot_stat() {
        let mut discovery = mra_discovery(1, "Moon Patrol (US)");
        discovery.setname = Some("mpatrol".to_string());
        let preview_images = PreviewImageIndex::arcade(&["mpatrol"]);

        attach_preview_image(&mut discovery, &preview_images);

        assert!(discovery.has_image);
        assert_eq!(
            discovery.image_path.as_deref(),
            Some("/media/fat/_Arcade/media/screenshot/mpatrol.png")
        );
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
        let row: (String, String, String, i64, String) = conn
            .query_row(
                "SELECT i.namespace,i.identity_id,i.family_id,l.has_image,r.confidence
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
        metadata
            .title_index
            .insert(("snes".to_string(), "example-game".to_string()), vec!["example".to_string()]);
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
        metadata
            .hash_index
            .insert(("snes".to_string(), 11, crc32(b"fixture-rom")), vec!["fixture".to_string()]);
        let discovery = payload(&rom_path.display().to_string());

        let mut cache = SoftwareHashCache::default();
        let matched =
            match_software_by_file_hash_with_cache(&discovery, "snes", &metadata, false, &mut cache);

        assert_eq!(matched, None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn software_identity_hash_match_can_be_enabled() {
        let root = unique_temp_dir("software-hash-enabled");
        let rom_path = root.join("Fixture.sfc");
        std::fs::write(&rom_path, b"fixture-rom").expect("write rom");
        let mut metadata = MameSoftwareMetadata::default();
        metadata
            .hash_index
            .insert(("snes".to_string(), 11, crc32(b"fixture-rom")), vec!["fixture".to_string()]);
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
        let row: (String, i64, String) = conn
            .query_row(
                "SELECT image_path,has_image,system_id FROM launcher_catalog",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("launcher row");

        assert_eq!(row.1, 1);
        assert_eq!(row.2, "snes");
        assert!(row.0.ends_with("mame-software__snes__parent.png"));
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
    fn console_preview_ignores_noncanonical_console_pack_entries() {
        let root = unique_temp_dir("noncanonical-console-preview");
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
        let row: (String, i64, i64) = conn
            .query_row(
                "SELECT l.image_path,l.has_image,(
                    SELECT count(*)
                    FROM asset_entries
                    WHERE asset_key='albert-odyssey-legend-of-eldean-us'
                 )
                 FROM launcher_catalog l
                 WHERE l.system_id='saturn'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("launcher and asset row");

        assert_eq!(row, (String::new(), 0, 0));
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
        let row: (i64, i64) = conn
            .query_row(
                "SELECT has_image,(SELECT count(*) FROM asset_entries) FROM launcher_catalog",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("launcher row");

        assert_eq!(row, (0, 0));
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
        clone.has_image = true;
        clone.image_path = Some("/media/fat/_Arcade/media/screenshot/1942b.png".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![clone, parent]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let preferred = conn
            .query_row(
                "SELECT identity_id,family_id,preferred_reason,title,has_image
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
        world.has_image = true;
        world.image_path = Some("/media/fat/_Arcade/media/screenshot/1942w.png".to_string());

        write_sqlite_scan_with_mame(
            &db,
            &sqlite_scan_with_discoveries(vec![first, world]),
            &mame_db,
        )
        .expect("save sqlite");

        let conn = Connection::open(&db).expect("open library sqlite");
        let preferred = conn
            .query_row(
                "SELECT identity_id,family_id,preferred_reason,has_image
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

        assert_eq!(preferred.0.as_deref(), Some("1942w"));
        assert_eq!(preferred.1, "1942");
        assert_eq!(preferred.2, "deterministic-child");
        assert_eq!(preferred.3, 1);
        assert_eq!(variant_count, 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn screenshot_pack_resolves_exact_parent_and_sibling_assets() {
        let root = unique_temp_dir("screenshot-pack-family");
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
                "SELECT identity_id,asset_key,asset_link_reason,image_path,has_image
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
                    row.get::<_, i64>(4)?,
                ))
            })
            .expect("query variant assets")
            .map(|row| row.expect("read variant asset row"))
            .collect::<Vec<_>>();
        let preferred = conn
            .query_row(
                "SELECT identity_id,asset_key,asset_link_reason,image_path,has_image
                 FROM ui_arcade_preferred",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .expect("query preferred asset");

        assert_eq!(rows.len(), 4);
        for row in &rows {
            assert_eq!(row.1.as_deref(), Some("1941u"));
            assert_eq!(row.3, "/media/fat/_Arcade/media/screenshot/1941u.png");
            assert_eq!(row.4, 1);
        }
        assert!(rows
            .iter()
            .any(|row| row.0.as_deref() == Some("1941u") && row.2 == "exact"));
        assert!(rows
            .iter()
            .any(|row| row.0.as_deref() == Some("1941") && row.2 == "sibling"));
        assert_eq!(preferred.0.as_deref(), Some("1941"));
        assert_eq!(preferred.1.as_deref(), Some("1941u"));
        assert_eq!(preferred.2, "sibling");
        assert_eq!(preferred.3, "/media/fat/_Arcade/media/screenshot/1941u.png");
        assert_eq!(preferred.4, 1);
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
        assert_eq!(scan.ignored_files.len(), 2);
        assert!(scan
            .file_fingerprints
            .contains_key(&listings_dir.join("games.txt").display().to_string()));
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
        assert!(is_launcher_launch_ref(&amigavision_game_launch_ref(
            "4th & Inches (OCS)[en]"
        )));
    }

    #[test]
    fn directory_manifest_validation_recomputes_child_signature() {
        let root = unique_temp_dir("manifest-child-signature");
        let rom_dir = root.join("games/NES");
        std::fs::create_dir_all(&rom_dir).expect("create rom dir");
        let rom = rom_dir.join("same-second.nes");
        std::fs::write(&rom, b"rom").expect("write rom");
        let root_key = root.display().to_string();
        let current = build_directory_manifest(std::slice::from_ref(&root_key), None);
        let current_sig = current[&root_key];
        let mut manifest = DirectoryManifest::new();
        manifest.insert(
            root_key.clone(),
            DirectorySignature {
                dir_size: current_sig.dir_size,
                dir_mtime_secs: current_sig.dir_mtime_secs,
                child_count: 0,
                hash: MANIFEST_HASH_OFFSET,
            },
        );
        let fingerprint = fingerprint_with_manifest(manifest);

        let validated =
            validate_or_rebuild_directory_manifest(std::slice::from_ref(&root_key), &fingerprint);

        assert_eq!(validated, Some(DirectoryManifest::new()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn directory_manifest_validation_keeps_unchanged_manifest() {
        let root = unique_temp_dir("manifest-unchanged");
        let rom_dir = root.join("games/NES");
        std::fs::create_dir_all(&rom_dir).expect("create rom dir");
        std::fs::write(rom_dir.join("unchanged.nes"), b"rom").expect("write rom");
        let root_key = root.display().to_string();
        let manifest = build_directory_manifest(std::slice::from_ref(&root_key), None);
        let fingerprint = fingerprint_with_manifest(manifest.clone());

        let validated = validate_or_rebuild_directory_manifest(&[root_key], &fingerprint);

        assert_eq!(validated, Some(manifest));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn refresh_plan_uses_unchanged_database_without_refresh() {
        let manifest = single_dir_manifest("/media/fat/_Arcade", 1);
        let fingerprint = fingerprint_with_manifest(manifest.clone());

        assert_eq!(
            library_refresh_plan(&fingerprint, Some(&manifest), true, true),
            LibraryRefreshPlan::UseCachedDatabase
        );
    }

    #[test]
    fn refresh_plan_updates_preview_assets_without_rebuild() {
        let manifest = single_dir_manifest("/media/fat/_Arcade", 1);
        let fingerprint = fingerprint_with_manifest(manifest.clone());

        assert_eq!(
            library_refresh_plan(&fingerprint, Some(&manifest), false, true),
            LibraryRefreshPlan::RefreshPreviewAssets
        );
    }

    #[test]
    fn stale_preview_assets_refresh_before_manifest_validation_when_metadata_is_current() {
        assert!(should_refresh_preview_assets_before_manifest(false, true));
        assert!(!should_refresh_preview_assets_before_manifest(true, true));
        assert!(!should_refresh_preview_assets_before_manifest(false, false));
    }

    #[test]
    fn refresh_plan_rebuilds_when_metadata_changes() {
        let manifest = single_dir_manifest("/media/fat/_Arcade", 1);
        let fingerprint = fingerprint_with_manifest(manifest.clone());

        assert_eq!(
            library_refresh_plan(&fingerprint, Some(&manifest), true, false),
            LibraryRefreshPlan::RebuildDatabase
        );
    }

    #[test]
    fn refresh_plan_rebuilds_when_file_tree_changes() {
        let old_manifest = single_dir_manifest("/media/fat/_Arcade", 1);
        let current_manifest = single_dir_manifest("/media/fat/_Arcade", 2);
        let fingerprint = fingerprint_with_manifest(old_manifest);

        assert_eq!(
            library_refresh_plan(&fingerprint, Some(&current_manifest), true, true),
            LibraryRefreshPlan::RebuildDatabase
        );
    }

    #[test]
    fn preview_archive_paths_are_recognized_as_fingerprint_inputs() {
        assert!(is_preview_archive_fingerprint_path(
            "/media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320-rawpack.mmraw"
        ));
        assert!(is_preview_archive_fingerprint_path(
            "/media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320-lz4block-12.mmlz4b"
        ));
        assert!(!is_preview_archive_fingerprint_path(
            "/media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320/mpatrol.rgb565"
        ));
    }

    #[test]
    fn preview_archive_added_after_old_database_updates_assets_without_rescan() {
        let root = unique_temp_dir("preview-archive-asset-refresh");
        let db = root.join("library.sqlite3");
        let mut discovery = mra_discovery(1, "1941");
        discovery.setname = Some("game00001".to_string());
        save_sqlite_scan(&db, &sqlite_scan_with_discoveries(vec![discovery])).expect("save sqlite");
        let pack_path =
            "/media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320-rawpack.mmraw";
        let pack = preview_worker::PreviewArchiveIndex {
            path: pack_path.to_string(),
            codec: "raw",
            entries: vec!["game00001".to_string()],
        };

        let bytes =
            refresh_sqlite_preview_assets(&db, &[pack], vec![(pack_path.to_string(), 1234, 77)])
                .expect("refresh preview assets");

        assert!(bytes > 0);
        let conn = Connection::open(&db).expect("open library sqlite");
        let row = conn
            .query_row(
                "SELECT image_path,has_image,asset_key,asset_link_reason
                 FROM ui_arcade_preferred",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .expect("query refreshed preferred row");
        assert_eq!(row.0, "/media/fat/_Arcade/media/screenshot/game00001.png");
        assert_eq!(row.1, 1);
        assert_eq!(row.2.as_deref(), Some("game00001"));
        assert_eq!(row.3, "exact");
        assert_eq!(
            conn.query_row("SELECT count(*) FROM games", [], |row| row.get::<_, i64>(0))
                .expect("count games"),
            1
        );
        assert_eq!(
            read_sqlite_fingerprint(&db)
                .expect("read fingerprint")
                .file_fingerprints
                .get(pack_path),
            Some(&(1234, 77))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn console_preview_asset_refresh_clears_removed_pack_images() {
        let root = unique_temp_dir("console-preview-clear");
        let rom_path = root.join("Game.nes");
        let rom = b"fixture-rom";
        std::fs::write(&rom_path, rom).expect("write rom");
        let mame_db = root.join("mame.sqlite3");
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
            &[("nes", "fixture", rom.len() as i64, crc32(rom))],
        );
        let db = root.join("library.sqlite3");
        let mut discovery = payload(&rom_path.display().to_string());
        discovery.platform_id = "nes".to_string();
        discovery.category = "Console".to_string();
        discovery.core_id = "NES".to_string();
        discovery.hardware_id = "nes".to_string();
        let pack_path = "/media/fat/mister-magik/assets/nes-screenshots.mmlz4b";
        let pack = preview_worker::PreviewArchiveIndex {
            path: pack_path.to_string(),
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

        refresh_sqlite_preview_assets(&db, &[], Vec::new()).expect("refresh without pack");

        let conn = Connection::open(&db).expect("open library sqlite");
        let row: (String, i64) = conn
            .query_row(
                "SELECT image_path,has_image FROM launcher_catalog WHERE system_id='nes'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("launcher row");
        assert_eq!(row, (String::new(), 0));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn preview_archive_fingerprint_mismatch_marks_preview_assets_stale() {
        let fingerprint = fingerprint_with_files(&[]);

        assert!(!preview_archive_fingerprint_matches(
            &fingerprint,
            vec![(
                "/media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320-rawpack.mmraw"
                    .to_string(),
                1024,
                42,
            )],
        ));
    }

    #[test]
    fn matching_preview_archive_fingerprint_allows_catalog_refresh_skip() {
        let path = "/media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320-rawpack.mmraw";
        let fingerprint = fingerprint_with_files(&[(path, 1024, 42)]);

        assert!(preview_archive_fingerprint_matches(
            &fingerprint,
            vec![(path.to_string(), 1024, 42)],
        ));
    }

    #[test]
    fn matching_preview_archive_fingerprints_require_the_full_active_set() {
        let arcade =
            "/media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320-rawpack.mmraw";
        let neogeo = "/media/fat/mister-magik/assets/neogeo-screenshots.mmlz4b";
        let fingerprint = fingerprint_with_files(&[(arcade, 1024, 42), (neogeo, 2048, 77)]);

        assert!(preview_archive_fingerprint_matches(
            &fingerprint,
            vec![
                (arcade.to_string(), 1024, 42),
                (neogeo.to_string(), 2048, 77)
            ],
        ));
        assert!(!preview_archive_fingerprint_matches(
            &fingerprint,
            vec![(arcade.to_string(), 1024, 42)],
        ));
        assert!(!preview_archive_fingerprint_matches(
            &fingerprint,
            vec![
                (arcade.to_string(), 1024, 42),
                (neogeo.to_string(), 2048, 78)
            ],
        ));
    }

    #[test]
    fn directory_manifest_metadata_check_detects_missing_directory() {
        let root = unique_temp_dir("manifest-metadata-missing");
        let rom_dir = root.join("games/NES");
        std::fs::create_dir_all(&rom_dir).expect("create rom dir");
        std::fs::write(rom_dir.join("unchanged.nes"), b"rom").expect("write rom");
        let root_key = root.display().to_string();
        let manifest = build_directory_manifest(std::slice::from_ref(&root_key), None);

        assert!(!directory_manifest_metadata_changed(&manifest));
        std::fs::remove_dir_all(&rom_dir).expect("remove rom dir");
        assert!(directory_manifest_metadata_changed(&manifest));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_save_keeps_previous_database_when_replacement_fails() {
        let root = unique_temp_dir("sqlite-atomic-replace");
        let db = root.join("library.sqlite3");
        save_sqlite_scan(&db, &sqlite_scan_with_normal_files(&["/old/game.mra"]))
            .expect("write old database");
        let old_fingerprint = read_sqlite_fingerprint(&db).expect("old database readable");
        assert_eq!(old_fingerprint.normal_files, 1);

        let err = save_sqlite_scan(
            &db,
            &sqlite_scan_with_normal_files(&["/new/game.mra", "/new/game.mra"]),
        )
        .expect_err("duplicate normal_files row should fail temp import");

        assert!(
            err.contains("insert payload file"),
            "unexpected error: {err}"
        );
        let still_old = read_sqlite_fingerprint(&db).expect("old database survived failed import");
        assert_eq!(still_old.normal_files, 1);
        assert!(
            !sqlite_temp_path(&db).exists(),
            "failed temp database should be cleaned up"
        );
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
    fn sqlite_save_materializes_launcher_catalog_variants() {
        let root = unique_temp_dir("sqlite-launcher-catalog");
        let db = root.join("library.sqlite3");
        let mut world = mra_discovery(1, "Moon Patrol (World)");
        world.launch_ref = "/media/fat/_Arcade/Moon Patrol (World).mra".to_string();
        world.source_path = world.launch_ref.clone();
        world.setname = Some("mpatrol".to_string());
        let mut us = mra_discovery(2, "Moon Patrol (US)");
        us.launch_ref = "/media/fat/_Arcade/Moon Patrol (US).mra".to_string();
        us.source_path = us.launch_ref.clone();
        us.setname = Some("mpatrol".to_string());
        us.image_path = Some("/media/fat/_Arcade/media/screenshot/mpatrol.png".to_string());
        us.has_image = true;

        save_sqlite_scan(&db, &sqlite_scan_with_discoveries(vec![world, us]))
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
        assert!(loaded.catalog.games[0].has_image);
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
            image_path: None,
            has_image: false,
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
            image_path: None,
            has_image: false,
            confidence: DiscoveryConfidence::PayloadPath,
        }
    }

    fn catalog_row(title: &str, path: &str, setname: &str, parent: &str) -> CatalogRow {
        CatalogRow {
            game: ArcadeGameEntry {
                title: title.into(),
                mra_path: path.into(),
                image_path: "".into(),
                has_image: false,
                system_id: "arcade".into(),
            },
            source_kind: "mra".to_string(),
            setname: setname.to_string(),
            parent: parent.to_string(),
        }
    }

    fn catalog_launcher_row(title: &str, path: &str) -> CatalogRow {
        CatalogRow {
            game: ArcadeGameEntry {
                title: title.into(),
                mra_path: path.into(),
                image_path: "".into(),
                has_image: false,
                system_id: "unknown".into(),
            },
            source_kind: "mgl".to_string(),
            setname: String::new(),
            parent: String::new(),
        }
    }

    fn catalog_entry_row(title: &str, path: &str) -> CatalogRow {
        CatalogRow {
            game: ArcadeGameEntry {
                title: title.into(),
                mra_path: path.into(),
                image_path: "".into(),
                has_image: false,
                system_id: "amiga".into(),
            },
            source_kind: "catalog-entry".to_string(),
            setname: String::new(),
            parent: String::new(),
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

    fn write_mame_fixture_db(
        path: &Path,
        rows: &[(&str, Option<&str>, &str, Option<&str>, Option<&str>)],
    ) {
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
            image_path: None,
            has_image: false,
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
            image_path: None,
            has_image: false,
            confidence: DiscoveryConfidence::MraCore,
        }
    }

    fn sqlite_scan_with_normal_files(paths: &[&str]) -> LibraryScan {
        LibraryScan {
            version: SCHEMA_VERSION,
            scanned_at_unix: 1,
            file_fingerprints: FileFingerprint::default(),
            directory_manifest: DirectoryManifest::new(),
            normal_files: paths
                .iter()
                .map(|path| LibraryPayloadFile {
                    path: path.to_string(),
                    profile_id: "mgl".to_string(),
                    rule: PayloadRule {
                        extensions: &["mgl"],
                        mount: launch_profiles::MountSpec::launcher(),
                        disposition: PayloadDisposition::Playable,
                        provenance: RuleProvenance::mgl("test fixture launcher payload"),
                    },
                })
                .collect(),
            containers: Vec::new(),
            entries: Vec::new(),
            ignored_files: Vec::new(),
            discoveries: Vec::new(),
            discover_us: 0,
            classify_us: 0,
        }
    }

    fn sqlite_scan_with_discoveries(discoveries: Vec<GameDiscovery>) -> LibraryScan {
        LibraryScan {
            version: SCHEMA_VERSION,
            scanned_at_unix: 1,
            file_fingerprints: FileFingerprint::default(),
            directory_manifest: DirectoryManifest::new(),
            normal_files: Vec::new(),
            containers: Vec::new(),
            entries: Vec::new(),
            ignored_files: Vec::new(),
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

    fn fingerprint_with_manifest(directory_manifest: DirectoryManifest) -> DbFingerprint {
        DbFingerprint {
            normal_files: 0,
            containers: 0,
            entries: 0,
            discoveries: 0,
            mame_metadata: FileSignature::default(),
            hbmame_metadata: FileSignature::default(),
            file_fingerprints: FileFingerprint::default(),
            container_fingerprints: BTreeMap::new(),
            directory_manifest,
        }
    }

    fn fingerprint_with_files(files: &[(&str, u64, i64)]) -> DbFingerprint {
        let mut fingerprint = fingerprint_with_manifest(DirectoryManifest::new());
        fingerprint.file_fingerprints = files
            .iter()
            .map(|(path, size, mtime_secs)| (path.to_string(), (*size, *mtime_secs)))
            .collect();
        fingerprint
    }

    fn single_dir_manifest(path: &str, hash: u64) -> DirectoryManifest {
        let mut manifest = DirectoryManifest::new();
        manifest.insert(
            path.to_string(),
            DirectorySignature {
                dir_size: 1,
                dir_mtime_secs: 1,
                child_count: 1,
                hash,
            },
        );
        manifest
    }
}
