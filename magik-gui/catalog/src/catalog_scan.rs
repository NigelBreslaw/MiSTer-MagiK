//! Library walking and archive candidate discovery.

use crate::launch_profiles::{self, LaunchProfile, ProfilePathClass};
use crate::library_db::{
    self, ArchiveFormat, ArchiveScanStatus, LibraryContainer, LibraryContainerEntry,
};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

const DISCOVERY_EVENT_BUFFER: usize = 8192;
const ZIP_CENTRAL_DIRECTORY_BUFFER_BYTES: usize = 64 * 1024;
const ZIP_CENTRAL_DIRECTORY_MAX_BUFFER_BYTES: u64 = 8 * 1024 * 1024;
const ZIP_SKIP_BUFFER_BYTES: usize = 4 * 1024;

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

pub(crate) fn profile_for_path<'a>(
    profiles: &'a [LaunchProfile],
    path: &Path,
) -> Option<&'a LaunchProfile> {
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

struct WalkTargetStats {
    dirs: usize,
    files: usize,
    candidates: usize,
    elapsed_us: u64,
    aborted: bool,
}

struct WalkTargetBatch {
    target: PathBuf,
    stats: WalkTargetStats,
    files: Vec<FoundFile>,
}

pub(crate) fn discover_files_pipelined(roots: Vec<String>) -> mpsc::Receiver<DiscoveryEvent> {
    let (tx, rx) = mpsc::sync_channel(DISCOVERY_EVENT_BUFFER);
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
    if let Some(tx) = tx {
        return walk_index_candidates_streaming(targets, &profiles, &candidate_exts, tx);
    }
    for target in targets {
        let stats = scan_target_candidates(&target, &profiles, &candidate_exts, |_| true);
        dirs += stats.dirs;
        report_walk_target(&target, &stats);
    }
    dirs
}

fn walk_index_candidates_streaming(
    targets: Vec<PathBuf>,
    profiles: &[LaunchProfile],
    candidate_exts: &HashSet<String>,
    tx: &mpsc::SyncSender<DiscoveryEvent>,
) -> usize {
    if targets.is_empty() {
        return 0;
    }
    let mut dirs = 0usize;
    let (batch_tx, batch_rx) = mpsc::channel();
    let background_targets: Vec<(usize, PathBuf)> =
        targets.iter().cloned().enumerate().skip(1).collect();
    let background_count = background_targets.len();
    // Keep the first target streaming for early progress, while one extra walker
    // pre-scans later targets and replays them in deterministic target order.
    if !background_targets.is_empty() {
        let profiles = profiles.to_vec();
        let candidate_exts = candidate_exts.clone();
        std::thread::Builder::new()
            .name("library-walker-prefetch".to_string())
            .spawn(move || {
                for (idx, target) in background_targets {
                    let batch = collect_target_candidates(target, &profiles, &candidate_exts);
                    if batch_tx.send((idx, batch)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn library-walker-prefetch");
    }

    let first_target = &targets[0];
    let first_stats = scan_target_candidates(first_target, profiles, candidate_exts, |file| {
        tx.send(DiscoveryEvent::File(file)).is_ok()
    });
    dirs += first_stats.dirs;
    report_walk_target(first_target, &first_stats);
    if first_stats.aborted {
        return dirs;
    }

    let mut batches: Vec<Option<WalkTargetBatch>> = std::iter::repeat_with(|| None)
        .take(targets.len())
        .collect();
    for _ in 0..background_count {
        let Ok((idx, batch)) = batch_rx.recv() else {
            break;
        };
        batches[idx] = Some(batch);
    }

    for batch in batches.into_iter().skip(1).flatten() {
        dirs += batch.stats.dirs;
        report_walk_target(&batch.target, &batch.stats);
        for file in batch.files {
            if tx.send(DiscoveryEvent::File(file)).is_err() {
                return dirs;
            }
        }
    }
    dirs
}

fn collect_target_candidates(
    target: PathBuf,
    profiles: &[LaunchProfile],
    candidate_exts: &HashSet<String>,
) -> WalkTargetBatch {
    let mut files = Vec::new();
    let stats = scan_target_candidates(&target, profiles, candidate_exts, |file| {
        files.push(file);
        true
    });
    WalkTargetBatch {
        target,
        stats,
        files,
    }
}

fn scan_target_candidates(
    target: &Path,
    profiles: &[LaunchProfile],
    candidate_exts: &HashSet<String>,
    mut emit: impl FnMut(FoundFile) -> bool,
) -> WalkTargetStats {
    let target_t = Instant::now();
    let mut dirs = 1usize;
    let mut files = 0usize;
    let mut candidates = 0usize;
    let mut aborted = false;
    for entry in walkdir::WalkDir::new(target)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !should_ignore_path(e.path()))
        .filter_map(Result::ok)
    {
        let p = entry.path();
        if p == target {
            continue;
        }
        if entry.file_type().is_dir() {
            dirs += 1;
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        files += 1;
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !is_source_index_extension(candidate_exts, p, &ext) {
            continue;
        }
        if !is_index_candidate(profiles, p, &ext) {
            continue;
        }
        let (size, mtime_secs) = candidate_signature_for_walk_entry(p, &ext, &entry);
        let file = FoundFile {
            path: p.to_path_buf(),
            ext,
            size,
            mtime_secs,
        };
        candidates += 1;
        if !emit(file) {
            aborted = true;
            break;
        }
    }
    WalkTargetStats {
        dirs,
        files,
        candidates,
        elapsed_us: target_t.elapsed().as_micros() as u64,
        aborted,
    }
}

fn report_walk_target(target: &Path, stats: &WalkTargetStats) {
    library_db::report_library_scan_timing(
        "walk_target",
        stats.elapsed_us,
        format!(
            "path={} dirs={} files={} candidates={}",
            target.display(),
            stats.dirs,
            stats.files,
            stats.candidates
        ),
    );
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
    candidate_exts.contains(ext) || crate::media_metadata::is_amigavision_listing_path(path)
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
    match cd_offset.checked_add(cd_size) {
        Some(end) if end <= len => {}
        _ => return Err("zip central directory outside file".to_string()),
    }
    let file_path = file.path.display().to_string();
    f.seek(SeekFrom::Start(cd_offset))
        .map_err(|e| format!("seek zip central directory: {e}"))?;
    if cd_size <= ZIP_CENTRAL_DIRECTORY_MAX_BUFFER_BYTES {
        let mut central_directory = vec![0u8; cd_size as usize];
        f.read_exact(&mut central_directory)
            .map_err(|e| format!("read zip central directory: {e}"))?;
        return scan_zip_central_directory_entries(
            &mut central_directory.as_slice(),
            cd_size,
            cd_entries,
            &file_path,
            profile,
        );
    }
    let mut central_directory =
        BufReader::with_capacity(ZIP_CENTRAL_DIRECTORY_BUFFER_BYTES, f.take(cd_size));
    scan_zip_central_directory_entries(
        &mut central_directory,
        cd_size,
        cd_entries,
        &file_path,
        profile,
    )
}

fn scan_zip_central_directory_entries(
    mut central_directory: &mut impl Read,
    cd_size: u64,
    cd_entries: usize,
    file_path: &str,
    profile: &LaunchProfile,
) -> Result<Vec<LibraryContainerEntry>, String> {
    let mut entries = Vec::new();
    let mut remaining = cd_size;
    let mut scanned = 0usize;
    while remaining >= 46 && scanned < cd_entries {
        let entry_offset = cd_size - remaining;
        let mut header = [0u8; 46];
        central_directory
            .read_exact(&mut header)
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
        central_directory
            .read_exact(&mut name_buf)
            .map_err(|e| format!("read zip entry name: {e}"))?;
        remaining -= name_len;
        if trailing_len > 0 {
            discard_zip_bytes(&mut central_directory, trailing_len)
                .map_err(|e| format!("skip zip entry metadata: {e}"))?;
            remaining -= trailing_len;
        }
        let name = String::from_utf8_lossy(&name_buf).into_owned();
        if !name.ends_with('/') && !name.starts_with("__MACOSX/") {
            if let Some(rule) = profile.classify_archive_entry(Path::new(&name)) {
                entries.push(LibraryContainerEntry {
                    file_path: file_path.to_string(),
                    entry_path: name.clone(),
                    normalized_title: library_db::normalize_title(&name),
                    profile_id: profile.id.to_string(),
                    rule,
                    compressed_size: Some(compressed),
                    uncompressed_size: Some(uncompressed),
                    crc32: Some(crc32),
                    launchable: true,
                    launch_ref: format!("{file_path}/{name}"),
                });
            }
        }
    }
    Ok(entries)
}

fn discard_zip_bytes(reader: &mut impl Read, mut len: u64) -> Result<(), std::io::Error> {
    let mut scratch = [0u8; ZIP_SKIP_BUFFER_BYTES];
    while len > 0 {
        let read_len = len.min(scratch.len() as u64) as usize;
        reader.read_exact(&mut scratch[..read_len])?;
        len -= read_len as u64;
    }
    Ok(())
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
    ) || crate::media_metadata::is_amigavision_listing_path(path)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_discovery::DiscoverySourceKind;
    use crate::launch_profiles::{self, ProfilePathClass};
    use crate::library_db::{mtime_secs, scan_library, BenchConfig};
    use crate::sqlite_catalog::{load_arcade_catalog_from_sqlite_at, save_sqlite_scan};
    use crate::test_support::*;
    use std::path::Path;

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
    fn zip_central_directory_skips_large_metadata_padding() {
        let root = unique_temp_dir("zip-central-large-padding");
        std::fs::create_dir_all(&root).expect("create temp root");
        let zip_path = root.join("games.zip");
        let extra = vec![0x5a; ZIP_SKIP_BUFFER_BYTES + 17];
        let comment = vec![0xa5; ZIP_SKIP_BUFFER_BYTES + 31];
        write_stored_zip_with_central_metadata(
            &zip_path,
            &[
                (
                    "World A-Z/2020 Super Baseball (2020bb).neo",
                    b"neo".as_slice(),
                ),
                ("World A-Z/Neo Bomberman (neobombe).neo", b"neo".as_slice()),
            ],
            &extra,
            &comment,
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

        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[1].entry_path,
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
}
