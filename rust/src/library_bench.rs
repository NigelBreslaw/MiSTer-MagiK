//! Whole-MiSTer library benchmark: normal files + archive containers.
//!
//! This is deliberately TOC/header-only for archives. Indexing must never
//! decompress full game libraries just to make the launcher searchable.

use crate::arcade_catalog::{ArcadeCatalog, ArcadeGameEntry};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
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

pub const DEFAULT_SQLITE_PATH: &str = "/media/fat/mister-magic/library.sqlite3";
const LEGACY_BENCH_SQLITE_PATH: &str = "/media/fat/mister-magic/library-bench.sqlite3";

const NORMAL_LAUNCH_EXTS: &[&str] = &[
    "mra", "mgl", "rbf", "rom", "bin", "cue", "iso", "img", "dsk", "vhd", "hdf", "adf", "ipf",
    "st", "msa", "tap", "tzx", "z80", "sna", "nes", "fds", "smc", "sfc", "gb", "gbc", "gba", "gg",
    "sms", "md", "gen", "32x", "pce", "vec", "n64", "z64", "v64", "neo", "chd",
];

const MRA_PREFIX_BYTES: usize = 160 * 1024;
type FileFingerprint = BTreeMap<String, (u64, i64)>;

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
    normal_files: Vec<String>,
    containers: Vec<LibraryContainer>,
    entries: Vec<LibraryContainerEntry>,
    discoveries: Vec<GameDiscovery>,
    root_stats: HashMap<String, RootStats>,
    format_stats: BTreeMap<ArchiveFormat, FormatStats>,
    phase_stats: PhaseStats,
    discover_us: u64,
    classify_us: u64,
    largest_archives: Vec<(u64, String)>,
    slowest_archives: Vec<(u64, String)>,
}

pub struct LibraryCatalogLoad {
    pub catalog: ArcadeCatalog,
    pub us: u64,
    pub rows: usize,
}

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

#[derive(Clone, Debug, Default)]
struct RootStats {
    files: u64,
    normal_launchables: u64,
    archives: u64,
    scan_us: u64,
}

#[derive(Clone, Debug, Default)]
struct FormatStats {
    containers: u64,
    entries: u64,
    launchable_entries: u64,
    scan_us: u64,
    skipped: u64,
    errors: u64,
}

#[derive(Clone, Debug, Default)]
struct PhaseStats {
    mra: TimedCount,
    mgl: TimedCount,
    payload: TimedCount,
    archive_toc: TimedCount,
    optional_catalog: TimedCount,
    optional_catalog_entries: u64,
}

#[derive(Clone, Debug, Default)]
struct TimedCount {
    count: u64,
    us: u64,
}

impl TimedCount {
    fn add(&mut self, elapsed_us: u64) {
        self.count += 1;
        self.us += elapsed_us;
    }
}

#[derive(Clone)]
struct FoundFile {
    path: PathBuf,
    root: String,
    ext: String,
    size: u64,
    mtime_secs: i64,
}

pub fn run() {
    let cfg = BenchConfig::from_env();
    let bench_start = Instant::now();
    println!("library-bench roots={}", cfg.roots.join("|"));
    println!("library-bench sqlite_path={}", cfg.sqlite_path.display());
    println!("library-bench archive_toc=header-only no-decompress");
    println!(
        "library-bench optional_catalogs={}",
        if cfg.optional_catalogs { "on" } else { "off" }
    );

    benchmark_sqlite_cached_open(&cfg.sqlite_path);

    let scan = scan_library(&cfg);
    println!(
        "library_bench_initial_scan_tsv\troot_discovery\t{}\tfiles={}",
        scan.discover_us,
        scan.root_stats.values().map(|s| s.files).sum::<u64>()
    );
    println!(
        "library_bench_initial_scan_tsv\tclassify_and_archive_toc\t{}\tnormal_files={}\tcontainers={}\tcontainer_entries={}",
        scan.classify_us,
        scan.normal_files.len(),
        scan.containers.len(),
        scan.entries.len()
    );

    print_root_stats("library_bench_initial_root_tsv", &scan.root_stats);
    print_format_stats("library_bench_initial_archive_tsv", &scan.format_stats);
    print_phase_stats("library_bench_initial_phase_tsv", &scan.phase_stats);
    print_taxonomy_stats(&scan.discoveries);
    print_unknown_stats(&scan.discoveries);
    print_top("largest_archive", &mut scan.largest_archives.clone(), 10);
    print_top(
        "slowest_archive_toc",
        &mut scan.slowest_archives.clone(),
        10,
    );

    benchmark_memory_queries(&scan);
    benchmark_sqlite_backend(&cfg.sqlite_path, &scan);
    benchmark_second_scan(&cfg, &scan);

    println!(
        "library_bench_total_tsv\ttotal\t{}\tbackend=sqlite",
        bench_start.elapsed().as_micros()
    );
}

pub fn run_db_bench() {
    let cfg = BenchConfig::from_env();
    println!("library-db-bench sqlite_path={}", cfg.sqlite_path.display());
    benchmark_sqlite_cached_open(&cfg.sqlite_path);
    benchmark_sqlite_queries(&cfg.sqlite_path);
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
    let legacy = Path::new(LEGACY_BENCH_SQLITE_PATH);
    if legacy != path {
        match std::fs::remove_file(legacy) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("failed to delete {}: {e}", legacy.display())),
        }
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
            "SELECT title, launch_ref, COALESCE(image_path,''), has_image
             FROM discoveries
             WHERE category='Arcade' AND source_kind='mra'
             ORDER BY lower(title)
             LIMIT 5000",
        )
        .map_err(|e| format!("prepare arcade catalog query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ArcadeGameEntry {
                title: row.get::<_, String>(0)?,
                mra_path: row.get::<_, String>(1)?,
                image_path: row.get::<_, String>(2)?,
                has_image: row.get::<_, i64>(3)? != 0,
            })
        })
        .map_err(|e| format!("query arcade catalog: {e}"))?;
    let mut games = Vec::new();
    for row in rows {
        games.push(row.map_err(|e| format!("read arcade catalog row: {e}"))?);
    }
    let rows = games.len();
    Ok(LibraryCatalogLoad {
        catalog: ArcadeCatalog { root, games },
        us: t.elapsed().as_micros() as u64,
        rows,
    })
}

pub fn refresh_default_sqlite_database(
    mut progress: Option<&mut dyn FnMut(&str, &str)>,
) -> Result<LibraryRefreshSummary, String> {
    let cfg = BenchConfig::production();
    let scan_t = Instant::now();
    if let Some(existing) = read_sqlite_fingerprint(&cfg.sqlite_path) {
        if let Some(report) = progress.as_mut() {
            report("Checking library", "Looking for changed files...");
        }
        let (files, _) = match progress.as_mut() {
            Some(report) => discover_files(&cfg.roots, Some(&mut **report)),
            None => discover_files(&cfg.roots, None),
        };
        let current = file_fingerprint_from_files(&files);
        let scan_us = scan_t.elapsed().as_micros() as u64;
        if current == existing.file_fingerprints {
            if let Some(report) = progress.as_mut() {
                report(
                    "Library unchanged",
                    &format!(
                        "{} files checked; using cached database",
                        current.len()
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
                &format!("{} files checked; rebuilding database...", current.len()),
            );
        }
    } else if let Some(report) = progress.as_mut() {
        report("Indexing library", "No usable database fingerprint; full scan...");
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
    mut progress: Option<&mut dyn FnMut(&str, &str)>,
) -> LibraryScan {
    let discover_t = Instant::now();
    let (files, mut root_stats) = match progress.as_mut() {
        Some(report) => discover_files(&cfg.roots, Some(&mut **report)),
        None => discover_files(&cfg.roots, None),
    };
    let discover_us = discover_t.elapsed().as_micros() as u64;
    let file_fingerprints = file_fingerprint_from_files(&files);
    if let Some(report) = progress.as_mut() {
        report(
            "Classifying library",
            &format!("Discovered {} files across MiSTer roots", files.len()),
        );
    }

    let mut normal_files = Vec::new();
    let mut containers = Vec::new();
    let mut entries = Vec::new();
    let mut discoveries = Vec::new();
    let mut format_stats = BTreeMap::<ArchiveFormat, FormatStats>::new();
    let mut phase_stats = PhaseStats::default();
    let mut largest_archives = Vec::<(u64, String)>::new();
    let mut slowest_archives = Vec::<(u64, String)>::new();

    let classify_t = Instant::now();
    let total_files = files.len();
    for (idx, f) in files.into_iter().enumerate() {
        if idx % 250 == 0 {
            if let Some(report) = progress.as_mut() {
                report(
                    "Classifying library",
                    &format!(
                        "{idx}/{total_files} files; {} games, {} archives, {} archive entries",
                        discoveries.len(),
                        containers.len(),
                        entries.len()
                    ),
                );
            }
        }
        if let Some(format) = ArchiveFormat::from_ext(&f.ext) {
            if let Some(r) = root_stats.get_mut(&f.root) {
                r.archives += 1;
            }
            largest_archives.push((f.size, f.path.display().to_string()));
            let archive_phase_t = Instant::now();
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
            phase_stats
                .archive_toc
                .add(archive_phase_t.elapsed().as_micros() as u64);
            slowest_archives.push((scan.container.scan_us, f.path.display().to_string()));
            let stats = format_stats.entry(format).or_default();
            stats.containers += 1;
            stats.entries += scan.entries.len() as u64;
            stats.launchable_entries += scan.entries.iter().filter(|e| e.launchable).count() as u64;
            stats.scan_us += scan.container.scan_us;
            match &scan.container.scan_status {
                ArchiveScanStatus::Ok | ArchiveScanStatus::HeaderOnly => {}
                ArchiveScanStatus::Unsupported => stats.skipped += 1,
                ArchiveScanStatus::Error(_) => stats.errors += 1,
            }
            if let Some(r) = root_stats.get_mut(&f.root) {
                r.scan_us += scan.container.scan_us;
            }
            if format == ArchiveFormat::Chd {
                discoveries.push(discovery_from_file(&f, DiscoverySourceKind::Container));
            } else if is_launchable_container(&f, format) {
                discoveries.push(discovery_from_file(&f, DiscoverySourceKind::Container));
            }
            if cfg.optional_catalogs {
                let catalog_t = Instant::now();
                let catalog_discoveries = catalog_discoveries_from_container(&f, format);
                phase_stats
                    .optional_catalog
                    .add(catalog_t.elapsed().as_micros() as u64);
                phase_stats.optional_catalog_entries += catalog_discoveries.len() as u64;
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
            if let Some(r) = root_stats.get_mut(&f.root) {
                r.normal_launchables += 1;
            }
            normal_files.push(f.path.display().to_string());
            let file_t = Instant::now();
            let discovery = discovery_from_file(&f, source_kind_for_ext(&f.ext));
            let file_us = file_t.elapsed().as_micros() as u64;
            match f.ext.as_str() {
                "mra" => phase_stats.mra.add(file_us),
                "mgl" => phase_stats.mgl.add(file_us),
                _ => phase_stats.payload.add(file_us),
            }
            discoveries.push(discovery);
        }
    }
    if cfg.optional_catalogs {
        if let Some(report) = progress.as_mut() {
            report("Importing metadata", "Looking for gamelist.xml screenshots...");
        }
        let catalog_t = Instant::now();
        let imported = match progress.as_mut() {
            Some(report) => enrich_discoveries_from_gamelists(
                &mut discoveries,
                &cfg.roots,
                Some(&mut **report),
            ),
            None => enrich_discoveries_from_gamelists(&mut discoveries, &cfg.roots, None),
        };
        phase_stats
            .optional_catalog
            .add(catalog_t.elapsed().as_micros() as u64);
        phase_stats.optional_catalog_entries += imported as u64;
        if let Some(report) = progress.as_mut() {
            report(
                "Importing metadata",
                &format!("Matched screenshot metadata for {imported} games"),
            );
        }
    }

    LibraryScan {
        version: 4,
        scanned_at_unix: unix_now_secs(),
        file_fingerprints,
        normal_files,
        containers,
        entries,
        discoveries,
        root_stats,
        format_stats,
        phase_stats,
        discover_us,
        classify_us: classify_t.elapsed().as_micros() as u64,
        largest_archives,
        slowest_archives,
    }
}

fn file_fingerprint_from_files(files: &[FoundFile]) -> FileFingerprint {
    files
        .iter()
        .map(|f| (f.path.display().to_string(), (f.size, f.mtime_secs)))
        .collect()
}

fn discover_files(
    roots: &[String],
    mut progress: Option<&mut dyn FnMut(&str, &str)>,
) -> (Vec<FoundFile>, HashMap<String, RootStats>) {
    let mut out = Vec::new();
    let mut stats = HashMap::new();
    for root in roots {
        if let Some(report) = progress.as_mut() {
            report("Scanning roots", &format!("Walking {root}"));
        }
        let t = Instant::now();
        let mut rs = RootStats::default();
        let path = Path::new(root);
        if path.is_dir() {
            for entry in walkdir::WalkDir::new(path)
                .follow_links(true)
                .into_iter()
                .filter_map(Result::ok)
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let p = entry.path();
                if should_ignore_path(p) {
                    continue;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                let ext = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                rs.files += 1;
                out.push(FoundFile {
                    path: p.to_path_buf(),
                    root: root.clone(),
                    ext,
                    size: meta.len(),
                    mtime_secs: mtime_secs(&meta),
                });
                if rs.files % 500 == 0 {
                    if let Some(report) = progress.as_mut() {
                        report(
                            "Scanning roots",
                            &format!("{root}: {} files found", rs.files),
                        );
                    }
                }
            }
        }
        rs.scan_us = t.elapsed().as_micros() as u64;
        if let Some(report) = progress.as_mut() {
            report(
                "Scanning roots",
                &format!("{root}: {} files found", rs.files),
            );
        }
        stats.insert(root.clone(), rs);
    }
    (out, stats)
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
    if !path.contains("/games/Amiga/AmigaVision") {
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
    mut progress: Option<&mut dyn FnMut(&str, &str)>,
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

fn match_stem(path: &str) -> String {
    normalize_id(
        Path::new(path.split("::").next().unwrap_or(path))
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(path),
    )
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
                launch_ref: mgl
                    .file_path
                    .clone()
                    .unwrap_or_else(|| file.path.display().to_string()),
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

fn benchmark_memory_queries(scan: &LibraryScan) {
    let query_t = Instant::now();
    let first_page = scan.normal_files.iter().take(40).count()
        + scan
            .entries
            .iter()
            .filter(|e| e.launchable)
            .take(40)
            .count();
    println!(
        "library_bench_memory_query_tsv\tquery_first_page\t{}\trows={first_page}",
        query_t.elapsed().as_micros()
    );

    let search_t = Instant::now();
    let needle = "mario";
    let normal_matches = scan
        .normal_files
        .iter()
        .filter(|p| p.to_ascii_lowercase().contains(needle))
        .count();
    let archive_matches = scan
        .entries
        .iter()
        .filter(|e| e.normalized_title.contains(needle))
        .count();
    println!(
        "library_bench_memory_query_tsv\ttext_search_scan\t{}\tneedle={needle}\tmatches={}",
        search_t.elapsed().as_micros(),
        normal_matches + archive_matches
    );

    let taxonomy_t = Instant::now();
    let unique: HashMap<String, &GameDiscovery> = scan
        .discoveries
        .iter()
        .map(|d| (discovery_unique_key(d), d))
        .collect();
    let cps2_matches = unique
        .values()
        .copied()
        .filter(|d| d.hardware_id == "capcom-cps2")
        .count();
    let saturn_matches = unique
        .values()
        .copied()
        .filter(|d| d.platform_id == "saturn")
        .count();
    let unknown_matches = unique
        .values()
        .copied()
        .filter(|d| d.platform_id == "unknown" || d.hardware_id == "unknown")
        .count();
    let raw_cps2_matches = scan
        .discoveries
        .iter()
        .filter(|d| d.hardware_id == "capcom-cps2")
        .count();
    println!(
        "library_bench_memory_query_tsv\ttaxonomy_query_scan\t{}\tcps2={cps2_matches}\traw_cps2={raw_cps2_matches}\tsaturn={saturn_matches}\tunknown={unknown_matches}",
        taxonomy_t.elapsed().as_micros()
    );
}

fn benchmark_sqlite_backend(path: &Path, scan: &LibraryScan) {
    benchmark_sqlite_cached_open(path);

    let import_t = Instant::now();
    match save_sqlite_scan(path, scan) {
        Ok(bytes) => println!(
            "library_bench_sqlite_tsv\tfull_import\t{}\tbytes={bytes}\tnormal_files={}\tcontainers={}\tentries={}\traw_discoveries={}\tstored_discoveries={}",
            import_t.elapsed().as_micros(),
            scan.normal_files.len(),
            scan.containers.len(),
            scan.entries.len(),
            scan.discoveries.len(),
            count_sqlite_table(path, "discoveries").unwrap_or(0)
        ),
        Err(e) => {
            println!(
                "library_bench_sqlite_tsv\tfull_import_error\t{}\t{}",
                import_t.elapsed().as_micros(),
                e
            );
            return;
        }
    }

    benchmark_sqlite_cached_open(path);
    benchmark_sqlite_queries(path);
    benchmark_sqlite_no_change(path, scan);
}

fn benchmark_sqlite_cached_open(path: &Path) {
    let t = Instant::now();
    match open_sqlite_counts(path) {
        Ok((bytes, normal_files, containers, entries, discoveries)) => println!(
            "library_bench_sqlite_tsv\tcached_open\t{}\tbytes={bytes}\tnormal_files={normal_files}\tcontainers={containers}\tentries={entries}\tdiscoveries={discoveries}",
            t.elapsed().as_micros()
        ),
        Err(e) => println!(
            "library_bench_sqlite_tsv\tcached_open_error\t{}\t{}",
            t.elapsed().as_micros(),
            e
        ),
    }
}

fn benchmark_sqlite_queries(path: &Path) {
    let Ok(conn) = Connection::open(path) else {
        println!("library_bench_sqlite_tsv\tquery_error\t0\topen failed");
        return;
    };
    let _ = conn.execute_batch("PRAGMA query_only=ON;");

    let first_page_t = Instant::now();
    let first_page = conn
        .query_row(
            "SELECT COUNT(*) FROM (SELECT key FROM discoveries LIMIT 80)",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n.max(0) as u64)
        .unwrap_or(0);
    println!(
        "library_bench_sqlite_tsv\tquery_first_page\t{}\trows={first_page}",
        first_page_t.elapsed().as_micros()
    );

    let search_t = Instant::now();
    let needle = "mario";
    let matches = conn
        .query_row(
            "SELECT COUNT(*) FROM discoveries_fts WHERE discoveries_fts MATCH ?1",
            [needle],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n.max(0) as u64)
        .unwrap_or(0);
    println!(
        "library_bench_sqlite_tsv\ttext_search_fts\t{}\tneedle={needle}\tmatches={matches}",
        search_t.elapsed().as_micros()
    );

    let get_t = Instant::now();
    let found = conn
        .query_row("SELECT key FROM discoveries LIMIT 1", [], |r| {
            r.get::<_, String>(0)
        })
        .ok()
        .and_then(|key| {
            conn.query_row(
                "SELECT 1 FROM discoveries WHERE key=?1",
                [key.as_str()],
                |r| r.get::<_, u8>(0),
            )
            .optional()
            .ok()
            .flatten()
        })
        .is_some();
    println!(
        "library_bench_sqlite_tsv\tpoint_lookup\t{}\tfound={found}",
        get_t.elapsed().as_micros()
    );
}

fn benchmark_sqlite_no_change(path: &Path, scan: &LibraryScan) {
    let t = Instant::now();
    let changed = read_sqlite_fingerprint(path)
        .map(|fingerprint| fingerprint != db_fingerprint(scan))
        .unwrap_or(true);
    println!(
        "library_bench_sqlite_tsv\tno_change_fingerprint\t{}\tchanged={changed}",
        t.elapsed().as_micros()
    );
}

fn benchmark_second_scan(cfg: &BenchConfig, initial_scan: &LibraryScan) {
    let scan = scan_library(cfg);
    let fingerprint_t = Instant::now();
    let changed = db_fingerprint(&scan) != db_fingerprint(initial_scan)
        || read_sqlite_fingerprint(&cfg.sqlite_path)
            .map(|fingerprint| fingerprint != db_fingerprint(&scan))
            .unwrap_or(true);
    println!(
        "library_bench_second_scan_tsv\troot_discovery\t{}\tfiles={}",
        scan.discover_us,
        scan.root_stats.values().map(|s| s.files).sum::<u64>()
    );
    println!(
        "library_bench_second_scan_tsv\tclassify_and_archive_toc\t{}\tnormal_files={}\tcontainers={}\tcontainer_entries={}",
        scan.classify_us,
        scan.normal_files.len(),
        scan.containers.len(),
        scan.entries.len()
    );
    println!(
        "library_bench_second_scan_tsv\tno_change_fingerprint\t{}\tchanged={changed}",
        fingerprint_t.elapsed().as_micros()
    );
    print_phase_stats("library_bench_second_phase_tsv", &scan.phase_stats);
}

fn db_fingerprint(scan: &LibraryScan) -> DbFingerprint {
    DbFingerprint {
        normal_files: scan.normal_files.len(),
        containers: scan.containers.len(),
        entries: scan.entries.len(),
        discoveries: unique_discovery_count(&scan.discoveries),
        file_fingerprints: scan.file_fingerprints.clone(),
        container_fingerprints: scan
            .containers
            .iter()
            .map(|c| (c.file_path.clone(), (c.size, c.mtime_secs)))
            .collect(),
    }
}

fn unique_discovery_count(discoveries: &[GameDiscovery]) -> usize {
    discoveries
        .iter()
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
    tx.commit().map_err(|e| format!("commit sqlite tx: {e}"))?;
    std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("stat sqlite: {e}"))
}

fn open_sqlite_counts(path: &Path) -> Result<(u64, u64, u64, u64, u64), String> {
    let bytes = std::fs::metadata(path)
        .map_err(|e| format!("stat sqlite: {e}"))?
        .len();
    let fingerprint = read_sqlite_fingerprint(path).ok_or("read sqlite fingerprint failed")?;
    Ok((
        bytes,
        fingerprint.normal_files as u64,
        fingerprint.containers as u64,
        fingerprint.entries as u64,
        fingerprint.discoveries as u64,
    ))
}

fn count_sqlite_table(path: &Path, table: &str) -> Option<u64> {
    let conn = Connection::open(path).ok()?;
    sqlite_count(&conn, table).ok()
}

fn sqlite_count(conn: &Connection, table: &str) -> Result<u64, String> {
    let sql = match table {
        "normal_files" => "SELECT COUNT(*) FROM normal_files",
        "containers" => "SELECT COUNT(*) FROM containers",
        "entries" => "SELECT COUNT(*) FROM entries",
        "discoveries" => "SELECT COUNT(*) FROM discoveries",
        _ => return Err(format!("unknown table: {table}")),
    };
    conn.query_row(sql, [], |r| r.get::<_, i64>(0))
        .map(|n| n.max(0) as u64)
        .map_err(|e| format!("count {table}: {e}"))
}

fn read_sqlite_fingerprint(path: &Path) -> Option<DbFingerprint> {
    let conn = Connection::open(path).ok()?;
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
    Some(DbFingerprint {
        normal_files: sqlite_meta_usize(&conn, "normal_files")?,
        containers: sqlite_meta_usize(&conn, "containers")?,
        entries: sqlite_meta_usize(&conn, "entries")?,
        discoveries: sqlite_meta_usize(&conn, "discoveries")?,
        file_fingerprints,
        container_fingerprints,
    })
}

fn sqlite_meta_usize(conn: &Connection, key: &str) -> Option<usize> {
    conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| {
        r.get::<_, i64>(0)
    })
    .ok()
    .map(|n| n.max(0) as usize)
}

fn print_root_stats(label: &str, stats: &HashMap<String, RootStats>) {
    let mut rows: Vec<_> = stats.iter().collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    for (root, s) in rows {
        println!(
            "{label}\t{root}\tfiles={}\tnormal_launchables={}\tarchives={}\tscan_us={}",
            s.files, s.normal_launchables, s.archives, s.scan_us
        );
    }
}

fn print_format_stats(label: &str, stats: &BTreeMap<ArchiveFormat, FormatStats>) {
    for (format, s) in stats {
        println!(
            "{label}\t{}\tcontainers={}\tentries={}\tlaunchable_entries={}\tscan_us={}\tskipped={}\terrors={}",
            format.as_str(),
            s.containers,
            s.entries,
            s.launchable_entries,
            s.scan_us,
            s.skipped,
            s.errors
        );
    }
}

fn print_phase_stats(label: &str, stats: &PhaseStats) {
    print_phase(label, "mra_parse", &stats.mra, "");
    print_phase(label, "mgl_parse", &stats.mgl, "");
    print_phase(label, "payload_classify", &stats.payload, "");
    print_phase(label, "archive_toc", &stats.archive_toc, "");
    print_phase(
        label,
        "optional_catalog_extract",
        &stats.optional_catalog,
        &format!("entries={}", stats.optional_catalog_entries),
    );
}

fn print_phase(label: &str, name: &str, count: &TimedCount, extra: &str) {
    let avg_us = if count.count == 0 {
        0
    } else {
        count.us / count.count
    };
    if extra.is_empty() {
        println!(
            "{label}\t{name}\tcount={}\tscan_us={}\tavg_us={avg_us}",
            count.count, count.us
        );
    } else {
        println!(
            "{label}\t{name}\tcount={}\tscan_us={}\tavg_us={avg_us}\t{extra}",
            count.count, count.us
        );
    }
}

fn print_taxonomy_stats(discoveries: &[GameDiscovery]) {
    let mut seen = HashSet::<String>::new();
    let mut rows = BTreeMap::<(String, String, String, String), u64>::new();
    for d in discoveries {
        if !seen.insert(discovery_unique_key(d)) {
            continue;
        }
        *rows
            .entry((
                d.category.clone(),
                d.platform_id.clone(),
                d.core_id.clone(),
                d.hardware_id.clone(),
            ))
            .or_default() += 1;
    }
    let mut sorted: Vec<_> = rows.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    println!(
        "library_bench_tsv\ttaxonomy_discovery\t0\traw_discoveries={}\tunique_discoveries={}\tgroups={}",
        discoveries.len(),
        seen.len(),
        sorted.len()
    );
    for ((category, platform, core, hardware), count) in sorted.into_iter().take(80) {
        println!(
            "library_bench_taxonomy_tsv\t{category}\tplatform={platform}\tcore={core}\thardware={hardware}\tgames={count}"
        );
    }
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

fn print_unknown_stats(discoveries: &[GameDiscovery]) {
    let mut seen = HashSet::<String>::new();
    let mut buckets = BTreeMap::<String, (u64, String)>::new();
    for d in discoveries {
        if !seen.insert(discovery_unique_key(d)) {
            continue;
        }
        if d.platform_id != "unknown" && d.hardware_id != "unknown" {
            continue;
        }
        let bucket = unknown_bucket(d);
        let entry = buckets
            .entry(bucket)
            .or_insert_with(|| (0, d.source_path.clone()));
        entry.0 += 1;
    }
    let mut rows: Vec<_> = buckets.into_iter().collect();
    rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then_with(|| a.0.cmp(&b.0)));
    for (bucket, (count, sample)) in rows.into_iter().take(30) {
        println!("library_bench_unknown_tsv\t{bucket}\tcount={count}\tsample={sample}");
    }
}

fn unknown_bucket(d: &GameDiscovery) -> String {
    let ext = path_ext(&d.source_path).unwrap_or_else(|| "none".to_string());
    let lower_path = d.source_path.to_ascii_lowercase();
    let folder = games_folder(&lower_path)
        .or_else(|| launcher_folder(&lower_path))
        .unwrap_or("none");
    format!(
        "source={}\tfolder={folder}\text={ext}",
        source_kind_str(d.source_kind)
    )
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

fn print_top(label: &str, rows: &mut Vec<(u64, String)>, n: usize) {
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    for (value, path) in rows.iter().take(n) {
        println!("library_bench_top_tsv\t{label}\t{value}\t{path}");
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

fn launcher_folder(lower_path: &str) -> Option<&str> {
    let rest = lower_path.split("/_games/").nth(1)?;
    rest.split('/').next()
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
}
