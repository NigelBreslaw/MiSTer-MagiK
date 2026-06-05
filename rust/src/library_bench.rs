//! Whole-MiSTer library benchmark: normal files + archive containers.
//!
//! This is deliberately TOC/header-only for archives. Indexing must never
//! decompress full game libraries just to make the launcher searchable.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

const DEFAULT_ROOTS: &[&str] = &[
    "/media/fat/_Arcade",
    "/media/fat/_Games",
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
    "mra", "mgl", "rbf", "rom", "bin", "cue", "iso", "img", "dsk", "vhd", "hdf", "adf",
    "ipf", "st", "msa", "tap", "tzx", "z80", "sna", "nes", "fds", "smc", "sfc", "gb", "gbc",
    "gba", "gg", "sms", "md", "gen", "32x", "pce", "vec", "n64", "z64", "v64", "neo",
];

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
    let mut format_stats = BTreeMap::<ArchiveFormat, FormatStats>::new();
    let mut largest = Vec::<(u64, String)>::new();
    let mut slowest = Vec::<(u64, String)>::new();

    let classify_t = Instant::now();
    for f in files {
        if let Some(format) = ArchiveFormat::from_ext(&f.ext) {
            if let Some(r) = root_stats.get_mut(&f.root) {
                r.archives += 1;
            }
            largest.push((f.size, f.path.display().to_string()));
            let scan = scan_archive_toc(&f, format);
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
            containers.push(scan.container);
            entries.extend(scan.entries);
        } else if is_normal_launchable(&f.ext) {
            if let Some(r) = root_stats.get_mut(&f.root) {
                r.normal_launchables += 1;
            }
            normal_files.push(f.path.display().to_string());
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
    print_top("largest_archive", &mut largest, 10);
    print_top("slowest_archive_toc", &mut slowest, 10);

    let index = LibraryBenchIndex {
        version: 1,
        scanned_at_unix: unix_now_secs(),
        normal_files,
        containers,
        entries,
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
        Self { roots, index_path }
    }
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
                if is_dot_underscore(p) {
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
    let first_page = index
        .normal_files
        .iter()
        .take(40)
        .count()
        + index.entries.iter().filter(|e| e.launchable).take(40).count();
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
}

fn benchmark_no_change(previous: Option<&LibraryBenchIndex>, current: &LibraryBenchIndex) {
    let t = Instant::now();
    let changed = previous
        .map(|p| {
            p.normal_files.len() != current.normal_files.len()
                || p.containers.len() != current.containers.len()
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

fn is_dot_underscore(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with("._"))
        .unwrap_or(false)
}

fn normalize_title(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_ascii_lowercase()
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
