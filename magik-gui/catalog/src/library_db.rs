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

const MRA_PREFIX_BYTES: usize = 160 * 1024;
type FileFingerprint = BTreeMap<String, (u64, i64)>;
type ProgressCallback<'a> = Option<&'a mut dyn FnMut(&str, &str)>;
type DirectoryManifest = BTreeMap<String, DirectorySignature>;

const SCHEMA_VERSION: u32 = 18;
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
    pub rows: usize,
}

#[derive(Clone, Debug)]
pub struct LibraryRefreshSummary {
    pub skipped: bool,
    pub scan_us: u64,
    pub import_us: u64,
    pub bytes: u64,
    pub normal_files: usize,
    pub containers: usize,
    pub entries: usize,
    pub discoveries: usize,
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
    file_fingerprints: FileFingerprint,
    container_fingerprints: BTreeMap<String, (u64, i64)>,
    directory_manifest: DirectoryManifest,
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
                    "library_scan_bench_tsv\t{label}\t{iteration}\tchanged_refresh\t{changed_refresh_us}\tscan_us={}\timport_us={}\tskipped={}\tdiscoveries={}",
                    summary.scan_us,
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
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open library db: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    let games = match load_materialized_launcher_catalog(&conn) {
        Ok(Some(games)) => games,
        Ok(None) => load_joined_launcher_catalog(&conn)?,
        Err(e) => return Err(e),
    };
    let rows = games.len();
    let systems = arcade_catalog::systems_from_games(&games);
    Ok(LibraryCatalogLoad {
        catalog: ArcadeCatalog::new(root, games, systems),
        us: t.elapsed().as_micros() as u64,
        rows,
    })
}

fn load_materialized_launcher_catalog(
    conn: &Connection,
) -> Result<Option<Vec<ArcadeGameEntry>>, String> {
    if !sqlite_table_exists(conn, "launcher_catalog")? {
        return Ok(None);
    }
    let mut stmt = conn
        .prepare(
            "SELECT title,
                    launch_ref,
                    image_path,
                    has_image,
                    system_id
             FROM launcher_catalog
             ORDER BY ordinal",
        )
        .map_err(|e| format!("prepare launcher catalog query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ArcadeGameEntry {
                title: row.get::<_, String>(0)?,
                mra_path: row.get::<_, String>(1)?,
                image_path: row.get::<_, String>(2)?,
                has_image: row.get::<_, i64>(3)? != 0,
                system_id: row.get::<_, String>(4)?,
            })
        })
        .map_err(|e| format!("query launcher catalog: {e}"))?;
    let mut games = Vec::new();
    for row in rows {
        games.push(row.map_err(|e| format!("read launcher catalog row: {e}"))?);
    }
    Ok(Some(games))
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
                    title: row.get::<_, String>(0)?,
                    mra_path: row.get::<_, String>(1)?,
                    image_path: row.get::<_, String>(2)?,
                    has_image: row.get::<_, i64>(3)? != 0,
                    system_id: row.get::<_, String>(4)?,
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

    variant_score_from_haystack(&haystack)
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

fn refresh_sqlite_database(
    cfg: &BenchConfig,
    mut progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    let scan_t = Instant::now();
    if let Some(existing) = read_sqlite_fingerprint(&cfg.sqlite_path) {
        if let Some(report) = progress.as_mut() {
            report("Checking library", "Looking for changed files...");
        }
        let current_manifest = validate_or_rebuild_directory_manifest(&cfg.roots, &existing);
        let scan_us = scan_t.elapsed().as_micros() as u64;
        if current_manifest == Some(existing.directory_manifest.clone())
            && preview_archive_fingerprint_unchanged(&existing)
        {
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
                import_us: 0,
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
                "Directory manifest changed; rebuilding database...",
            );
        }
    } else {
        let _ = std::fs::remove_file(&cfg.sqlite_path);
        if let Some(report) = progress.as_mut() {
            report(
                "Indexing library",
                "No usable database fingerprint; full scan...",
            );
        }
    }

    let scan = match progress.as_mut() {
        Some(report) => scan_library_with_progress(&cfg, Some(&mut **report)),
        None => scan_library(&cfg),
    };
    let scan_us = scan_t.elapsed().as_micros() as u64;

    if let Some(report) = progress.as_mut() {
        report(
            "Indexing library",
            &format!(
                "Writing {} games, {} archives...",
                unique_discovery_count(&scan.discoveries),
                scan.containers.len()
            ),
        );
    }
    let import_t = Instant::now();
    let bytes = save_sqlite_scan(&cfg.sqlite_path, &scan)?;
    let import_us = import_t.elapsed().as_micros() as u64;
    Ok(LibraryRefreshSummary {
        skipped: false,
        scan_us,
        import_us,
        bytes,
        normal_files: scan.normal_files.len(),
        containers: scan.containers.len(),
        entries: scan.entries.len(),
        discoveries: unique_discovery_count(&scan.discoveries),
    })
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
                    &format!(
                        "{idx} candidate files; {} games, {} archives, {} archive entries",
                        discoveries.len(),
                        containers.len(),
                        entries.len()
                    ),
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
    if let Ok(Some((path, size, mtime_secs))) = preview_worker::preview_archive_fingerprint_from_env()
    {
        file_fingerprints.insert(path, (size, mtime_secs));
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
    let components = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();

    for window in components.windows(2) {
        if window[0].eq_ignore_ascii_case("games") {
            if let Some(profile) = launch_profiles::profile_for_game_dir(profiles, window[1]) {
                return Some(profile);
            }
        }
    }

    profiles.iter().find(|profile| {
        components.iter().any(|component| {
            profile
                .game_dirs
                .iter()
                .any(|dir| component.eq_ignore_ascii_case(dir))
        })
    })
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
    match preview_worker::preview_archive_fingerprint_from_env() {
        Ok(current) => preview_archive_fingerprint_matches(existing, current),
        Err(_) => false,
    }
}

fn preview_archive_fingerprint_matches(
    existing: &DbFingerprint,
    current: Option<(String, u64, i64)>,
) -> bool {
    match current {
        Some((path, size, mtime_secs)) => existing
            .file_fingerprints
            .get(&path)
            .is_some_and(|fingerprint| *fingerprint == (size, mtime_secs)),
        None => !existing
            .file_fingerprints
            .keys()
            .any(|path| is_preview_archive_fingerprint_path(path)),
    }
}

fn is_preview_archive_fingerprint_path(path: &str) -> bool {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    name.starts_with("raw565-") && (name.ends_with(".mmraw") || name.ends_with(".mmlz4b"))
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
    let cd_size_usize = usize::try_from(cd_size)
        .map_err(|_| "zip central directory too large to index".to_string())?;

    f.seek(SeekFrom::Start(cd_offset))
        .map_err(|e| format!("seek zip central directory: {e}"))?;
    let mut cd = vec![0u8; cd_size_usize];
    f.read_exact(&mut cd)
        .map_err(|e| format!("read zip central directory: {e}"))?;

    let mut entries = Vec::new();
    let mut pos = 0usize;
    let mut scanned = 0usize;
    while pos + 46 <= cd.len() && scanned < cd_entries {
        if le_u32(&cd[pos..pos + 4]) != 0x0201_4b50 {
            return Err(format!("bad central directory signature at {pos}"));
        }
        scanned += 1;
        let crc32 = le_u32(&cd[pos + 16..pos + 20]);
        let compressed = le_u32(&cd[pos + 20..pos + 24]) as u64;
        let uncompressed = le_u32(&cd[pos + 24..pos + 28]) as u64;
        let name_len = le_u16(&cd[pos + 28..pos + 30]) as usize;
        let extra_len = le_u16(&cd[pos + 30..pos + 32]) as usize;
        let comment_len = le_u16(&cd[pos + 32..pos + 34]) as usize;
        let name_start = pos + 46;
        let name_end = name_start + name_len;
        if name_end > cd.len() {
            return Err("zip entry name outside central directory".to_string());
        }
        let name = String::from_utf8_lossy(&cd[name_start..name_end]).into_owned();
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
        pos = name_end + extra_len + comment_len;
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
        Self {
            arcade_stems: preview_worker::preview_archive_entry_stems_from_env()
                .ok()
                .flatten(),
        }
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
    if discovery.platform_id != "arcade" {
        return;
    }
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
                setname: None,
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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create sqlite dir: {e}"))?;
    }

    let tmp_path = sqlite_temp_path(path);
    match std::fs::remove_file(&tmp_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove stale sqlite temp: {e}")),
    }

    if let Err(e) = write_sqlite_scan(&tmp_path, scan) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    File::open(&tmp_path)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("sync sqlite temp: {e}"))?;
    std::fs::rename(&tmp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("replace sqlite: {e}")
    })?;
    sync_parent_dir(path);
    std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("stat sqlite: {e}"))
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

fn write_sqlite_scan(path: &Path, scan: &LibraryScan) -> Result<(), String> {
    let mut conn = Connection::open(path).map_err(|e| format!("open sqlite: {e}"))?;
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
        "#,
    )
    .map_err(|e| format!("create sqlite schema: {e}"))?;

    let tx = conn
        .transaction()
        .map_err(|e| format!("begin sqlite tx: {e}"))?;
    {
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
    }
    {
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
    }
    {
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
    }
    {
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
    }
    {
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
        for (key, discovery) in discoveries {
            let system_id = catalog_system_id_for_discovery(discovery);
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
                    discovery.image_path.as_deref(),
                    if discovery.has_image { 1 } else { 0 }
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
                launcher_rows.push(CatalogRow {
                    game: ArcadeGameEntry {
                        title: discovery.title.clone(),
                        mra_path: plan_launch_ref.clone(),
                        image_path: discovery.image_path.clone().unwrap_or_default(),
                        has_image: discovery.has_image,
                        system_id: system_id.clone(),
                    },
                    source_kind: launch_kind_for_discovery(discovery).to_string(),
                    setname: discovery.setname.clone().unwrap_or_default(),
                    parent: discovery.parent.clone().unwrap_or_default(),
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
            let region = infer_region_metadata(discovery);
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
        }
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
                    idx as i64,
                    game.title.as_str(),
                    normalize_title(&game.title),
                    game.mra_path.as_str(),
                    game.image_path.as_str(),
                    if game.has_image { 1 } else { 0 },
                    game.system_id.as_str()
                ])
                .map_err(|e| format!("insert launcher catalog: {e}"))?;
        }
    }
    {
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
    }
    {
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
    }
    {
        let mut stmt = tx
            .prepare("INSERT INTO file_fingerprints(file_path,size,mtime_secs) VALUES (?1,?2,?3)")
            .map_err(|e| format!("prepare file fingerprint insert: {e}"))?;
        for (path, (size, mtime_secs)) in &scan.file_fingerprints {
            stmt.execute(params![path.as_str(), *size as i64, *mtime_secs])
                .map_err(|e| format!("insert file fingerprint: {e}"))?;
        }
    }
    {
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
    }
    {
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
    }
    tx.commit().map_err(|e| format!("commit sqlite tx: {e}"))
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
        file_fingerprints,
        container_fingerprints,
        directory_manifest,
    })
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

fn le_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
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
        assert_eq!(games[0].title, "Moon Patrol (US)");
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
        assert!(games.iter().any(|game| game.title == "Agony"));
        assert!(games.iter().any(|game| game.title == "Alien Breed"));
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
        assert_eq!(loaded.catalog.games[0].system_id, "neogeo");
        assert!(loaded.catalog.games[0].mra_path.starts_with("magik-plan:"));
        assert!(loaded
            .catalog
            .systems
            .iter()
            .any(|system| system.id == "neogeo" && system.count == 1));
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
            .all(|game| game.system_id == "amiga"));
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
            game.mra_path == AMIGAVISION_LAUNCHER_REF
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
    fn preview_archive_added_after_old_database_forces_catalog_refresh() {
        let fingerprint = fingerprint_with_files(&[]);

        assert!(!preview_archive_fingerprint_matches(
            &fingerprint,
            Some((
                "/media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320-rawpack.mmraw"
                    .to_string(),
                1024,
                42,
            )),
        ));
    }

    #[test]
    fn matching_preview_archive_fingerprint_allows_catalog_refresh_skip() {
        let path = "/media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320-rawpack.mmraw";
        let fingerprint = fingerprint_with_files(&[(path, 1024, 42)]);

        assert!(preview_archive_fingerprint_matches(
            &fingerprint,
            Some((path.to_string(), 1024, 42)),
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
            .any(|game| game.title == "Game 20004"));
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
        assert_eq!(loaded.catalog.games[0].title, "Moon Patrol (US)");
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
                title: title.to_string(),
                mra_path: path.to_string(),
                image_path: String::new(),
                has_image: false,
                system_id: "arcade".to_string(),
            },
            source_kind: "mra".to_string(),
            setname: setname.to_string(),
            parent: parent.to_string(),
        }
    }

    fn catalog_launcher_row(title: &str, path: &str) -> CatalogRow {
        CatalogRow {
            game: ArcadeGameEntry {
                title: title.to_string(),
                mra_path: path.to_string(),
                image_path: String::new(),
                has_image: false,
                system_id: "unknown".to_string(),
            },
            source_kind: "mgl".to_string(),
            setname: String::new(),
            parent: String::new(),
        }
    }

    fn catalog_entry_row(title: &str, path: &str) -> CatalogRow {
        CatalogRow {
            game: ArcadeGameEntry {
                title: title.to_string(),
                mra_path: path.to_string(),
                image_path: String::new(),
                has_image: false,
                system_id: "amiga".to_string(),
            },
            source_kind: "catalog-entry".to_string(),
            setname: String::new(),
            parent: String::new(),
        }
    }

    fn write_stored_zip(path: &Path, entries: &[(&str, &[u8])]) {
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
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u32(&mut central, 0);
            push_u32(&mut central, local_offset);
            central.extend_from_slice(name.as_bytes());
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
}
