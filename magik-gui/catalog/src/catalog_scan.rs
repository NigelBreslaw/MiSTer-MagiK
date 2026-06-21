//! Library walking and archive candidate discovery.

use crate::launch_profiles::{self, LaunchProfile, ProfilePathClass};
use crate::library_db::{
    self, ArchiveFormat, ArchiveScanStatus, LibraryContainer, LibraryContainerEntry,
};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

pub(crate) struct FoundFile {
    pub(crate) path: PathBuf,
    pub(crate) ext: String,
    pub(crate) size: u64,
    pub(crate) mtime_secs: i64,
}

pub(crate) struct ArchiveScan {
    pub(crate) container: LibraryContainer,
    pub(crate) entries: Vec<LibraryContainerEntry>,
}

pub(crate) fn precount_discovery_candidates(roots: &[String]) -> (usize, usize, u64) {
    let started = Instant::now();
    let rx = discover_files_pipelined(roots.to_vec());
    let mut candidates = 0usize;
    let mut dirs = 0usize;
    while let Ok(event) = rx.recv() {
        match event {
            DiscoveryEvent::File(_) => candidates += 1,
            DiscoveryEvent::Done { dirs: count, .. } => {
                dirs = count;
                break;
            }
        }
    }
    (candidates, dirs, started.elapsed().as_micros() as u64)
}

pub(crate) fn classify_profile_path<'a>(
    profiles: &'a [LaunchProfile],
    path: &Path,
) -> Option<(&'a LaunchProfile, ProfilePathClass)> {
    let profile = profile_for_path(profiles, path)?;
    Some((profile, profile.classify_path(path)))
}

pub(crate) fn profile_for_path<'a>(profiles: &'a [LaunchProfile], path: &Path) -> Option<&'a LaunchProfile> {
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

pub(crate) enum DiscoveryEvent {
    File(FoundFile),
    Done { dirs: usize, discover_us: u64 },
}

pub(crate) fn discover_files_pipelined(roots: Vec<String>) -> mpsc::Receiver<DiscoveryEvent> {
    let (tx, rx) = mpsc::sync_channel(256);
    std::thread::Builder::new()
        .name("library-walker".to_string())
        .spawn(move || {
            let t = Instant::now();
            let dirs = discover_files_streaming(&roots, &tx);
            let _ = tx.send(DiscoveryEvent::Done {
                dirs,
                discover_us: t.elapsed().as_micros() as u64,
            });
        })
        .expect("spawn library-walker");
    rx
}

fn discover_files_streaming(roots: &[String], tx: &mpsc::SyncSender<DiscoveryEvent>) -> usize {
    walk_index_candidates(roots, Some(tx))
}

fn walk_index_candidates(roots: &[String], tx: Option<&mpsc::SyncSender<DiscoveryEvent>>) -> usize {
    let profiles = launch_profiles::builtin_profiles();
    let candidate_exts = source_index_extensions(&profiles);
    let targets = scan_targets_for_roots(roots, &profiles);
    library_db::report_library_scan_timing(
        "walk_targets",
        0,
        format!(
            "roots={} targets={} extensions={}",
            roots.len(),
            targets.len(),
            candidate_exts.len()
        ),
    );
    let mut dirs = 0usize;
    for target in targets {
        let target_t = Instant::now();
        let mut target_dirs = 1usize;
        let mut target_files = 0usize;
        let mut target_candidates = 0usize;
        dirs += 1;
        for entry in walkdir::WalkDir::new(&target)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !should_ignore_path(e.path()))
            .filter_map(Result::ok)
        {
            let p = entry.path();
            if p == target {
                continue;
            }
            if should_ignore_path(p) {
                continue;
            }
            if entry.file_type().is_dir() {
                dirs += 1;
                target_dirs += 1;
                continue;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            target_files += 1;
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !is_source_index_extension(&candidate_exts, p, &ext) {
                continue;
            }
            if !is_index_candidate(&profiles, p, &ext) {
                continue;
            }
            let (size, mtime_secs) = candidate_signature_for_walk_entry(p, &ext, &entry);
            let file = FoundFile {
                path: p.to_path_buf(),
                ext,
                size,
                mtime_secs,
            };
            target_candidates += 1;
            if let Some(tx) = tx {
                if tx.send(DiscoveryEvent::File(file)).is_err() {
                    return dirs;
                }
            }
        }
        library_db::report_library_scan_timing(
            "walk_target",
            target_t.elapsed().as_micros() as u64,
            format!(
                "path={} dirs={} files={} candidates={}",
                target.display(),
                target_dirs,
                target_files,
                target_candidates
            ),
        );
    }
    dirs
}

fn source_index_extensions(profiles: &[LaunchProfile]) -> HashSet<String> {
    let mut extensions = HashSet::new();
    for profile in profiles {
        for rule in &profile.payload_rules {
            insert_extensions(&mut extensions, rule.extensions);
        }
        for rule in &profile.archive_entry_rules {
            insert_extensions(&mut extensions, rule.extensions);
            extensions.insert("zip".to_string());
        }
        for rule in &profile.collection_rules {
            insert_extensions(&mut extensions, rule.archive_extensions);
        }
        for rule in &profile.ignore_rules {
            insert_extensions(&mut extensions, rule.extensions);
        }
    }
    extensions
}

fn insert_extensions(extensions: &mut HashSet<String>, values: &[&str]) {
    for value in values {
        let value = value.trim().to_ascii_lowercase();
        if !value.is_empty() {
            extensions.insert(value);
        }
    }
}

fn is_source_index_extension(candidate_exts: &HashSet<String>, path: &Path, ext: &str) -> bool {
    candidate_exts.contains(ext) || library_db::is_amigavision_listing_path(path)
}

fn scan_targets_for_roots(roots: &[String], profiles: &[LaunchProfile]) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    for root in roots {
        let path = Path::new(root);
        if !is_real_dir(path) {
            continue;
        }
        if is_direct_scan_root(path, profiles) {
            push_scan_target(&mut targets, path.to_path_buf());
            continue;
        }
        if path_name_eq(path, "games") {
            push_profile_game_dirs(&mut targets, path, profiles);
            continue;
        }

        for launcher_dir in ["_Arcade", "_Games", "_DOS Games", "_Console (autoboot)"] {
            push_scan_target(&mut targets, path.join(launcher_dir));
        }
        push_profile_game_dirs(&mut targets, &path.join("games"), profiles);
    }
    dedupe_existing_scan_targets(targets)
}

fn push_profile_game_dirs(
    targets: &mut Vec<PathBuf>,
    games_dir: &Path,
    profiles: &[LaunchProfile],
) {
    for profile in profiles {
        for dir in &profile.game_dirs {
            if dir.starts_with('_') {
                continue;
            }
            push_scan_target(targets, games_dir.join(dir));
        }
    }
}

fn push_scan_target(targets: &mut Vec<PathBuf>, path: PathBuf) {
    if is_real_dir(&path) {
        targets.push(path);
    }
}

fn dedupe_existing_scan_targets(targets: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for target in targets {
        let key = target.display().to_string().to_ascii_lowercase();
        if seen.insert(key) {
            out.push(target);
        }
    }
    out
}

fn is_direct_scan_root(path: &Path, profiles: &[LaunchProfile]) -> bool {
    ["_Arcade", "_Games", "_DOS Games", "_Console (autoboot)"]
        .iter()
        .any(|name| path_name_eq(path, name))
        || profiles.iter().any(|profile| {
            profile
                .game_dirs
                .iter()
                .any(|dir| !dir.starts_with('_') && path_name_eq(path, dir))
        })
}

fn path_name_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(expected))
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

fn candidate_signature_for_walk_entry(
    path: &Path,
    ext: &str,
    entry: &walkdir::DirEntry,
) -> (u64, i64) {
    if ext.eq_ignore_ascii_case("zip") {
        if let Ok(meta) = entry.metadata() {
            return (meta.len(), library_db::mtime_secs(&meta));
        }
    }
    let _ = path;
    (0, 0)
}

pub(crate) fn scan_archive_toc(
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

pub(crate) fn scan_container_header(file: &FoundFile, format: ArchiveFormat) -> LibraryContainer {
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

pub(crate) fn scan_zip_central_directory(
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
    let Some(eocd) = library_db::find_eocd(&tail) else {
        return Err("zip EOCD not found".to_string());
    };

    let mut cd_entries = library_db::le_u16(&tail[eocd + 10..eocd + 12]) as usize;
    let mut cd_size = library_db::le_u32(&tail[eocd + 12..eocd + 16]) as u64;
    let mut cd_offset = library_db::le_u32(&tail[eocd + 16..eocd + 20]) as u64;
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
        if library_db::le_u32(&header[0..4]) != 0x0201_4b50 {
            return Err(format!("bad central directory signature at {entry_offset}"));
        }
        scanned += 1;
        let crc32 = library_db::le_u32(&header[16..20]);
        let compressed = library_db::le_u32(&header[20..24]) as u64;
        let uncompressed = library_db::le_u32(&header[24..28]) as u64;
        let name_len = library_db::le_u16(&header[28..30]) as u64;
        let extra_len = library_db::le_u16(&header[30..32]) as u64;
        let comment_len = library_db::le_u16(&header[32..34]) as u64;
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
                    normalized_title: library_db::normalize_title(&name),
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
    let zip64_eocd_offset = library_db::le_u64(&tail[locator + 8..locator + 16]);
    f.seek(SeekFrom::Start(zip64_eocd_offset))
        .map_err(|e| format!("seek zip64 EOCD: {e}"))?;
    let mut record = [0u8; 56];
    f.read_exact(&mut record)
        .map_err(|e| format!("read zip64 EOCD: {e}"))?;
    if library_db::le_u32(&record[0..4]) != 0x0606_4b50 {
        return Err("zip64 EOCD signature not found".to_string());
    }
    let entries = usize::try_from(library_db::le_u64(&record[32..40]))
        .map_err(|_| "zip64 entry count too large to index".to_string())?;
    Ok(ZipCentralDirectoryLocation {
        entries,
        size: library_db::le_u64(&record[40..48]),
        offset: library_db::le_u64(&record[48..56]),
    })
}

pub(crate) fn is_index_candidate(profiles: &[LaunchProfile], path: &Path, _ext: &str) -> bool {
    matches!(
        crate::catalog_scan::classify_profile_path(profiles, path),
        Some((
            _,
            ProfilePathClass::Payload { .. }
                | ProfilePathClass::Collection { .. }
                | ProfilePathClass::Ignored { .. }
        ))
    ) || library_db::is_amigavision_listing_path(path)
}

pub(crate) fn should_ignore_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy().to_ascii_lowercase();
    if path_str.contains("/.____padding_file/") || path_str.contains("/__macosx/") {
        return true;
    }
    if is_arcade_non_game_tree(path) {
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

fn is_arcade_non_game_tree(path: &Path) -> bool {
    let mut previous_was_arcade = false;
    for component in path.components().filter_map(|c| c.as_os_str().to_str()) {
        if previous_was_arcade
            && (component.eq_ignore_ascii_case("media") || component.eq_ignore_ascii_case("cores"))
        {
            return true;
        }
        previous_was_arcade = component.eq_ignore_ascii_case("_Arcade");
    }
    false
}
