//! Whole-MiSTer library database scanning and loading.
//!
//! This is deliberately TOC/header-only for archives. Indexing must never
//! decompress full game libraries just to make the launcher searchable.

use crate::arcade_catalog::{self, ArcadeCatalog, ArcadeGameEntry};
use crate::launch_profiles::{
    self, IgnoreReason, LaunchProfile, MountKind, PayloadDisposition, PayloadRule, ProfilePathClass,
    RuleProvenance, RuleSourceKind,
};
use rusqlite::{params, Connection, OpenFlags};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
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

const SCHEMA_VERSION: u32 = 9;
const MANIFEST_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const MANIFEST_HASH_PRIME: u64 = 0x0000_0100_0000_01b3;

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
}

#[derive(Clone, Copy, Debug)]
enum DiscoveryConfidence {
    MraHardware,
    MraCore,
    PayloadPath,
    Extension,
}

#[derive(Clone)]
struct FoundFile {
    path: PathBuf,
    ext: String,
    size: u64,
    mtime_secs: i64,
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
    }
}

pub fn default_sqlite_path() -> PathBuf {
    std::env::var("MISTER_LIBRARY_SQLITE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_SQLITE_PATH))
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
               AND launch_plans.launch_kind IN ('mra','mgl','virtual-mgl')
               AND (
                 lower(launch_plans.launch_ref) LIKE '%.mra'
                 OR lower(launch_plans.launch_ref) LIKE '%.mgl'
                 OR launch_plans.launch_kind='virtual-mgl'
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
    let games = collapse_catalog_variants(rows_out);
    let rows = games.len();
    let systems = arcade_catalog::systems_from_games(&games);
    Ok(LibraryCatalogLoad {
        catalog: ArcadeCatalog::new(root, games, systems),
        us: t.elapsed().as_micros() as u64,
        rows,
    })
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

    let mut score = 0;
    if contains_any(
        &haystack,
        &[
            "(usa", "(us,", "(us)", "(u)", "/_usa/", " america", "american",
        ],
    ) {
        score += 1000;
    } else if contains_any(&haystack, &["(japan", "(jp", "(j)", "/_japan/"]) {
        score += 900;
    } else if contains_any(&haystack, &["(world", "(w,", "(w)", "/_world/"]) {
        score += 800;
    } else if contains_any(&haystack, &["(europe", "(eu", "(e)", "/_europe/"]) {
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
    mut progress: ProgressCallback<'_>,
) -> Result<LibraryRefreshSummary, String> {
    let cfg = BenchConfig::production();
    let scan_t = Instant::now();
    if let Some(existing) = read_sqlite_fingerprint(&cfg.sqlite_path) {
        if let Some(report) = progress.as_mut() {
            report("Checking library", "Looking for changed files...");
        }
        let current_manifest = validate_or_rebuild_directory_manifest(&cfg.roots, &existing);
        let scan_us = scan_t.elapsed().as_micros() as u64;
        if current_manifest == Some(existing.directory_manifest.clone()) {
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
    optional_catalogs: bool,
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
        let optional_catalogs = env_bool("MISTER_LIBRARY_OPTIONAL_CATALOGS");
        Self {
            roots,
            sqlite_path,
            optional_catalogs,
        }
    }

    fn production() -> Self {
        let mut cfg = Self::from_env();
        cfg.sqlite_path = default_sqlite_path();
        cfg.optional_catalogs = true;
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
    let containers = Vec::new();
    let entries = Vec::new();
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
                normal_files.push(LibraryPayloadFile {
                    path: f.path.display().to_string(),
                    profile_id: profile.id.to_string(),
                    rule: payload_rule,
                });
                discoveries.push(discovery_from_profile_file(
                    &f,
                    profile,
                    &payload_rule,
                    &profiles,
                ));
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
            Some((
                profile,
                ProfilePathClass::Ignored {
                    reason,
                    provenance,
                },
            )) => {
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
    if discover_us == 0 {
        discover_us = discover_t.elapsed().as_micros() as u64;
    }
    if cfg.optional_catalogs {
        if let Some(report) = progress.as_mut() {
            report(
                "Importing metadata",
                "Looking for gamelist.xml screenshots...",
            );
        }
        let imported = match progress.as_mut() {
            Some(report) => {
                enrich_discoveries_from_gamelists(&mut discoveries, &cfg.roots, Some(&mut **report))
            }
            None => enrich_discoveries_from_gamelists(&mut discoveries, &cfg.roots, None),
        };
        if let Some(report) = progress.as_mut() {
            report(
                "Importing metadata",
                &format!("Matched screenshot metadata for {imported} games"),
            );
        }
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

fn profile_for_path<'a>(
    profiles: &'a [LaunchProfile],
    path: &Path,
) -> Option<&'a LaunchProfile> {
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
    let current = build_directory_manifest(roots, None);
    if current == existing.directory_manifest {
        Some(current)
    } else {
        Some(DirectoryManifest::new())
    }
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

#[derive(Clone, Debug)]
struct GamelistMetadata {
    title: Option<String>,
    image_path: Option<String>,
    has_image: bool,
}

fn enrich_discoveries_from_gamelists(
    discoveries: &mut [GameDiscovery],
    roots: &[String],
    mut progress: ProgressCallback<'_>,
) -> usize {
    let mut by_path = HashMap::<String, GamelistMetadata>::new();
    let mut by_stem = HashMap::<String, GamelistMetadata>::new();
    let mut xml_files = 0usize;
    let mut xml_games = 0usize;
    for root in roots {
        let path = Path::new(root);
        if !path.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(path)
            .follow_links(true)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if !entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("gamelist.xml")
            {
                continue;
            }
            let parent = entry.path().parent().unwrap_or(path);
            xml_files += 1;
            if let Some(report) = progress.as_mut() {
                report(
                    "Importing metadata",
                    &format!("Reading {}", entry.path().display()),
                );
            }
            let rows = parse_gamelist_metadata(entry.path(), parent);
            xml_games += rows.len();
            for (game_path, meta) in rows {
                by_stem
                    .entry(match_stem(&game_path))
                    .or_insert_with(|| meta.clone());
                by_path.insert(normalize_match_path(&game_path), meta);
            }
        }
    }
    if let Some(report) = progress.as_mut() {
        report(
            "Importing metadata",
            &format!("{xml_files} XML files, {xml_games} metadata rows"),
        );
    }

    let mut matched = 0usize;
    let total = discoveries.len();
    for (idx, discovery) in discoveries.iter_mut().enumerate() {
        if idx % 500 == 0 {
            if let Some(report) = progress.as_mut() {
                report(
                    "Matching screenshots",
                    &format!("{idx}/{total} discoveries; {matched} matched"),
                );
            }
        }
        let meta = by_path
            .get(&normalize_match_path(&discovery.source_path))
            .or_else(|| by_path.get(&normalize_match_path(&discovery.launch_ref)))
            .or_else(|| by_stem.get(&match_stem(&discovery.source_path)))
            .or_else(|| by_stem.get(&match_stem(&discovery.launch_ref)));
        let Some(meta) = meta else {
            continue;
        };
        if let Some(title) = meta.title.as_deref().filter(|s| !s.trim().is_empty()) {
            discovery.title = title.to_string();
        }
        if let Some(image_path) = meta.image_path.as_deref().filter(|s| !s.trim().is_empty()) {
            discovery.image_path = Some(image_path.to_string());
            discovery.has_image = meta.has_image;
        }
        matched += 1;
    }
    matched
}

fn parse_gamelist_metadata(path: &Path, base: &Path) -> Vec<(String, GamelistMetadata)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("<game") {
        rest = &rest[start..];
        let Some(tag_end) = rest.find('>') else {
            break;
        };
        let after_open = &rest[tag_end + 1..];
        let Some(end) = after_open.find("</game>") else {
            break;
        };
        let block = &after_open[..end];
        rest = &after_open[end + "</game>".len()..];
        let Some(raw_path) = tag_text(block, "path") else {
            continue;
        };
        let game_path = resolve_gamelist_path(base, &raw_path);
        let image_path = tag_text(block, "image").map(|image| resolve_gamelist_path(base, &image));
        let has_image = image_path
            .as_deref()
            .map(|p| Path::new(p).is_file())
            .unwrap_or(false);
        out.push((
            game_path,
            GamelistMetadata {
                title: tag_text(block, "name"),
                image_path,
                has_image,
            },
        ));
    }
    out
}

fn resolve_gamelist_path(base: &Path, raw: &str) -> String {
    let clean = raw.trim().trim_start_matches("./");
    if clean.starts_with('/') {
        clean.to_string()
    } else {
        base.join(clean).display().to_string()
    }
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

fn match_stem(path: &str) -> String {
    normalize_id(
        Path::new(path.split("::").next().unwrap_or(path))
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(path),
    )
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
    discoveries
        .iter()
        .filter(|d| is_playable_discovery_with_coverage(d, &covered_payloads))
        .map(discovery_unique_key)
        .collect::<HashSet<_>>()
        .len()
}

fn confidence_str(confidence: DiscoveryConfidence) -> &'static str {
    match confidence {
        DiscoveryConfidence::MraHardware => "mra-hardware",
        DiscoveryConfidence::MraCore => "mra-core",
        DiscoveryConfidence::PayloadPath => "payload-path",
        DiscoveryConfidence::Extension => "extension",
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
                "INSERT INTO profiles(profile_id,system_id,category,title,core_name,source_kind,source_detail)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(|e| format!("prepare profile insert: {e}"))?;
        for profile in launch_profiles::builtin_profiles() {
            stmt.execute(params![
                profile.id,
                profile.system_id,
                profile.category,
                profile.title,
                profile.core_name,
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
            let (size, mtime_secs) = scan
                .file_fingerprints
                .get(path)
                .copied()
                .unwrap_or((0, 0));
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
                Option::<&str>::None,
                entry.normalized_title.as_str(),
                Option::<&str>::None,
                Option::<i64>::None,
                Option::<i64>::None,
                if entry.launchable {
                    "candidate"
                } else {
                    "support"
                },
                entry.uncompressed_size.or(entry.compressed_size).unwrap_or(0) as i64,
                0i64,
                "archive-toc",
                "ZIP central directory header"
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
        let mut seen = HashSet::<String>::new();
        for discovery in &scan.discoveries {
            if !is_playable_discovery_with_coverage(discovery, &covered_payloads) {
                continue;
            }
            let key = discovery_unique_key(discovery);
            if !seen.insert(key.clone()) {
                continue;
            }
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
                DiscoverySourceKind::Mra | DiscoverySourceKind::Mgl => Some(discovery.launch_ref.as_str()),
                DiscoverySourceKind::PayloadFile => None,
            };
            let payload_path = if launcher_path.is_none() {
                Some(discovery.launch_ref.as_str())
            } else {
                None
            };
            let plan_launch_ref = launch_ref_for_discovery(&key, discovery);
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
        DiscoverySourceKind::PayloadFile => {
            !covered_payloads.contains(&normalize_launch_path(&d.launch_ref))
        }
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
        DiscoverySourceKind::PayloadFile => "virtual-mgl",
    }
}

fn launch_ref_for_discovery(game_id: &str, discovery: &GameDiscovery) -> String {
    match discovery.source_kind {
        DiscoverySourceKind::Mra | DiscoverySourceKind::Mgl => discovery.launch_ref.clone(),
        DiscoverySourceKind::PayloadFile => virtual_launch_ref(game_id),
    }
}

fn virtual_launch_ref(game_id: &str) -> String {
    format!("magik-plan:{game_id}")
}

fn profile_id_for_discovery(discovery: &GameDiscovery) -> Option<&str> {
    if discovery.platform_id == "unknown" || discovery.platform_id.is_empty() {
        None
    } else {
        Some(discovery.platform_id.as_str())
    }
}

fn is_launcher_launch_ref(path: &str) -> bool {
    if path.starts_with("magik-plan:") {
        return true;
    }
    match path_ext(path).as_deref() {
        Some("mra" | "mgl") => !path.contains("::"),
        _ => false,
    }
}

fn source_kind_str(kind: DiscoverySourceKind) -> &'static str {
    match kind {
        DiscoverySourceKind::Mra => "mra",
        DiscoverySourceKind::Mgl => "mgl",
        DiscoverySourceKind::PayloadFile => "payload",
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
        &["(europe", "(eu", "(e)", "[europe", "[eu]", " europe", " pal"],
    ) {
        Some("europe")
    } else if contains_any(
        &lower,
        &["(japan", "(jp", "(j)", "[japan", "[jp]", " japan", " ntsc-j"],
    ) {
        Some("japan")
    } else if contains_any(&lower, &["(world", "(w)", "[world", " world"]) {
        Some("world")
    } else {
        None
    }
}

fn catalog_system_id_for_discovery(discovery: &GameDiscovery) -> String {
    if discovery.category == "Arcade" {
        "arcade".to_string()
    } else if discovery.platform_id.is_empty() {
        "unknown".to_string()
    } else {
        discovery.platform_id.clone()
    }
}

fn system_title_for_discovery(discovery: &GameDiscovery, system_id: &str) -> String {
    if discovery.core_id.trim().is_empty() || discovery.core_id == "unknown" {
        system_id.to_string()
    } else {
        discovery.core_id.clone()
    }
}

fn is_index_candidate(profiles: &[LaunchProfile], path: &Path, _ext: &str) -> bool {
    matches!(
        classify_profile_path(profiles, path),
        Some((_, ProfilePathClass::Payload { .. } | ProfilePathClass::Ignored { .. }))
    ) || path
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("gamelist.xml"))
        .unwrap_or(false)
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

        assert!(classify_profile_path(
            &profiles,
            Path::new("/media/fat/_Computer/Amiga.mgl")
        )
        .is_none());
        assert!(classify_profile_path(
            &profiles,
            Path::new("/media/fat/_Console/Game Gear.mgl")
        )
        .is_none());
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

        let validated = validate_or_rebuild_directory_manifest(
            std::slice::from_ref(&root_key),
            &fingerprint,
        );

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
}
