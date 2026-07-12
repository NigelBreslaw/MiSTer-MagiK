//! Shared filesystem facts for catalog profile planning and audit.

use crate::catalog_scan::should_ignore_path;
use crate::launch_profiles;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InstalledCore {
    pub(crate) core_id: String,
    pub(crate) path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GameDirFact {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) has_payload_files: bool,
    pub(crate) has_zip_files: bool,
    pub(crate) payload_extensions: BTreeSet<String>,
}

impl GameDirFact {
    pub(crate) fn has_payloadish_files(&self) -> bool {
        self.has_payload_files || self.has_zip_files
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GameDirHeader {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
}

pub(crate) fn installed_cores_for_roots(roots: &[String]) -> Vec<InstalledCore> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for search_root in core_search_roots(roots) {
        // MiSTer installs Console, Computer, and Arcade cores directly in
        // their canonical roots. LLAPI additionally owns one canonical
        // `cores/` child. Reading those directories by name avoids a recursive
        // WalkDir plus an exFAT metadata round-trip for every non-core entry.
        let mut directories = vec![search_root.clone()];
        if path_name_eq(&search_root, "_LLAPI") {
            directories.push(search_root.join("cores"));
        }
        for directory in directories {
            let Ok(entries) = std::fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if should_ignore_hidden_path(&path) || !path_ext_eq(&path, "rbf") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if stem.eq_ignore_ascii_case("menu") {
                    continue;
                }
                let core_id = launch_profiles::canonical_core_id(stem);
                let key = format!("{}\t{}", core_id.to_ascii_lowercase(), path.display());
                if seen.insert(key) {
                    out.push(InstalledCore {
                        core_id,
                        path: path.to_path_buf(),
                    });
                }
            }
        }
    }
    append_mgl_system_descriptors(roots, &mut out, &mut seen);
    out
}

fn append_mgl_system_descriptors(
    roots: &[String],
    out: &mut Vec<InstalledCore>,
    seen: &mut BTreeSet<String>,
) {
    let physical = out.clone();
    for search_root in core_search_roots(roots) {
        let Ok(entries) = std::fs::read_dir(search_root) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let descriptor_path = entry.path();
            if !path_ext_eq(&descriptor_path, "mgl") {
                continue;
            }
            let Some(metadata) = crate::media_metadata::read_mgl_metadata(&descriptor_path) else {
                continue;
            };
            if metadata.file_path.is_some() {
                continue;
            }
            let (Some(setname), Some(rbf)) = (metadata.setname, metadata.rbf) else {
                continue;
            };
            let rbf_name = Path::new(&rbf)
                .file_name()
                .and_then(|value| value.to_str())
                .map(launch_profiles::canonical_core_id);
            let Some(target) = rbf_name.and_then(|name| {
                physical
                    .iter()
                    .find(|core| compact_system_name(&core.core_id) == compact_system_name(&name))
            }) else {
                continue;
            };
            let core_id = launch_profiles::canonical_core_id(&setname);
            let key = format!("{}\t{}", core_id.to_ascii_lowercase(), target.path.display());
            if seen.insert(key) {
                out.push(InstalledCore {
                    core_id,
                    path: target.path.clone(),
                });
            }
        }
    }
}

pub(crate) fn compact_system_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

/// Reads only bounded ZIP central-directory metadata. This is used solely
/// after a folder has strong name/descriptor evidence, never to choose a core.
pub(crate) fn archive_member_extensions_for_dir(path: &Path) -> BTreeSet<String> {
    let mut extensions = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(path) else {
        return extensions;
    };
    for entry in entries.filter_map(Result::ok).take(4096) {
        let archive = entry.path();
        if archive.is_file() && path_ext_eq(&archive, "zip") {
            let stem = archive
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if stem.contains("sdcard") || stem.contains("alt_roms") || stem.contains("empty_hdd") {
                continue;
            }
            append_zip_member_extensions(&archive, &mut extensions);
        }
    }
    extensions
}

fn append_zip_member_extensions(path: &Path, extensions: &mut BTreeSet<String>) {
    let Ok(mut file) = File::open(path) else { return };
    let Ok(len) = file.metadata().map(|metadata| metadata.len()) else { return };
    if len < 22 { return; }
    let tail_len = len.min(66_000) as usize;
    if file.seek(SeekFrom::End(-(tail_len as i64))).is_err() { return; }
    let mut tail = vec![0; tail_len];
    if file.read_exact(&mut tail).is_err() { return; }
    let Some(eocd) = crate::library_db::find_eocd(&tail) else { return };
    let entries = usize::from(crate::library_db::le_u16(&tail[eocd + 10..eocd + 12])).min(4096);
    let size = u64::from(crate::library_db::le_u32(&tail[eocd + 12..eocd + 16]));
    let offset = u64::from(crate::library_db::le_u32(&tail[eocd + 16..eocd + 20]));
    if offset.checked_add(size).is_none_or(|end| end > len)
        || file.seek(SeekFrom::Start(offset)).is_err()
    { return; }
    for _ in 0..entries {
        let mut header = [0; 46];
        if file.read_exact(&mut header).is_err()
            || crate::library_db::le_u32(&header[0..4]) != 0x0201_4b50
        { return; }
        let name_len = usize::from(crate::library_db::le_u16(&header[28..30]));
        let extra_len = usize::from(crate::library_db::le_u16(&header[30..32]));
        let comment_len = usize::from(crate::library_db::le_u16(&header[32..34]));
        if name_len > 4096 { return; }
        let mut name = vec![0; name_len];
        if file.read_exact(&mut name).is_err() { return; }
        if file.seek(SeekFrom::Current((extra_len + comment_len) as i64)).is_err() { return; }
        let name = String::from_utf8_lossy(&name);
        let member = Path::new(name.as_ref());
        if !name.ends_with('/') && !should_ignore_hidden_path(member) {
            if let Some(ext) = member.extension().and_then(|value| value.to_str()) {
                extensions.insert(ext.to_ascii_lowercase());
            }
        }
    }
}

pub(crate) fn top_level_game_dirs_for_roots(roots: &[String]) -> Vec<GameDirFact> {
    top_level_game_dirs_for_roots_excluding(roots, &BTreeSet::new())
}

pub(crate) fn top_level_game_dirs_for_roots_excluding(
    roots: &[String],
    excluded_names: &BTreeSet<String>,
) -> Vec<GameDirFact> {
    top_level_game_dir_headers_for_roots_excluding(roots, excluded_names)
        .into_iter()
        .map(|header| {
            let (has_payload_files, has_zip_files, payload_extensions) =
                game_dir_payload_facts(&header.path);
            GameDirFact {
                name: header.name,
                path: header.path,
                has_payload_files,
                has_zip_files,
                payload_extensions,
            }
        })
        .collect()
}

pub(crate) fn top_level_game_dir_headers_for_roots_excluding(
    roots: &[String],
    excluded_names: &BTreeSet<String>,
) -> Vec<GameDirHeader> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for game_root in game_roots(roots) {
        let Ok(read_dir) = std::fs::read_dir(&game_root) else {
            continue;
        };
        let mut entries = Vec::new();
        for entry in read_dir.filter_map(Result::ok) {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if should_ignore_game_dir(name) {
                continue;
            }
            if entry
                .file_type()
                .ok()
                .is_some_and(|file_type| file_type.is_symlink())
            {
                continue;
            }
            // `/games` is a directory namespace: every visible top-level
            // entry names a system directory. Treat that layout as the source
            // contract instead of issuing one synchronous exFAT metadata call
            // per system (and per hidden AppleDouble sidecar) merely to prove
            // the entry type. A non-directory header simply yields no facts
            // when its bounded target scan runs.
            if excluded_names.contains(&name.to_ascii_lowercase()) {
                continue;
            }
            let key = path.display().to_string().to_ascii_lowercase();
            if seen.insert(key) {
                entries.push(GameDirHeader {
                    name: name.to_string(),
                    path,
                });
            }
        }
        entries.sort_by_key(|entry| entry.name.to_ascii_lowercase());
        out.extend(entries);
    }
    out
}

#[cfg(test)]
pub(crate) fn game_dir_has_payload_candidate(path: &Path, extensions: &[String]) -> bool {
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .max_depth(2)
        .into_iter()
        .filter_entry(|entry| !should_ignore_path(entry.path()))
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path_ext_eq(path, "zip")
            || path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| contains_ignore_ascii_case(extensions, ext))
        {
            return true;
        }
    }
    false
}

pub(crate) fn game_dir_payload_facts_for_header(header: GameDirHeader) -> GameDirFact {
    let (has_payload_files, has_zip_files, payload_extensions) =
        game_dir_payload_facts(&header.path);
    GameDirFact {
        name: header.name,
        path: header.path,
        has_payload_files,
        has_zip_files,
        payload_extensions,
    }
}

pub(crate) fn game_roots(roots: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for root in roots {
        let path = Path::new(root);
        let games = if path_name_eq(path, "games") {
            path.to_path_buf()
        } else {
            path.join("games")
        };
        let key = games.display().to_string().to_ascii_lowercase();
        if seen.insert(key) {
            out.push(games);
        }
    }
    out
}

pub(crate) fn core_search_roots(roots: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for root in roots {
        let root = Path::new(root);
        let candidates = if path_name_eq(root, "games") {
            let base = root.parent().unwrap_or(root);
            vec![
                base.join("_Console"),
                base.join("_Computer"),
                base.join("_Arcade/cores"),
                base.join("_LLAPI"),
            ]
        } else if path_name_eq(root, "_Arcade") {
            vec![root.join("cores")]
        } else if path_name_eq(root, "_Console")
            || path_name_eq(root, "_Computer")
            || path_name_eq(root, "_LLAPI")
        {
            vec![root.to_path_buf()]
        } else {
            vec![
                root.join("_Console"),
                root.join("_Computer"),
                root.join("_Arcade/cores"),
                root.join("_LLAPI"),
            ]
        };
        for candidate in candidates {
            let key = candidate.display().to_string().to_ascii_lowercase();
            if seen.insert(key) {
                out.push(candidate);
            }
        }
    }
    out
}

pub(crate) fn should_ignore_game_dir(name: &str) -> bool {
    (name.len() > 1 && name.starts_with('.'))
        || name.eq_ignore_ascii_case("palettes")
        || name.eq_ignore_ascii_case("images")
        || name.eq_ignore_ascii_case("manuals")
        || name.eq_ignore_ascii_case("screenshot")
        || name.eq_ignore_ascii_case("screenshots")
        || name.eq_ignore_ascii_case("screenshot-magik")
        || name.eq_ignore_ascii_case("_organized")
        || name.eq_ignore_ascii_case("boxart")
}

fn game_dir_payload_facts(path: &Path) -> (bool, bool, BTreeSet<String>) {
    let mut has_payload = false;
    let mut has_zip = false;
    let mut payload_extensions = BTreeSet::new();
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .max_depth(2)
        .into_iter()
        .filter_entry(|entry| !should_ignore_path(entry.path()))
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let p = entry.path();
        if path_ext_eq(p, "zip") {
            has_zip = true;
        } else {
            has_payload = true;
            if let Some(ext) = p
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.to_ascii_lowercase())
            {
                payload_extensions.insert(ext);
            }
        }
    }
    (has_payload, has_zip, payload_extensions)
}

fn path_name_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(expected))
}

fn path_ext_eq(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(expected))
}

#[cfg(test)]
fn contains_ignore_ascii_case(values: &[String], needle: &str) -> bool {
    values
        .iter()
        .any(|value| value.eq_ignore_ascii_case(needle))
}

fn should_ignore_hidden_path(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name.len() > 1 && name.starts_with('.')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_temp_dir;

    #[test]
    fn installed_cores_normalize_names_and_skip_sidecars() {
        let root = unique_temp_dir("discovery-installed-cores");
        let console = root.join("_Console");
        std::fs::create_dir_all(&console).expect("create console dir");
        std::fs::write(console.join("Gameboy_20260630.rbf"), b"core").expect("write core");
        std::fs::write(console.join("._C64_20260630.rbf"), b"sidecar").expect("write sidecar");
        std::fs::write(console.join("menu.rbf"), b"menu").expect("write menu");

        let cores = installed_cores_for_roots(&[root.display().to_string()]);

        assert_eq!(cores.len(), 1);
        assert_eq!(cores[0].core_id, "Gameboy");
        assert!(cores[0].path.ends_with("Gameboy_20260630.rbf"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn installed_system_descriptors_keep_setname_and_resolve_real_rbf() {
        let root = unique_temp_dir("discovery-mgl-system");
        let console = root.join("_Console");
        std::fs::create_dir_all(&console).expect("create console dir");
        let rbf = console.join("Atari7800_20260630.rbf");
        std::fs::write(&rbf, b"core").expect("write core");
        std::fs::write(
            console.join("Atari 2600.mgl"),
            r#"<mistergamedescription><rbf>_Console/Atari7800</rbf><setname>Atari2600</setname></mistergamedescription>"#,
        )
        .expect("write descriptor");

        let cores = installed_cores_for_roots(&[root.display().to_string()]);
        let descriptor = cores
            .iter()
            .find(|core| core.core_id == "Atari2600")
            .expect("descriptor-backed system");

        assert_eq!(descriptor.path, rbf);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn tsconf_support_archives_do_not_supply_payload_extensions() {
        let root = unique_temp_dir("discovery-tsconf-support");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("SDCard.zip"), b"not needed for skip test")
            .expect("write support archive");
        std::fs::write(root.join("alt_roms.zip"), b"not needed for skip test")
            .expect("write support archive");

        assert!(archive_member_extensions_for_dir(&root).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn top_level_game_dirs_report_payload_and_zip_shape() {
        let root = unique_temp_dir("discovery-game-dirs");
        let games = root.join("games");
        std::fs::create_dir_all(games.join("Gameboy")).expect("create gameboy dir");
        std::fs::create_dir_all(games.join("NeoGeoPocket")).expect("create ngp dir");
        std::fs::create_dir_all(games.join("Empty")).expect("create empty dir");
        std::fs::create_dir_all(games.join("screenshot-magik")).expect("create media dir");
        std::fs::write(games.join("Gameboy/Tetris.gb"), b"rom").expect("write rom");
        std::fs::write(games.join("NeoGeoPocket/Additions.zip"), b"zip").expect("write zip");
        std::fs::write(games.join("screenshot-magik/Fake.gb"), b"media").expect("write media");

        let dirs = top_level_game_dirs_for_roots(&[root.display().to_string()]);

        assert!(dirs.iter().any(|dir| {
            dir.name == "Gameboy"
                && dir.has_payload_files
                && !dir.has_zip_files
                && dir.payload_extensions.contains("gb")
        }));
        assert!(dirs.iter().any(|dir| {
            dir.name == "NeoGeoPocket" && !dir.has_payload_files && dir.has_zip_files
        }));
        assert!(dirs
            .iter()
            .any(|dir| { dir.name == "Empty" && !dir.has_payloadish_files() }));
        assert!(!dirs.iter().any(|dir| dir.name == "screenshot-magik"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn top_level_game_dirs_do_not_follow_symlinks() {
        let root = unique_temp_dir("discovery-game-dir-symlink");
        let outside = unique_temp_dir("discovery-game-dir-symlink-target");
        let games = root.join("games");
        std::fs::create_dir_all(&games).expect("create games dir");
        std::fs::create_dir_all(&outside).expect("create outside dir");
        std::fs::write(outside.join("Ghost.gb"), b"rom").expect("write outside rom");
        std::os::unix::fs::symlink(&outside, games.join("Gameboy")).expect("create symlink dir");

        let dirs = top_level_game_dirs_for_roots(&[root.display().to_string()]);

        assert!(dirs.is_empty());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }
}
