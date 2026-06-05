//! Whole-MiSTer library benchmark: normal files + archive containers.
//!
//! This is deliberately TOC/header-only for archives. Indexing must never
//! decompress full game libraries just to make the launcher searchable.

use serde::{Deserialize, Serialize};
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

const DEFAULT_INDEX_PATH: &str = "/media/fat/mister-magic/library-bench-index.json";

const NORMAL_LAUNCH_EXTS: &[&str] = &[
    "mra", "mgl", "rbf", "rom", "bin", "cue", "iso", "img", "dsk", "vhd", "hdf", "adf", "ipf",
    "st", "msa", "tap", "tzx", "z80", "sna", "nes", "fds", "smc", "sfc", "gb", "gbc", "gba", "gg",
    "sms", "md", "gen", "32x", "pce", "vec", "n64", "z64", "v64", "neo", "chd",
];

const MRA_PREFIX_BYTES: usize = 160 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryContainer {
    pub file_path: String,
    pub format: ArchiveFormat,
    pub size: u64,
    pub mtime_secs: i64,
    pub entry_count: u32,
    pub scan_status: ArchiveScanStatus,
    pub scan_us: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ArchiveScanStatus {
    Ok,
    HeaderOnly,
    Unsupported,
    Error(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LibraryBenchIndex {
    version: u32,
    scanned_at_unix: i64,
    normal_files: Vec<String>,
    containers: Vec<LibraryContainer>,
    entries: Vec<LibraryContainerEntry>,
    discoveries: Vec<GameDiscovery>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    confidence: DiscoveryConfidence,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum DiscoverySourceKind {
    Mra,
    Mgl,
    PayloadFile,
    ArchiveEntry,
    Container,
    CatalogEntry,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum DiscoveryConfidence {
    MraHardware,
    MraCore,
    PayloadPath,
    Extension,
    ArchiveToc,
    CatalogMetadata,
}

#[derive(Default)]
struct RootStats {
    files: u64,
    normal_launchables: u64,
    archives: u64,
    scan_us: u64,
}

#[derive(Default)]
struct FormatStats {
    containers: u64,
    entries: u64,
    launchable_entries: u64,
    scan_us: u64,
    skipped: u64,
    errors: u64,
}

#[derive(Default)]
struct PhaseStats {
    mra: TimedCount,
    mgl: TimedCount,
    payload: TimedCount,
    archive_toc: TimedCount,
    optional_catalog: TimedCount,
    optional_catalog_entries: u64,
}

#[derive(Default)]
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
    println!("library-bench index_path={}", cfg.index_path.display());
    println!("library-bench archive_toc=header-only no-decompress");
    println!(
        "library-bench optional_catalogs={}",
        if cfg.optional_catalogs { "on" } else { "off" }
    );

    let open_t = Instant::now();
    let previous = load_index(&cfg.index_path);
    println!(
        "library_bench_tsv\tcached_open\t{}\tbytes={}\tcontainers={}\tentries={}",
        open_t.elapsed().as_micros(),
        previous.as_ref().map(|p| p.0).unwrap_or(0),
        previous.as_ref().map(|p| p.1.containers.len()).unwrap_or(0),
        previous.as_ref().map(|p| p.1.entries.len()).unwrap_or(0)
    );
    let previous = previous.map(|(_, idx)| idx);

    let discover_t = Instant::now();
    let (files, mut root_stats) = discover_files(&cfg.roots);
    let discover_us = discover_t.elapsed().as_micros() as u64;
    println!(
        "library_bench_tsv\troot_discovery\t{discover_us}\tfiles={}",
        files.len()
    );

    let mut normal_files = Vec::new();
    let mut containers = Vec::new();
    let mut entries = Vec::new();
    let mut discoveries = Vec::new();
    let mut format_stats = BTreeMap::<ArchiveFormat, FormatStats>::new();
    let mut phase_stats = PhaseStats::default();
    let mut largest = Vec::<(u64, String)>::new();
    let mut slowest = Vec::<(u64, String)>::new();

    let classify_t = Instant::now();
    for f in files {
        if let Some(format) = ArchiveFormat::from_ext(&f.ext) {
            if let Some(r) = root_stats.get_mut(&f.root) {
                r.archives += 1;
            }
            largest.push((f.size, f.path.display().to_string()));
            let archive_phase_t = Instant::now();
            let scan = scan_archive_toc(&f, format);
            phase_stats
                .archive_toc
                .add(archive_phase_t.elapsed().as_micros() as u64);
            slowest.push((scan.container.scan_us, f.path.display().to_string()));
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
    let classify_us = classify_t.elapsed().as_micros() as u64;
    println!(
        "library_bench_tsv\tclassify_and_archive_toc\t{classify_us}\tnormal_files={}\tcontainers={}\tcontainer_entries={}",
        normal_files.len(),
        containers.len(),
        entries.len()
    );

    print_root_stats(&root_stats);
    print_format_stats(&format_stats);
    print_phase_stats(&phase_stats);
    print_taxonomy_stats(&discoveries);
    print_unknown_stats(&discoveries);
    print_top("largest_archive", &mut largest, 10);
    print_top("slowest_archive_toc", &mut slowest, 10);

    let index = LibraryBenchIndex {
        version: 2,
        scanned_at_unix: unix_now_secs(),
        normal_files,
        containers,
        entries,
        discoveries,
    };

    benchmark_queries(&index);
    benchmark_no_change(previous.as_ref(), &index);

    let save_t = Instant::now();
    match save_index(&cfg.index_path, &index) {
        Ok(bytes) => println!(
            "library_bench_tsv\tjson_index_save\t{}\tbytes={bytes}",
            save_t.elapsed().as_micros()
        ),
        Err(e) => println!(
            "library_bench_tsv\tjson_index_save_error\t{}\t{}",
            save_t.elapsed().as_micros(),
            e
        ),
    }

    println!(
        "library_bench_tsv\ttotal\t{}\tbackend=json-index baseline; redb/sqlite candidates not linked yet",
        bench_start.elapsed().as_micros()
    );
}

struct BenchConfig {
    roots: Vec<String>,
    index_path: PathBuf,
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
        let index_path = std::env::var("MISTER_LIBRARY_BENCH_INDEX")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_INDEX_PATH));
        let optional_catalogs = env_bool("MISTER_LIBRARY_OPTIONAL_CATALOGS");
        Self {
            roots,
            index_path,
            optional_catalogs,
        }
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

fn discover_files(roots: &[String]) -> (Vec<FoundFile>, HashMap<String, RootStats>) {
    let mut out = Vec::new();
    let mut stats = HashMap::new();
    for root in roots {
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
            }
        }
        rs.scan_us = t.elapsed().as_micros() as u64;
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
            confidence: DiscoveryConfidence::CatalogMetadata,
        })
        .collect()
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

fn benchmark_queries(index: &LibraryBenchIndex) {
    let query_t = Instant::now();
    let first_page = index.normal_files.iter().take(40).count()
        + index
            .entries
            .iter()
            .filter(|e| e.launchable)
            .take(40)
            .count();
    println!(
        "library_bench_tsv\tquery_first_page\t{}\trows={first_page}",
        query_t.elapsed().as_micros()
    );

    let search_t = Instant::now();
    let needle = "mario";
    let normal_matches = index
        .normal_files
        .iter()
        .filter(|p| p.to_ascii_lowercase().contains(needle))
        .count();
    let archive_matches = index
        .entries
        .iter()
        .filter(|e| e.normalized_title.contains(needle))
        .count();
    println!(
        "library_bench_tsv\ttext_search_scan\t{}\tneedle={needle}\tmatches={}",
        search_t.elapsed().as_micros(),
        normal_matches + archive_matches
    );

    let taxonomy_t = Instant::now();
    let unique: HashMap<String, &GameDiscovery> = index
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
    let raw_cps2_matches = index
        .discoveries
        .iter()
        .filter(|d| d.hardware_id == "capcom-cps2")
        .count();
    println!(
        "library_bench_tsv\ttaxonomy_query_scan\t{}\tcps2={cps2_matches}\traw_cps2={raw_cps2_matches}\tsaturn={saturn_matches}\tunknown={unknown_matches}",
        taxonomy_t.elapsed().as_micros()
    );
}

fn benchmark_no_change(previous: Option<&LibraryBenchIndex>, current: &LibraryBenchIndex) {
    let t = Instant::now();
    let changed = previous
        .map(|p| {
            p.normal_files.len() != current.normal_files.len()
                || p.containers.len() != current.containers.len()
                || p.discoveries.len() != current.discoveries.len()
                || fingerprint_map(&p.containers) != fingerprint_map(&current.containers)
        })
        .unwrap_or(true);
    println!(
        "library_bench_tsv\tno_change_fingerprint\t{}\tchanged={changed}",
        t.elapsed().as_micros()
    );
}

fn fingerprint_map(containers: &[LibraryContainer]) -> HashMap<&str, (u64, i64)> {
    containers
        .iter()
        .map(|c| (c.file_path.as_str(), (c.size, c.mtime_secs)))
        .collect()
}

fn load_index(path: &Path) -> Option<(u64, LibraryBenchIndex)> {
    let data = std::fs::read(path).ok()?;
    let bytes = data.len() as u64;
    let idx = serde_json::from_slice(&data).ok()?;
    Some((bytes, idx))
}

fn save_index(path: &Path, index: &LibraryBenchIndex) -> Result<u64, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create index dir: {e}"))?;
    }
    let data = serde_json::to_vec(index).map_err(|e| format!("encode index: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &data).map_err(|e| format!("write index tmp: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename index: {e}"))?;
    Ok(data.len() as u64)
}

fn print_root_stats(stats: &HashMap<String, RootStats>) {
    let mut rows: Vec<_> = stats.iter().collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    for (root, s) in rows {
        println!(
            "library_bench_root_tsv\t{root}\tfiles={}\tnormal_launchables={}\tarchives={}\tscan_us={}",
            s.files, s.normal_launchables, s.archives, s.scan_us
        );
    }
}

fn print_format_stats(stats: &BTreeMap<ArchiveFormat, FormatStats>) {
    for (format, s) in stats {
        println!(
            "library_bench_archive_tsv\t{}\tcontainers={}\tentries={}\tlaunchable_entries={}\tscan_us={}\tskipped={}\terrors={}",
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

fn print_phase_stats(stats: &PhaseStats) {
    print_phase("mra_parse", &stats.mra, "");
    print_phase("mgl_parse", &stats.mgl, "");
    print_phase("payload_classify", &stats.payload, "");
    print_phase("archive_toc", &stats.archive_toc, "");
    print_phase(
        "optional_catalog_extract",
        &stats.optional_catalog,
        &format!("entries={}", stats.optional_catalog_entries),
    );
}

fn print_phase(name: &str, count: &TimedCount, extra: &str) {
    let avg_us = if count.count == 0 {
        0
    } else {
        count.us / count.count
    };
    if extra.is_empty() {
        println!(
            "library_bench_phase_tsv\t{name}\tcount={}\tscan_us={}\tavg_us={avg_us}",
            count.count, count.us
        );
    } else {
        println!(
            "library_bench_phase_tsv\t{name}\tcount={}\tscan_us={}\tavg_us={avg_us}\t{extra}",
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
