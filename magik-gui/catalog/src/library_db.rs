//! Whole-MiSTer library database scanning and loading.
//!
//! This is deliberately TOC/header-only for archives. Indexing must never
//! decompress full game libraries just to make the launcher searchable.

use crate::arcade_catalog::{self, ArcadeCatalog, ArcadeGameEntry};
use rusqlite::{params, Connection, OpenFlags};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
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

const NORMAL_LAUNCH_EXTS: &[&str] = &[
    "mra", "mgl", "rbf", "rom", "bin", "cue", "iso", "img", "dsk", "vhd", "hdf", "adf", "ipf",
    "st", "msa", "tap", "tzx", "z80", "sna", "nes", "fds", "smc", "sfc", "gb", "gbc", "gba", "gg",
    "sms", "md", "gen", "32x", "pce", "vec", "n64", "z64", "v64", "neo", "chd",
];

const MRA_PREFIX_BYTES: usize = 160 * 1024;
type FileFingerprint = BTreeMap<String, (u64, i64)>;
type ProgressCallback<'a> = Option<&'a mut dyn FnMut(&str, &str)>;
type DirectoryManifest = BTreeMap<String, DirectorySignature>;

const SCHEMA_VERSION: u32 = 7;
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

    fn as_str(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::SevenZip => "7z",
            Self::Lha => "lha",
            Self::Lzh => "lzh",
            Self::Rar => "rar",
            Self::Chd => "chd",
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
    normal_files: Vec<String>,
    containers: Vec<LibraryContainer>,
    entries: Vec<LibraryContainerEntry>,
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

#[derive(Clone, Copy, Debug)]
enum DiscoverySourceKind {
    Mra,
    Mgl,
    PayloadFile,
    ArchiveEntry,
    Container,
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
    let root = root.as_ref().to_path_buf();
    let t = Instant::now();
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open library db: {e}"))?;
    let _ = conn.execute_batch("PRAGMA query_only=ON;");
    let mut stmt = conn
        .prepare(
            "SELECT title,
                    CASE WHEN source_kind='mgl' THEN source_path ELSE launch_ref END,
                    COALESCE(image_path,''), has_image,
                    CASE
                      WHEN category='Arcade'
                       AND (platform_id='neogeo'
                         OR lower(core_id) IN ('neogeo', 'neo geo', 'neo-geo')
                         OR hardware_id='snk-neo-geo'
                         OR lower(hardware_id) LIKE '%neo%geo%') THEN 'neogeo'
                      WHEN category='Arcade' THEN 'arcade'
                      WHEN platform_id='' THEN 'unknown'
                      ELSE platform_id
                    END,
                    source_kind,
                    COALESCE(setname,''),
                    COALESCE(parent,'')
             FROM discoveries
             WHERE launch_ref != ''
             ORDER BY lower(title)
             LIMIT 20000",
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
    rows_out.retain(|row| {
        is_launcher_launch_ref(&row.game.mra_path) && !is_support_file_path(&row.game.mra_path)
    });
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
    } else if let Some(report) = progress.as_mut() {
        report(
            "Indexing library",
            "No usable database fingerprint; full scan...",
        );
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

struct ArchiveScan {
    container: LibraryContainer,
    entries: Vec<LibraryContainerEntry>,
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
        if let Some(format) = ArchiveFormat::from_ext(&f.ext) {
            let scan = scan_archive_toc(&f, format);
            if containers.len() % 10 == 0 {
                if let Some(report) = progress.as_mut() {
                    report(
                        "Scanning archive TOCs",
                        &format!(
                            "{} archives; {} entries; {}",
                            containers.len() + 1,
                            entries.len() + scan.entries.len(),
                            f.path.display()
                        ),
                    );
                }
            }
            if format == ArchiveFormat::Chd || is_launchable_container(&f, format) {
                discoveries.push(discovery_from_file(&f, DiscoverySourceKind::Container));
            }
            if cfg.optional_catalogs {
                let catalog_discoveries = catalog_discoveries_from_container(&f, format);
                discoveries.extend(catalog_discoveries);
            }
            if !archive_entries_are_rom_parts(&f, format) {
                discoveries.extend(
                    scan.entries
                        .iter()
                        .filter(|e| e.launchable)
                        .map(discovery_from_archive_entry),
                );
            }
            containers.push(scan.container);
            entries.extend(scan.entries);
        } else if is_normal_launchable(&f.ext) {
            normal_files.push(f.path.display().to_string());
            let discovery = discovery_from_file(&f, source_kind_for_ext(&f.ext));
            discoveries.push(discovery);
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
        discoveries,
        discover_us,
        classify_us: classify_t.elapsed().as_micros() as u64,
    }
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
                if !is_index_candidate(p, &ext) {
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

fn scan_archive_toc(file: &FoundFile, format: ArchiveFormat) -> ArchiveScan {
    let t = Instant::now();
    let (status, entries) = match format {
        ArchiveFormat::Zip => match scan_zip_central_directory(file) {
            Ok(entries) => (ArchiveScanStatus::Ok, entries),
            Err(e) => (ArchiveScanStatus::Error(e), Vec::new()),
        },
        ArchiveFormat::Chd => (ArchiveScanStatus::HeaderOnly, Vec::new()),
        ArchiveFormat::SevenZip | ArchiveFormat::Lha | ArchiveFormat::Lzh | ArchiveFormat::Rar => {
            (ArchiveScanStatus::Unsupported, Vec::new())
        }
    };
    let scan_us = t.elapsed().as_micros() as u64;
    ArchiveScan {
        container: LibraryContainer {
            file_path: file.path.display().to_string(),
            format,
            size: file.size,
            mtime_secs: file.mtime_secs,
            entry_count: entries.len() as u32,
            scan_status: status,
            scan_us,
        },
        entries,
    }
}

fn source_kind_for_ext(ext: &str) -> DiscoverySourceKind {
    match ext {
        "mra" => DiscoverySourceKind::Mra,
        "mgl" => DiscoverySourceKind::Mgl,
        _ => DiscoverySourceKind::PayloadFile,
    }
}

fn is_launchable_container(file: &FoundFile, format: ArchiveFormat) -> bool {
    let path = file.path.to_string_lossy().to_ascii_lowercase();
    matches!(
        format,
        ArchiveFormat::Zip | ArchiveFormat::Lha | ArchiveFormat::Lzh
    ) && (path.contains("/games/mame/")
        || path.contains("/games/hbmame/")
        || path.contains("/games/neogeo/"))
}

fn archive_entries_are_rom_parts(file: &FoundFile, format: ArchiveFormat) -> bool {
    let path = file.path.to_string_lossy().to_ascii_lowercase();
    matches!(
        format,
        ArchiveFormat::Zip | ArchiveFormat::Lha | ArchiveFormat::Lzh
    ) && (path.contains("/games/mame/")
        || path.contains("/games/hbmame/")
        || path.contains("/games/neogeo/"))
}

fn catalog_discoveries_from_container(
    file: &FoundFile,
    format: ArchiveFormat,
) -> Vec<GameDiscovery> {
    if format != ArchiveFormat::SevenZip {
        return Vec::new();
    }
    let path = file.path.to_string_lossy();
    if !path
        .to_ascii_lowercase()
        .contains("/games/amiga/amigavision")
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    out.extend(amigavision_listing_discoveries(
        file,
        "games/Amiga/listings/games.txt",
        "AmigaVision",
    ));
    out.extend(amigavision_listing_discoveries(
        file,
        "games/Amiga/listings/demos.txt",
        "AmigaVision demos",
    ));
    out
}

fn amigavision_listing_discoveries(
    file: &FoundFile,
    entry_path: &str,
    genre: &str,
) -> Vec<GameDiscovery> {
    let tool = std::env::var("MISTER_7ZA").unwrap_or_else(|_| "/media/fat/linux/7za".to_string());
    let Ok(output) = Command::new(tool)
        .args(["e", "-so"])
        .arg(&file.path)
        .arg(entry_path)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|title| GameDiscovery {
            source_path: format!("{}::{entry_path}::{title}", file.path.display()),
            launch_ref: file.path.display().to_string(),
            source_kind: DiscoverySourceKind::CatalogEntry,
            title: title.to_string(),
            category: "Computer".to_string(),
            platform_id: "amiga".to_string(),
            core_id: "AmigaVision".to_string(),
            hardware_id: "commodore-amiga".to_string(),
            manufacturer: Some("Commodore".to_string()),
            genre: Some(genre.to_string()),
            year: None,
            setname: None,
            parent: None,
            image_path: None,
            has_image: false,
            confidence: DiscoveryConfidence::CatalogMetadata,
        })
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
    let neogeo_media = build_neogeo_media_index(roots);
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
            .or_else(|| by_stem.get(&match_stem(&discovery.launch_ref)))
            .or_else(|| lookup_neogeo_media(&neogeo_media, discovery));
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

fn build_neogeo_media_index(roots: &[String]) -> HashMap<String, GamelistMetadata> {
    let mut by_setname = HashMap::new();
    for dir in neogeo_media_dirs(roots) {
        let screenshots = dir.join("screenshots");
        if let Ok(entries) = fs::read_dir(&screenshots) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if !path.is_file() || path_ext_path(&path).as_deref() != Some("png") {
                    continue;
                }
                let Some(setname) = path.file_stem().and_then(|s| s.to_str()).map(normalize_id)
                else {
                    continue;
                };
                by_setname
                    .entry(setname)
                    .or_insert_with(|| GamelistMetadata {
                        title: None,
                        image_path: Some(path.display().to_string()),
                        has_image: true,
                    });
            }
        }

        for (game_path, meta) in parse_gamelist_metadata(&dir.join("gamelist.xml"), &dir) {
            let setname = match_stem(&game_path);
            let fallback = screenshots.join(format!("{setname}.png"));
            let image_path = if meta.has_image {
                meta.image_path
            } else if fallback.is_file() {
                Some(fallback.display().to_string())
            } else {
                meta.image_path
            };
            let has_image = image_path
                .as_deref()
                .map(|p| Path::new(p).is_file())
                .unwrap_or(false);
            by_setname.insert(
                setname,
                GamelistMetadata {
                    title: meta.title,
                    image_path,
                    has_image,
                },
            );
        }
    }
    by_setname
}

fn neogeo_media_dirs(roots: &[String]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for root in roots {
        let path = Path::new(root);
        for candidate in [
            path.to_path_buf(),
            path.join("NEOGEO"),
            path.join("games/NEOGEO"),
        ] {
            if !candidate.is_dir() {
                continue;
            }
            let Some(name) = candidate.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if !name.eq_ignore_ascii_case("NEOGEO") {
                continue;
            }
            let key = candidate.display().to_string();
            if seen.insert(key) {
                out.push(candidate);
            }
        }
    }
    out
}

fn lookup_neogeo_media<'a>(
    by_setname: &'a HashMap<String, GamelistMetadata>,
    discovery: &GameDiscovery,
) -> Option<&'a GamelistMetadata> {
    if by_setname.is_empty() || !is_neogeo_discovery(discovery) {
        return None;
    }
    neogeo_setname_candidates(discovery)
        .into_iter()
        .find_map(|setname| by_setname.get(&setname))
}

fn is_neogeo_discovery(discovery: &GameDiscovery) -> bool {
    discovery.platform_id == "neogeo"
        || discovery.core_id.eq_ignore_ascii_case("neogeo")
        || discovery.hardware_id == "snk-neo-geo"
        || discovery
            .source_path
            .to_ascii_lowercase()
            .contains("/games/neogeo/")
}

fn neogeo_setname_candidates(discovery: &GameDiscovery) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(setname) = discovery.setname.as_deref() {
        push_setname_candidate(&mut out, setname);
    }
    push_parenthesized_setname(&mut out, &discovery.source_path);
    push_parenthesized_setname(&mut out, &discovery.launch_ref);
    push_setname_candidate(&mut out, &match_stem(&discovery.source_path));
    push_setname_candidate(&mut out, &match_stem(&discovery.launch_ref));
    out
}

fn push_parenthesized_setname(out: &mut Vec<String>, path: &str) {
    let stem = Path::new(path.split("::").next().unwrap_or(path))
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path);
    if let Some(open) = stem.rfind('(') {
        if let Some(close) = stem[open + 1..].find(')') {
            push_setname_candidate(out, &stem[open + 1..open + 1 + close]);
        }
    }
}

fn push_setname_candidate(out: &mut Vec<String>, raw: &str) {
    let setname = normalize_id(raw);
    if setname == "unknown"
        || setname.len() > 32
        || !setname
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return;
    }
    if !out.iter().any(|existing| existing == &setname) {
        out.push(setname);
    }
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

fn match_stem(path: &str) -> String {
    normalize_id(
        Path::new(path.split("::").next().unwrap_or(path))
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(path),
    )
}

fn path_ext_path(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
}

fn discovery_from_archive_entry(entry: &LibraryContainerEntry) -> GameDiscovery {
    let mut taxonomy = taxonomy_from_path(&entry.entry_path, "");
    taxonomy.confidence = DiscoveryConfidence::ArchiveToc;
    GameDiscovery {
        source_path: format!("{}::{}", entry.file_path, entry.entry_path),
        launch_ref: entry.launch_ref.clone(),
        source_kind: DiscoverySourceKind::ArchiveEntry,
        title: title_from_path(&entry.entry_path),
        category: taxonomy.category,
        platform_id: taxonomy.platform_id,
        core_id: taxonomy.core_id,
        hardware_id: taxonomy.hardware_id,
        manufacturer: None,
        genre: None,
        year: None,
        setname: None,
        parent: None,
        image_path: None,
        has_image: false,
        confidence: taxonomy.confidence,
    }
}

fn discovery_from_file(file: &FoundFile, source_kind: DiscoverySourceKind) -> GameDiscovery {
    if file.ext == "mra" {
        if let Some(mra) = read_mra_metadata(&file.path) {
            let taxonomy = taxonomy_from_mra(&mra, &file.path.display().to_string());
            return GameDiscovery {
                source_path: file.path.display().to_string(),
                launch_ref: file.path.display().to_string(),
                source_kind,
                title: mra
                    .name
                    .unwrap_or_else(|| title_from_path(&file.path.display().to_string())),
                category: taxonomy.category,
                platform_id: taxonomy.platform_id,
                core_id: taxonomy.core_id,
                hardware_id: taxonomy.hardware_id,
                manufacturer: mra.manufacturer,
                genre: mra.category.or(mra.catver),
                year: mra.year.and_then(|s| s.parse::<u16>().ok()),
                setname: mra.setname,
                parent: mra.parent,
                image_path: None,
                has_image: false,
                confidence: taxonomy.confidence,
            };
        }
    }
    if file.ext == "mgl" {
        if let Some(mgl) = read_mgl_metadata(&file.path) {
            let taxonomy = taxonomy_from_mgl(&mgl, &file.path.display().to_string());
            return GameDiscovery {
                source_path: file.path.display().to_string(),
                launch_ref: file.path.display().to_string(),
                source_kind,
                title: title_from_path(&file.path.display().to_string()),
                category: taxonomy.category,
                platform_id: taxonomy.platform_id,
                core_id: taxonomy.core_id,
                hardware_id: taxonomy.hardware_id,
                manufacturer: None,
                genre: None,
                year: None,
                setname: None,
                parent: None,
                image_path: None,
                has_image: false,
                confidence: taxonomy.confidence,
            };
        }
    }

    let taxonomy = taxonomy_from_path(&file.path.display().to_string(), &file.ext);
    GameDiscovery {
        source_path: file.path.display().to_string(),
        launch_ref: file.path.display().to_string(),
        source_kind,
        title: title_from_path(&file.path.display().to_string()),
        category: taxonomy.category,
        platform_id: taxonomy.platform_id,
        core_id: taxonomy.core_id,
        hardware_id: taxonomy.hardware_id,
        manufacturer: None,
        genre: None,
        year: None,
        setname: None,
        parent: None,
        image_path: None,
        has_image: false,
        confidence: taxonomy.confidence,
    }
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

struct Taxonomy {
    category: String,
    platform_id: String,
    core_id: String,
    hardware_id: String,
    confidence: DiscoveryConfidence,
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

fn taxonomy_from_mra(mra: &MraMetadata, path: &str) -> Taxonomy {
    let raw = format!(
        "{} {} {}",
        mra.rbf.as_deref().unwrap_or(""),
        mra.platform.as_deref().unwrap_or(""),
        path
    );
    let mut taxonomy = taxonomy_from_arcade_hint(&raw);
    if taxonomy.hardware_id == "arcade-unknown" {
        taxonomy.core_id = normalize_id(mra.rbf.as_deref().unwrap_or("arcade"));
        taxonomy.hardware_id = mra
            .platform
            .as_deref()
            .filter(|p| !p.trim().is_empty())
            .map(normalize_id)
            .unwrap_or_else(|| taxonomy.core_id.clone());
        taxonomy.confidence = if mra.platform.is_some() {
            DiscoveryConfidence::MraHardware
        } else {
            DiscoveryConfidence::MraCore
        };
    }
    taxonomy
}

fn taxonomy_from_mgl(mgl: &MglMetadata, path: &str) -> Taxonomy {
    let payload = mgl.file_path.as_deref().unwrap_or(path);
    let mut taxonomy = taxonomy_from_path(payload, path_ext(payload).as_deref().unwrap_or(""));
    if taxonomy.platform_id == "unknown" {
        taxonomy = taxonomy_from_path(path, "mgl");
    }
    if let Some(rbf) = mgl.rbf.as_deref() {
        if taxonomy.core_id == taxonomy.platform_id || taxonomy.core_id == "unknown" {
            taxonomy.core_id = normalize_id(rbf);
        }
    }
    taxonomy
}

fn taxonomy_from_arcade_hint(raw: &str) -> Taxonomy {
    let hint = raw.to_ascii_lowercase();
    let (core, hardware, confidence) =
        if contains_any(&hint, &["cps2", "cps-2", "cps ii", "cps-ii"]) {
            ("CPS2", "capcom-cps2", DiscoveryConfidence::MraHardware)
        } else if contains_any(&hint, &["cps1", "cps-1", "cps i", "cps-i"]) {
            ("CPS1", "capcom-cps1", DiscoveryConfidence::MraHardware)
        } else if contains_any(&hint, &["cps3", "cps-3", "cps iii", "cps-iii"]) {
            ("CPS3", "capcom-cps3", DiscoveryConfidence::MraHardware)
        } else if contains_any(&hint, &["neogeo", "neo geo", "neo-geo"]) {
            ("NeoGeo", "snk-neo-geo", DiscoveryConfidence::MraHardware)
        } else if contains_any(&hint, &["irem m92", "m92 hardware", "irem-m92"]) {
            ("M92", "irem-m92", DiscoveryConfidence::MraHardware)
        } else if contains_any(&hint, &["irem m72", "irem-m72"]) {
            ("M72", "irem-m72", DiscoveryConfidence::MraHardware)
        } else if contains_any(&hint, &["system 16", "system16"]) {
            (
                "System16",
                "sega-system16",
                DiscoveryConfidence::MraHardware,
            )
        } else if contains_any(&hint, &["system 18", "system18"]) {
            (
                "System18",
                "sega-system18",
                DiscoveryConfidence::MraHardware,
            )
        } else if contains_any(&hint, &["toaplan", "twin cobra", "fshark"]) {
            ("Toaplan", "toaplan", DiscoveryConfidence::MraHardware)
        } else {
            ("arcade", "arcade-unknown", DiscoveryConfidence::MraCore)
        };

    Taxonomy {
        category: "Arcade".to_string(),
        platform_id: "arcade".to_string(),
        core_id: core.to_string(),
        hardware_id: hardware.to_string(),
        confidence,
    }
}

fn taxonomy_from_path(path: &str, ext: &str) -> Taxonomy {
    let lower = path.to_ascii_lowercase();
    if lower.contains("/_arcade/") || lower.contains("/games/mame/") || ext == "mra" {
        return taxonomy_from_arcade_hint(path);
    }
    if lower.contains("/games/hbmame/") {
        return simple_taxonomy("Arcade", "hbmame", "hbmame", "arcade-homebrew");
    }
    if lower.contains("/games/ao486/") {
        return simple_taxonomy("Computer", "ao486", "ao486", "pc-compatible");
    }
    if lower.contains("/games/saturn/") {
        return simple_taxonomy("Console", "saturn", "Saturn", "sega-saturn");
    }
    if lower.contains("/games/amiga/") || lower.contains("/_amiga/") {
        return simple_taxonomy("Computer", "amiga", "Amiga", "commodore-amiga");
    }
    if lower.contains("/games/neogeo/") || lower.contains("neo geo") {
        return simple_taxonomy("Arcade", "neogeo", "NeoGeo", "snk-neo-geo");
    }
    if lower.contains("/games/gba/") {
        return simple_taxonomy("Handheld", "gba", "GBA", "nintendo-gba");
    }
    if lower.contains("/games/snes/") {
        return simple_taxonomy("Console", "snes", "SNES", "nintendo-snes");
    }
    if lower.contains("/games/nes/") {
        return simple_taxonomy("Console", "nes", "NES", "nintendo-nes");
    }
    if lower.contains("/games/megadrive/") {
        return simple_taxonomy("Console", "megadrive", "MegaDrive", "sega-mega-drive");
    }
    if lower.contains("/games/n64/") {
        return simple_taxonomy("Console", "n64", "N64", "nintendo-n64");
    }
    if lower.contains("/games/gbc/") {
        return simple_taxonomy("Handheld", "gbc", "GBC", "nintendo-gbc");
    }
    if lower.contains("/games/gamegear/") {
        return simple_taxonomy("Handheld", "gamegear", "GameGear", "sega-game-gear");
    }
    if let Some(taxonomy) = taxonomy_from_games_folder(&lower) {
        return taxonomy;
    }

    Taxonomy {
        category: category_from_ext(ext).to_string(),
        platform_id: platform_from_ext(ext).to_string(),
        core_id: platform_from_ext(ext).to_string(),
        hardware_id: platform_from_ext(ext).to_string(),
        confidence: DiscoveryConfidence::Extension,
    }
}

fn taxonomy_from_games_folder(lower_path: &str) -> Option<Taxonomy> {
    let folder = games_folder(lower_path)?;
    let row = match folder {
        "3do" => ("Console", "3do", "3DO", "panasonic-3do"),
        "apple-i" => ("Computer", "apple-i", "Apple I", "apple-i"),
        "apple-ii" => ("Computer", "apple-ii", "Apple II", "apple-ii"),
        "apple-iigs" => ("Computer", "apple-iigs", "Apple IIGS", "apple-iigs"),
        "archie" => ("Computer", "archie", "Archimedes", "acorn-archimedes"),
        "atari2600" => ("Console", "atari2600", "Atari2600", "atari-2600"),
        "atari5200" => ("Console", "atari5200", "Atari5200", "atari-5200"),
        "atari7800" => ("Console", "atari7800", "Atari7800", "atari-7800"),
        "atari800" => ("Computer", "atari800", "Atari800", "atari-8-bit"),
        "atarilynx" => ("Handheld", "atarilynx", "AtariLynx", "atari-lynx"),
        "atarist" => ("Computer", "atarist", "AtariST", "atari-st"),
        "bbcmicro" => ("Computer", "bbcmicro", "BBCMicro", "bbc-micro"),
        "c128" => ("Computer", "c128", "C128", "commodore-c128"),
        "c16" => ("Computer", "c16", "C16", "commodore-c16"),
        "c64" => ("Computer", "c64", "C64", "commodore-c64"),
        "cd-i" => ("Console", "cdi", "CD-i", "philips-cdi"),
        "coleco" => ("Console", "coleco", "ColecoVision", "colecovision"),
        "gameboy" => ("Handheld", "gb", "Gameboy", "nintendo-game-boy"),
        "gameboy2p" => ("Handheld", "gb", "Gameboy", "nintendo-game-boy"),
        "jaguar" => ("Console", "jaguar", "Jaguar", "atari-jaguar"),
        "macplus" => ("Computer", "macplus", "MacPlus", "apple-macintosh"),
        "megacd" => ("Console", "megacd", "MegaCD", "sega-mega-cd"),
        "megaduck" => ("Handheld", "megaduck", "MegaDuck", "megaduck"),
        "msx" => ("Computer", "msx", "MSX", "msx"),
        "msx1" => ("Computer", "msx", "MSX", "msx"),
        "neogeo-cd" => ("Console", "neogeo-cd", "NeoGeoCD", "snk-neo-geo-cd"),
        "neogeopocket" => ("Handheld", "ngp", "NeoGeoPocket", "snk-neo-geo-pocket"),
        "odyssey2" => ("Console", "odyssey2", "Odyssey2", "magnavox-odyssey2"),
        "openbor" => ("Engine", "openbor", "OpenBOR", "openbor"),
        "pc8801" => ("Computer", "pc8801", "PC-8801", "nec-pc8801"),
        "pico-8" => ("Engine", "pico-8", "PICO-8", "pico-8"),
        "psx" => ("Console", "psx", "PlayStation", "sony-playstation"),
        "s32x" => ("Console", "s32x", "S32X", "sega-32x"),
        "sgb" => ("Console", "sgb", "SuperGameBoy", "nintendo-super-game-boy"),
        "sms" => ("Console", "sms", "MasterSystem", "sega-master-system"),
        "spectrum" => ("Computer", "spectrum", "Spectrum", "zx-spectrum"),
        "tgfx16" => ("Console", "tgfx16", "TurboGrafx16", "nec-pc-engine"),
        "tgfx16-cd" => ("Console", "tgfx16-cd", "TurboGrafx16CD", "nec-pc-engine-cd"),
        "trs-80" => ("Computer", "trs-80", "TRS-80", "trs-80"),
        "tsconf" => ("Computer", "tsconf", "TSConf", "tsconf"),
        "vic20" => ("Computer", "vic20", "VIC20", "commodore-vic20"),
        "vectrex" => ("Console", "vectrex", "Vectrex", "vectrex"),
        "wonderswan" => ("Handheld", "wonderswan", "WonderSwan", "bandai-wonderswan"),
        "wonderswancolor" => (
            "Handheld",
            "wonderswancolor",
            "WonderSwanColor",
            "bandai-wonderswan-color",
        ),
        "x68000" => ("Computer", "x68000", "X68000", "sharp-x68000"),
        "zx81" => ("Computer", "zx81", "ZX81", "zx81"),
        "zxnext" => ("Computer", "zxnext", "ZXNext", "zx-spectrum-next"),
        _ => return None,
    };
    Some(simple_taxonomy(row.0, row.1, row.2, row.3))
}

fn games_folder(lower_path: &str) -> Option<&str> {
    let rest = lower_path.split("/games/").nth(1)?;
    rest.split('/').next()
}

fn simple_taxonomy(category: &str, platform: &str, core: &str, hardware: &str) -> Taxonomy {
    Taxonomy {
        category: category.to_string(),
        platform_id: platform.to_string(),
        core_id: core.to_string(),
        hardware_id: hardware.to_string(),
        confidence: DiscoveryConfidence::PayloadPath,
    }
}

fn category_from_ext(ext: &str) -> &'static str {
    match ext {
        "gb" | "gbc" | "gba" | "gg" => "Handheld",
        "adf" | "hdf" | "dsk" | "vhd" | "st" | "msa" | "tap" | "tzx" | "z80" | "sna" => "Computer",
        "mra" => "Arcade",
        _ => "Unknown",
    }
}

fn platform_from_ext(ext: &str) -> &'static str {
    match ext {
        "nes" | "fds" => "nes",
        "smc" | "sfc" => "snes",
        "gb" => "gb",
        "gbc" => "gbc",
        "gba" => "gba",
        "gg" => "gamegear",
        "sms" => "mastersystem",
        "md" | "gen" => "megadrive",
        "n64" | "z64" | "v64" => "n64",
        "neo" => "neogeo",
        "adf" | "hdf" => "amiga",
        "chd" => "disc",
        "mra" => "arcade",
        _ => "unknown",
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
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

fn scan_zip_central_directory(file: &FoundFile) -> Result<Vec<LibraryContainerEntry>, String> {
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

    let cd_entries = le_u16(&tail[eocd + 10..eocd + 12]) as usize;
    let cd_size = le_u32(&tail[eocd + 12..eocd + 16]) as u64;
    let cd_offset = le_u32(&tail[eocd + 16..eocd + 20]) as u64;
    if cd_offset == u32::MAX as u64 || cd_size == u32::MAX as u64 || cd_entries == u16::MAX as usize
    {
        return Err("zip64 central directory is not supported yet".to_string());
    }
    if cd_offset + cd_size > len {
        return Err("zip central directory outside file".to_string());
    }

    f.seek(SeekFrom::Start(cd_offset))
        .map_err(|e| format!("seek zip central directory: {e}"))?;
    let mut cd = vec![0u8; cd_size as usize];
    f.read_exact(&mut cd)
        .map_err(|e| format!("read zip central directory: {e}"))?;

    let mut entries = Vec::with_capacity(cd_entries);
    let mut pos = 0usize;
    while pos + 46 <= cd.len() && entries.len() < cd_entries {
        if le_u32(&cd[pos..pos + 4]) != 0x0201_4b50 {
            return Err(format!("bad central directory signature at {pos}"));
        }
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
            let ext = Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let launchable = is_normal_launchable(&ext);
            entries.push(LibraryContainerEntry {
                file_path: file.path.display().to_string(),
                entry_path: name.clone(),
                normalized_title: normalize_title(&name),
                compressed_size: Some(compressed),
                uncompressed_size: Some(uncompressed),
                crc32: Some(crc32),
                launchable,
                launch_ref: format!("{}::{name}", file.path.display()),
            });
        }
        pos = name_end + extra_len + comment_len;
    }
    Ok(entries)
}

fn unique_discovery_count(discoveries: &[GameDiscovery]) -> usize {
    discoveries
        .iter()
        .filter(|d| is_playable_discovery(d))
        .map(discovery_unique_key)
        .collect::<HashSet<_>>()
        .len()
}

fn archive_status_str(status: &ArchiveScanStatus) -> &'static str {
    match status {
        ArchiveScanStatus::Ok => "ok",
        ArchiveScanStatus::HeaderOnly => "header-only",
        ArchiveScanStatus::Unsupported => "unsupported",
        ArchiveScanStatus::Error(_) => "error",
    }
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
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove old sqlite: {e}")),
    }

    let mut conn = Connection::open(path).map_err(|e| format!("open sqlite: {e}"))?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode=OFF;
        PRAGMA synchronous=OFF;
        PRAGMA temp_store=MEMORY;
        PRAGMA locking_mode=EXCLUSIVE;
        CREATE TABLE normal_files (
            path TEXT PRIMARY KEY
        ) WITHOUT ROWID;
        CREATE TABLE containers (
            file_path TEXT PRIMARY KEY,
            size INTEGER NOT NULL,
            mtime_secs INTEGER NOT NULL,
            format TEXT NOT NULL,
            entry_count INTEGER NOT NULL,
            scan_status TEXT NOT NULL,
            scan_us INTEGER NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE entries (
            launch_ref TEXT PRIMARY KEY,
            file_path TEXT NOT NULL,
            entry_path TEXT NOT NULL,
            title TEXT NOT NULL,
            launchable INTEGER NOT NULL,
            compressed_size INTEGER,
            uncompressed_size INTEGER,
            crc32 INTEGER
        ) WITHOUT ROWID;
        CREATE TABLE discoveries (
            key TEXT PRIMARY KEY,
            source_path TEXT NOT NULL,
            launch_ref TEXT NOT NULL,
            source_kind TEXT NOT NULL,
            title TEXT NOT NULL,
            category TEXT NOT NULL,
            platform_id TEXT NOT NULL,
            core_id TEXT NOT NULL,
            hardware_id TEXT NOT NULL,
            manufacturer TEXT,
            genre TEXT,
            year INTEGER,
            setname TEXT,
            parent TEXT,
            image_path TEXT,
            has_image INTEGER NOT NULL,
            confidence TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE VIRTUAL TABLE discoveries_fts USING fts5(
            key UNINDEXED,
            title,
            launch_ref,
            platform_id,
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
            .prepare("INSERT INTO normal_files(path) VALUES (?1)")
            .map_err(|e| format!("prepare normal insert: {e}"))?;
        for path in &scan.normal_files {
            stmt.execute([path.as_str()])
                .map_err(|e| format!("insert normal file: {e}"))?;
        }
    }
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO containers(file_path,size,mtime_secs,format,entry_count,scan_status,scan_us)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(|e| format!("prepare container insert: {e}"))?;
        for container in &scan.containers {
            stmt.execute(params![
                container.file_path.as_str(),
                container.size as i64,
                container.mtime_secs,
                container.format.as_str(),
                container.entry_count as i64,
                archive_status_str(&container.scan_status),
                container.scan_us as i64
            ])
            .map_err(|e| format!("insert container: {e}"))?;
        }
    }
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO entries(launch_ref,file_path,entry_path,title,launchable,compressed_size,uncompressed_size,crc32)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            )
            .map_err(|e| format!("prepare entry insert: {e}"))?;
        for entry in &scan.entries {
            stmt.execute(params![
                entry.launch_ref.as_str(),
                entry.file_path.as_str(),
                entry.entry_path.as_str(),
                entry.normalized_title.as_str(),
                if entry.launchable { 1 } else { 0 },
                entry.compressed_size.map(|n| n as i64),
                entry.uncompressed_size.map(|n| n as i64),
                entry.crc32.map(|n| n as i64)
            ])
            .map_err(|e| format!("insert entry: {e}"))?;
        }
    }
    {
        let mut row_stmt = tx
            .prepare(
                "INSERT INTO discoveries(key,source_path,launch_ref,source_kind,title,category,platform_id,core_id,hardware_id,manufacturer,genre,year,setname,parent,image_path,has_image,confidence)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
            )
            .map_err(|e| format!("prepare discovery insert: {e}"))?;
        let mut fts_stmt = tx
            .prepare(
                "INSERT INTO discoveries_fts(key,title,launch_ref,platform_id,core_id,hardware_id)
                 VALUES (?1,?2,?3,?4,?5,?6)",
            )
            .map_err(|e| format!("prepare discovery fts insert: {e}"))?;
        let mut seen = HashSet::<String>::new();
        for discovery in &scan.discoveries {
            if !is_playable_discovery(discovery) {
                continue;
            }
            let key = discovery_unique_key(discovery);
            if !seen.insert(key.clone()) {
                continue;
            }
            row_stmt
                .execute(params![
                    key.as_str(),
                    discovery.source_path.as_str(),
                    discovery.launch_ref.as_str(),
                    source_kind_str(discovery.source_kind),
                    discovery.title.as_str(),
                    discovery.category.as_str(),
                    discovery.platform_id.as_str(),
                    discovery.core_id.as_str(),
                    discovery.hardware_id.as_str(),
                    discovery.manufacturer.as_deref(),
                    discovery.genre.as_deref(),
                    discovery.year.map(|n| n as i64),
                    discovery.setname.as_deref(),
                    discovery.parent.as_deref(),
                    discovery.image_path.as_deref(),
                    if discovery.has_image { 1 } else { 0 },
                    confidence_str(discovery.confidence)
                ])
                .map_err(|e| format!("insert discovery: {e}"))?;
            fts_stmt
                .execute(params![
                    key.as_str(),
                    discovery.title.as_str(),
                    discovery.launch_ref.as_str(),
                    discovery.platform_id.as_str(),
                    discovery.core_id.as_str(),
                    discovery.hardware_id.as_str()
                ])
                .map_err(|e| format!("insert discovery fts: {e}"))?;
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
    tx.commit().map_err(|e| format!("commit sqlite tx: {e}"))?;
    std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("stat sqlite: {e}"))
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
        DiscoverySourceKind::PayloadFile | DiscoverySourceKind::Container => {
            format!("payload:{}", d.launch_ref)
        }
        DiscoverySourceKind::ArchiveEntry => format!("archive:{}", d.launch_ref),
        DiscoverySourceKind::CatalogEntry => format!("catalog:{}:{}", d.launch_ref, d.title),
    }
}

fn is_playable_discovery(d: &GameDiscovery) -> bool {
    match d.source_kind {
        DiscoverySourceKind::Mra => !is_support_file_path(&d.launch_ref),
        DiscoverySourceKind::Mgl | DiscoverySourceKind::CatalogEntry => {
            is_launcher_launch_ref(&d.launch_ref) && !is_support_file_path(&d.launch_ref)
        }
        DiscoverySourceKind::PayloadFile
        | DiscoverySourceKind::ArchiveEntry
        | DiscoverySourceKind::Container => false,
    }
}

fn is_launcher_launch_ref(path: &str) -> bool {
    match path_ext(path).as_deref() {
        Some("mra" | "mgl") => !path.contains("::"),
        Some("7z") => is_amigavision_archive_path(path),
        _ => false,
    }
}

fn is_amigavision_archive_path(path: &str) -> bool {
    path.to_ascii_lowercase()
        .contains("/games/amiga/amigavision")
}

fn is_support_file_path(path: &str) -> bool {
    path.split("::").any(is_support_file_part)
}

fn is_support_file_part(path: &str) -> bool {
    let ext = path_ext(path).unwrap_or_default();
    if ext == "mra" {
        return false;
    }
    if ext == "rbf" || is_menu_launcher_path(path, &ext) {
        return true;
    }

    let file_stem = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let key = support_name_key(&file_stem);
    if is_boot_helper_key(&key)
        || matches!(
            key.as_str(),
            "bios"
                | "backgroundgen2"
                | "basic4k32"
                | "blank"
                | "bootloader"
                | "bootrom"
                | "cdbios"
                | "cd32"
                | "disk605"
                | "dolphindos20"
                | "empty"
                | "emptyhdd"
                | "firmware"
                | "fw"
                | "ipl"
                | "iplrom"
                | "kanji"
                | "kick"
                | "kick13"
                | "kick20"
                | "kick31"
                | "kickstart"
                | "misterboot"
                | "neocd"
                | "os"
                | "riscos"
                | "speeddosplus27"
                | "supergameboy"
                | "supergameboy2"
                | "system"
                | "topsp1"
                | "unibioscd"
        )
        || key.contains("bios")
    {
        return true;
    }

    let lower = path.to_ascii_lowercase();
    lower.split('/').any(|component| {
        let component = support_name_key(component);
        matches!(
            component.as_str(),
            "bios" | "boot" | "bootloader" | "firmware" | "fw" | "kickstart"
        )
    })
}

fn is_menu_launcher_path(path: &str, ext: &str) -> bool {
    if ext != "mgl" {
        return false;
    }
    let lower = path.to_ascii_lowercase();
    lower.starts_with("/media/fat/_computer/")
        || lower.starts_with("/media/fat/_console/")
        || lower.starts_with("/media/fat/_other/")
        || lower.starts_with("/media/fat/_utility/")
}

fn support_name_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn is_boot_helper_key(key: &str) -> bool {
    key == "boot"
        || key
            .strip_prefix("boot")
            .map(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false)
}

fn source_kind_str(kind: DiscoverySourceKind) -> &'static str {
    match kind {
        DiscoverySourceKind::Mra => "mra",
        DiscoverySourceKind::Mgl => "mgl",
        DiscoverySourceKind::PayloadFile => "payload",
        DiscoverySourceKind::ArchiveEntry => "archive-entry",
        DiscoverySourceKind::Container => "container",
        DiscoverySourceKind::CatalogEntry => "catalog-entry",
    }
}

fn find_eocd(buf: &[u8]) -> Option<usize> {
    if buf.len() < 22 {
        return None;
    }
    (0..=buf.len() - 4)
        .rev()
        .find(|&i| buf[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
}

fn le_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn is_normal_launchable(ext: &str) -> bool {
    NORMAL_LAUNCH_EXTS.contains(&ext)
}

fn is_index_candidate(path: &Path, ext: &str) -> bool {
    is_normal_launchable(ext)
        || ArchiveFormat::from_ext(ext).is_some()
        || path
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
    fn eocd_search_finds_last_signature() {
        let mut data = b"PK\x05\x06 noise".to_vec();
        data.extend_from_slice(&[0; 22]);
        data.extend_from_slice(b"abcPK\x05\x06");
        assert_eq!(find_eocd(&data), Some(data.len() - 4));
    }

    #[test]
    fn archive_format_from_ext() {
        assert_eq!(ArchiveFormat::from_ext("zip"), Some(ArchiveFormat::Zip));
        assert_eq!(ArchiveFormat::from_ext("7z"), Some(ArchiveFormat::SevenZip));
        assert_eq!(ArchiveFormat::from_ext("chd"), Some(ArchiveFormat::Chd));
        assert_eq!(ArchiveFormat::from_ext("mra"), None);
    }

    #[test]
    fn support_roms_do_not_count_as_games() {
        let discoveries = vec![
            payload("/media/fat/games/Saturn/boot.rom"),
            payload("/media/fat/games/PSX/bios/scph5501.bin"),
            payload("/media/fat/games/Amiga/Kickstart.rom"),
            payload("/media/fat/games/3DO/kanji.rom"),
            payload("/media/fat/games/ARCHIE/riscos.rom"),
            payload("/media/fat/games/C64/DolphinDOS_2.0.rom"),
            payload("/media/fat/games/MACPLUS/Disk605.dsk"),
            payload("/media/fat/games/NeoGeo-CD/uni-bioscd.rom"),
            payload("/media/fat/games/SGB/Super Game Boy.sfc"),
            payload("/media/fat/games/X68000/boot3.vhd"),
            payload("/media/fat/games/Altair8800/basic4k32.rom"),
            payload("/media/fat/games/Tamagotchi/background_gen2.bin"),
            payload("/media/fat/games/AmigaCD32/CD32.rom"),
        ];

        assert_eq!(unique_discovery_count(&discoveries), 0);
    }

    #[test]
    fn raw_payloads_do_not_count_as_launchable_games() {
        let discoveries = vec![
            payload("/media/fat/games/NES/Super Mario Bros.nes"),
            payload("/media/fat/games/MegaDrive/Bio-Hazard Battle.md"),
            payload("/media/fat/games/Saturn/Guardian Heroes.cue"),
            payload("/media/fat/games/NES/Boot Hill.nes"),
        ];

        assert_eq!(unique_discovery_count(&discoveries), 0);
    }

    #[test]
    fn rbf_cores_do_not_count_as_games() {
        let discoveries = vec![
            payload("/media/fat/_Computer/AcornAtom_20251001.rbf"),
            payload("/media/fat/_Console/Gameboy_20250618.rbf"),
            payload("/media/fat/_LLAPI/NES_LLAPI_20251206.rbf"),
            payload("/media/fat/_YCArcade/cores/Arkanoid_20220517.rbf"),
        ];

        assert_eq!(unique_discovery_count(&discoveries), 0);
    }

    #[test]
    fn menu_mgl_launchers_do_not_count_as_games() {
        let discoveries = vec![
            mgl(
                "/media/fat/_Computer/Amiga.mgl",
                "/media/fat/_Computer/Amiga.mgl",
            ),
            mgl(
                "/media/fat/_Console/Game Gear.mgl",
                "/media/fat/_Console/Game Gear.mgl",
            ),
        ];

        assert_eq!(unique_discovery_count(&discoveries), 0);
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

        let discovery = discovery_from_file(&file, DiscoverySourceKind::Mgl);

        assert_eq!(discovery.source_path, path.display().to_string());
        assert_eq!(discovery.launch_ref, path.display().to_string());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn neogeo_mgl_uses_setname_screenshot_from_neogeo_folder() {
        let root = unique_temp_dir("neogeo-media");
        let neogeo = root.join("games/NEOGEO");
        let screenshots = neogeo.join("screenshots");
        std::fs::create_dir_all(&screenshots).expect("create screenshots");
        std::fs::write(screenshots.join("mslug3.png"), b"png").expect("write screenshot");
        std::fs::write(
            neogeo.join("gamelist.xml"),
            r#"
            <gameList>
              <game>
                <path>./mslug3.zip</path>
                <name>Metal Slug 3</name>
                <image>./../../../../Volumes/MiSTer_Data/games/NEOGEO/screenshots/mslug3.png</image>
              </game>
            </gameList>
            "#,
        )
        .expect("write gamelist");
        let mut discoveries = vec![GameDiscovery {
            source_path:
                "/media/fat/_Games/_Neo Geo MVS & AES/_ World A-Z/Metal Slug 3 (mslug3).mgl"
                    .to_string(),
            launch_ref:
                "/media/fat/_Games/_Neo Geo MVS & AES/_ World A-Z/Metal Slug 3 (mslug3).mgl"
                    .to_string(),
            source_kind: DiscoverySourceKind::Mgl,
            title: "Metal Slug 3 (mslug3)".to_string(),
            category: "Arcade".to_string(),
            platform_id: "neogeo".to_string(),
            core_id: "NeoGeo".to_string(),
            hardware_id: "snk-neo-geo".to_string(),
            manufacturer: None,
            genre: None,
            year: None,
            setname: None,
            parent: None,
            image_path: None,
            has_image: false,
            confidence: DiscoveryConfidence::PayloadPath,
        }];

        let matched = enrich_discoveries_from_gamelists(
            &mut discoveries,
            &[root.join("games").display().to_string()],
            None,
        );

        assert_eq!(matched, 1);
        assert_eq!(discoveries[0].title, "Metal Slug 3");
        assert_eq!(
            discoveries[0].image_path.as_deref(),
            Some(
                screenshots
                    .join("mslug3.png")
                    .display()
                    .to_string()
                    .as_str()
            )
        );
        assert!(discoveries[0].has_image);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn neogeo_mgl_can_use_screenshot_file_without_gamelist_row() {
        let root = unique_temp_dir("neogeo-screenshot-only");
        let screenshots = root.join("NEOGEO/screenshots");
        std::fs::create_dir_all(&screenshots).expect("create screenshots");
        std::fs::write(screenshots.join("aof2a.png"), b"png").expect("write screenshot");
        let mut discoveries = vec![GameDiscovery {
            source_path: "/media/fat/_Games/_Neo Geo MVS & AES/Art of Fighting 2 (AES) (aof2a).mgl"
                .to_string(),
            launch_ref: "/media/fat/_Games/_Neo Geo MVS & AES/Art of Fighting 2 (AES) (aof2a).mgl"
                .to_string(),
            source_kind: DiscoverySourceKind::Mgl,
            title: "Art of Fighting 2 (AES) (aof2a)".to_string(),
            category: "Arcade".to_string(),
            platform_id: "neogeo".to_string(),
            core_id: "NeoGeo".to_string(),
            hardware_id: "snk-neo-geo".to_string(),
            manufacturer: None,
            genre: None,
            year: None,
            setname: None,
            parent: None,
            image_path: None,
            has_image: false,
            confidence: DiscoveryConfidence::PayloadPath,
        }];

        let matched = enrich_discoveries_from_gamelists(
            &mut discoveries,
            &[root.display().to_string()],
            None,
        );

        assert_eq!(matched, 1);
        assert_eq!(
            discoveries[0].image_path.as_deref(),
            Some(screenshots.join("aof2a.png").display().to_string().as_str())
        );
        assert!(discoveries[0].has_image);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn neogeo_setname_does_not_need_digits() {
        let root = unique_temp_dir("neogeo-no-digit-setname");
        let screenshots = root.join("NEOGEO/screenshots");
        std::fs::create_dir_all(&screenshots).expect("create screenshots");
        std::fs::write(screenshots.join("samsho.png"), b"png").expect("write screenshot");
        let mut discoveries = vec![GameDiscovery {
            source_path: "/media/fat/_Games/_Neo Geo MVS & AES/Samurai Shodown (samsho).mgl"
                .to_string(),
            launch_ref: "/media/fat/_Games/_Neo Geo MVS & AES/Samurai Shodown (samsho).mgl"
                .to_string(),
            source_kind: DiscoverySourceKind::Mgl,
            title: "Samurai Shodown (samsho)".to_string(),
            category: "Arcade".to_string(),
            platform_id: "neogeo".to_string(),
            core_id: "NeoGeo".to_string(),
            hardware_id: "snk-neo-geo".to_string(),
            manufacturer: None,
            genre: None,
            year: None,
            setname: None,
            parent: None,
            image_path: None,
            has_image: false,
            confidence: DiscoveryConfidence::PayloadPath,
        }];

        let matched = enrich_discoveries_from_gamelists(
            &mut discoveries,
            &[root.display().to_string()],
            None,
        );

        assert_eq!(matched, 1);
        assert_eq!(
            discoveries[0].image_path.as_deref(),
            Some(
                screenshots
                    .join("samsho.png")
                    .display()
                    .to_string()
                    .as_str()
            )
        );
        assert!(discoveries[0].has_image);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_catalog_entries_count_as_games() {
        let mut discovery = GameDiscovery {
            source_path: "/media/fat/games/Amiga/AmigaVision.7z::games.txt::Agony".to_string(),
            launch_ref: "/media/fat/games/Amiga/AmigaVision.7z".to_string(),
            source_kind: DiscoverySourceKind::CatalogEntry,
            title: "Agony".to_string(),
            category: "Computer".to_string(),
            platform_id: "amiga".to_string(),
            core_id: "AmigaVision".to_string(),
            hardware_id: "commodore-amiga".to_string(),
            manufacturer: None,
            genre: None,
            year: None,
            setname: None,
            parent: None,
            image_path: None,
            has_image: false,
            confidence: DiscoveryConfidence::CatalogMetadata,
        };

        assert!(is_playable_discovery(&discovery));
        assert_eq!(unique_discovery_count(&[discovery.clone()]), 1);

        discovery.launch_ref = "/media/fat/games/Other/System.7z".to_string();
        assert!(!is_playable_discovery(&discovery));

        discovery.launch_ref = "/media/fat/games/Amiga/Agony.mgl".to_string();
        assert!(is_playable_discovery(&discovery));
    }

    #[test]
    fn support_archive_entries_do_not_count_as_games() {
        let discoveries = vec![archive_entry(
            "/media/fat/games/MACPLUS/empty_hdd.zip::boot.vhd",
        )];

        assert_eq!(unique_discovery_count(&discoveries), 0);
    }

    #[test]
    fn mgl_boot_helpers_do_not_count_as_games() {
        let discovery = GameDiscovery {
            source_path: "/media/fat/games/TGFX16/mister-boot.mgl".to_string(),
            launch_ref: "/media/fat/games/TGFX16/mister-boot.pce".to_string(),
            source_kind: DiscoverySourceKind::Mgl,
            title: "mister-boot".to_string(),
            category: "Console".to_string(),
            platform_id: "tgfx16".to_string(),
            core_id: "TurboGrafx16".to_string(),
            hardware_id: "nec-pc-engine".to_string(),
            manufacturer: None,
            genre: None,
            year: None,
            setname: None,
            parent: None,
            image_path: None,
            has_image: false,
            confidence: DiscoveryConfidence::PayloadPath,
        };

        assert!(!is_playable_discovery(&discovery));
    }

    #[test]
    fn mra_files_are_not_filtered_by_support_names() {
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
        let rom = root.join("same-second.nes");
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
        std::fs::write(root.join("unchanged.nes"), b"rom").expect("write rom");
        let root_key = root.display().to_string();
        let manifest = build_directory_manifest(std::slice::from_ref(&root_key), None);
        let fingerprint = fingerprint_with_manifest(manifest.clone());

        let validated = validate_or_rebuild_directory_manifest(&[root_key], &fingerprint);

        assert_eq!(validated, Some(manifest));
        let _ = std::fs::remove_dir_all(root);
    }

    fn payload(path: &str) -> GameDiscovery {
        let ext = path_ext(path).unwrap_or_default();
        let taxonomy = taxonomy_from_path(path, &ext);
        GameDiscovery {
            source_path: path.to_string(),
            launch_ref: path.to_string(),
            source_kind: DiscoverySourceKind::PayloadFile,
            title: title_from_path(path),
            category: taxonomy.category,
            platform_id: taxonomy.platform_id,
            core_id: taxonomy.core_id,
            hardware_id: taxonomy.hardware_id,
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

    fn archive_entry(path: &str) -> GameDiscovery {
        GameDiscovery {
            source_path: path.to_string(),
            launch_ref: path.to_string(),
            source_kind: DiscoverySourceKind::ArchiveEntry,
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
            confidence: DiscoveryConfidence::ArchiveToc,
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
