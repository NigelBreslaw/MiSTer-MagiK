// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared filesystem facts for catalog profile planning and audit.

use crate::catalog_scan::should_ignore_path;
use crate::launch_profiles;
use crate::namespace_walk;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
#[cfg(feature = "builder")]
use std::time::Instant;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InstalledCore {
    pub(crate) core_id: String,
    pub(crate) path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct GameDirFact {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) signature: GameDirSignature,
    pub(crate) has_payload_files: bool,
    pub(crate) has_zip_files: bool,
    /// Visible ZIP files immediately below the game directory. The coverage
    /// audit retains one diagnostic row per such file when a cataloged profile
    /// has no archive rules, so keep these paths from the primary namespace
    /// walk instead of reopening every exFAT directory before CatalogReady.
    pub(crate) direct_zip_paths: Vec<PathBuf>,
    /// Metadata signatures for immediate child directories when payload shape
    /// is runtime-derived. They let warm validation cover the primary walk's
    /// depth-two namespace without enumerating every payload again.
    pub(crate) nested_probe_signatures: Vec<(PathBuf, GameDirSignature)>,
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
    pub(crate) signature: GameDirSignature,
    /// True only when the checked scan plan proved this exact path is an
    /// ordinary directory.  Unchecked and uncertain entries retain the old
    /// canonicalization/type-check fallback in the generic scanner.
    pub(crate) confirmed_directory: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum GameDirSignature {
    Present { len: u64, mtime_nanos: i64 },
    Unavailable,
}

impl GameDirSignature {
    pub(crate) fn from_path(path: &Path) -> Self {
        let Ok(metadata) = std::fs::metadata(path) else {
            return Self::Unavailable;
        };
        let Some(mtime_nanos) = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64)
        else {
            return Self::Unavailable;
        };
        Self::Present {
            len: metadata.len(),
            mtime_nanos,
        }
    }

    pub(crate) fn from_namespace_signature(signature: Option<(u64, i64)>) -> Self {
        signature.map_or(Self::Unavailable, |(len, mtime_nanos)| Self::Present {
            len,
            mtime_nanos,
        })
    }

    #[cfg(feature = "builder")]
    pub(crate) fn from_known_path_metadata(len: u64, modified_ns: i128) -> Self {
        Self::Present {
            len,
            mtime_nanos: modified_ns.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64,
        }
    }
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

#[cfg(feature = "builder")]
pub(crate) fn installed_cores_for_roots_checked(
    roots: &[String],
) -> Result<Vec<InstalledCore>, String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for search_root in core_search_roots(roots) {
        let mut directories = vec![search_root.clone()];
        if path_name_eq(&search_root, "_LLAPI") {
            directories.push(search_root.join("cores"));
        }
        for directory in directories {
            let Some(entries) = read_dir_entries_checked(&directory)? else {
                continue;
            };
            for entry in entries {
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
    append_mgl_system_descriptors_checked(roots, &mut out, &mut seen)?;
    Ok(out)
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
            let key = format!(
                "{}\t{}",
                core_id.to_ascii_lowercase(),
                target.path.display()
            );
            if seen.insert(key) {
                out.push(InstalledCore {
                    core_id,
                    path: target.path.clone(),
                });
            }
        }
    }
}

#[cfg(feature = "builder")]
fn append_mgl_system_descriptors_checked(
    roots: &[String],
    out: &mut Vec<InstalledCore>,
    seen: &mut BTreeSet<String>,
) -> Result<(), String> {
    let physical = out.clone();
    for search_root in core_search_roots(roots) {
        let Some(entries) = read_dir_entries_checked(&search_root)? else {
            continue;
        };
        for entry in entries {
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
            let key = format!(
                "{}\t{}",
                core_id.to_ascii_lowercase(),
                target.path.display()
            );
            if seen.insert(key) {
                out.push(InstalledCore {
                    core_id,
                    path: target.path.clone(),
                });
            }
        }
    }
    Ok(())
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
    let Ok(mut file) = File::open(path) else {
        return;
    };
    let Ok(len) = file.metadata().map(|metadata| metadata.len()) else {
        return;
    };
    if len < 22 {
        return;
    }
    let tail_len = len.min(66_000) as usize;
    if file.seek(SeekFrom::End(-(tail_len as i64))).is_err() {
        return;
    }
    let mut tail = vec![0; tail_len];
    if file.read_exact(&mut tail).is_err() {
        return;
    }
    let Some(eocd) = crate::library_db::find_eocd(&tail) else {
        return;
    };
    let entries = usize::from(crate::library_db::le_u16(&tail[eocd + 10..eocd + 12])).min(4096);
    let size = u64::from(crate::library_db::le_u32(&tail[eocd + 12..eocd + 16]));
    let offset = u64::from(crate::library_db::le_u32(&tail[eocd + 16..eocd + 20]));
    if offset.checked_add(size).is_none_or(|end| end > len)
        || file.seek(SeekFrom::Start(offset)).is_err()
    {
        return;
    }
    for _ in 0..entries {
        let mut header = [0; 46];
        if file.read_exact(&mut header).is_err()
            || crate::library_db::le_u32(&header[0..4]) != 0x0201_4b50
        {
            return;
        }
        let name_len = usize::from(crate::library_db::le_u16(&header[28..30]));
        let extra_len = usize::from(crate::library_db::le_u16(&header[30..32]));
        let comment_len = usize::from(crate::library_db::le_u16(&header[32..34]));
        if name_len > 4096 {
            return;
        }
        let mut name = vec![0; name_len];
        if file.read_exact(&mut name).is_err() {
            return;
        }
        if file
            .seek(SeekFrom::Current((extra_len + comment_len) as i64))
            .is_err()
        {
            return;
        }
        let name = String::from_utf8_lossy(&name);
        let member = Path::new(name.as_ref());
        if !name.ends_with('/')
            && !should_ignore_hidden_path(member)
            && let Some(ext) = member.extension().and_then(|value| value.to_str())
        {
            extensions.insert(ext.to_ascii_lowercase());
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
    top_level_game_dir_probe_headers_for_roots_excluding(roots, excluded_names)
        .into_iter()
        .map(|header| {
            let (
                has_payload_files,
                has_zip_files,
                direct_zip_paths,
                nested_probe_signatures,
                payload_extensions,
            ) = game_dir_payload_facts(&header.path);
            GameDirFact {
                name: header.name,
                path: header.path,
                signature: header.signature,
                has_payload_files,
                has_zip_files,
                direct_zip_paths,
                nested_probe_signatures,
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
                    signature: GameDirSignature::Unavailable,
                    path,
                    confirmed_directory: false,
                });
            }
        }
        entries.sort_by_key(|entry| entry.name.to_ascii_lowercase());
        out.extend(entries);
    }
    out
}

#[cfg(feature = "builder")]
pub(crate) fn top_level_game_dir_headers_for_roots_excluding_checked(
    roots: &[String],
    excluded_names: &BTreeSet<String>,
) -> Result<Vec<GameDirHeader>, String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for game_root in game_roots(roots) {
        let root_started = Instant::now();
        let mut entries_examined = 0_u64;
        let mut file_type_count = 0_u64;
        let mut file_type_us = 0_u64;
        let mut rejected_entries = 0_u64;
        let Some(read_dir) = read_dir_entries_checked(&game_root)? else {
            crate::catalog_logln!(
                "catalog_game_header_probe_tsv\troot={}\tentries=0\tcandidates=0\taccepted=0\trejected=0\tfile_type_count=0\tfile_type_us=0\tmetadata_probe_count=0\tmetadata_probe_us=0\telapsed_us={}",
                game_root.display(),
                root_started.elapsed().as_micros()
            );
            continue;
        };
        let mut candidates = Vec::new();
        for entry in read_dir {
            entries_examined = entries_examined.saturating_add(1);
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                rejected_entries = rejected_entries.saturating_add(1);
                continue;
            };
            if should_ignore_game_dir(name) {
                rejected_entries = rejected_entries.saturating_add(1);
                continue;
            }
            let file_type_started = Instant::now();
            let file_type = entry
                .file_type()
                .map_err(|error| format!("inspect {}: {error}", path.display()))?;
            file_type_us =
                file_type_us.saturating_add(file_type_started.elapsed().as_micros() as u64);
            file_type_count = file_type_count.saturating_add(1);
            if file_type.is_symlink() {
                rejected_entries = rejected_entries.saturating_add(1);
                continue;
            }
            if excluded_names.contains(&name.to_ascii_lowercase()) {
                rejected_entries = rejected_entries.saturating_add(1);
                continue;
            }
            let key = path.display().to_string().to_ascii_lowercase();
            if seen.insert(key) {
                candidates.push((name.to_string(), path, file_type.is_dir()));
            } else {
                rejected_entries = rejected_entries.saturating_add(1);
            }
        }
        // Keep the exact type check serial while avoiding a separate parent
        // pathname lookup for every entry.  `fstatat` follows the same symlink
        // semantics as the old metadata fallback; entries whose directory
        // type was not known by `readdir` remain unconfirmed and therefore
        // take that fallback later.
        let child_paths = candidates
            .iter()
            .map(|(_, path, _)| path.clone())
            .collect::<Vec<_>>();
        let metadata_started = Instant::now();
        let metadata = namespace_walk::probe_known_path_metadata(&game_root, &child_paths);
        let metadata_probe_us = metadata_started.elapsed().as_micros() as u64;
        let mut entries = Vec::new();
        for ((name, path, readdir_is_dir), metadata) in candidates.into_iter().zip(metadata) {
            let Some(metadata) = metadata else {
                entries.push(GameDirHeader {
                    name,
                    signature: GameDirSignature::Unavailable,
                    path,
                    confirmed_directory: false,
                });
                continue;
            };
            if !metadata.is_dir {
                continue;
            }
            entries.push(GameDirHeader {
                name,
                signature: GameDirSignature::from_known_path_metadata(
                    metadata.size,
                    metadata.modified_ns,
                ),
                path,
                confirmed_directory: readdir_is_dir,
            });
        }
        entries.sort_by_key(|entry| entry.name.to_ascii_lowercase());
        crate::catalog_logln!(
            "catalog_game_header_probe_tsv\troot={}\tentries={}\tcandidates={}\taccepted={}\trejected={}\tfile_type_count={}\tfile_type_us={}\tmetadata_probe_count={}\tmetadata_probe_us={}\telapsed_us={}",
            game_root.display(),
            entries_examined,
            child_paths.len(),
            entries.len(),
            rejected_entries,
            file_type_count,
            file_type_us,
            child_paths.len(),
            metadata_probe_us,
            root_started.elapsed().as_micros()
        );
        out.extend(entries);
    }
    Ok(out)
}

/// Adds compact directory signatures to the name-only cold scan headers.
/// Linux obtains them by opening each direct child relative to `/games` and
/// reading metadata from that fd, avoiding one full exFAT path lookup per
/// system directory.
pub(crate) fn top_level_game_dir_probe_headers_for_roots_excluding(
    roots: &[String],
    excluded_names: &BTreeSet<String>,
) -> Vec<GameDirHeader> {
    let mut headers = top_level_game_dir_headers_for_roots_excluding(roots, excluded_names);
    for game_root in game_roots(roots) {
        let indexes = headers
            .iter()
            .enumerate()
            .filter_map(|(index, header)| {
                (header.path.parent() == Some(game_root.as_path())).then_some(index)
            })
            .collect::<Vec<_>>();
        let child_paths = indexes
            .iter()
            .map(|index| headers[*index].path.clone())
            .collect::<Vec<_>>();
        let probe = namespace_walk::probe_directory_signatures(&game_root, &child_paths);
        for (index, signature) in indexes.into_iter().zip(probe.child_signatures) {
            headers[index].signature = GameDirSignature::from_namespace_signature(signature);
        }
    }
    headers
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
    let (
        has_payload_files,
        has_zip_files,
        direct_zip_paths,
        nested_probe_signatures,
        payload_extensions,
    ) = game_dir_payload_facts(&header.path);
    GameDirFact {
        name: header.name,
        path: header.path,
        signature: header.signature,
        has_payload_files,
        has_zip_files,
        direct_zip_paths,
        nested_probe_signatures,
        payload_extensions,
    }
}

#[cfg(feature = "builder")]
pub(crate) fn game_dir_payload_facts_for_header_checked(
    header: GameDirHeader,
) -> Result<GameDirFact, String> {
    let (
        has_payload_files,
        has_zip_files,
        direct_zip_paths,
        nested_probe_signatures,
        payload_extensions,
    ) = game_dir_payload_facts_checked(&header.path)?;
    Ok(GameDirFact {
        name: header.name,
        path: header.path,
        signature: header.signature,
        has_payload_files,
        has_zip_files,
        direct_zip_paths,
        nested_probe_signatures,
        payload_extensions,
    })
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

type GameDirPayloadFacts = (
    bool,
    bool,
    Vec<PathBuf>,
    Vec<(PathBuf, GameDirSignature)>,
    BTreeSet<String>,
);

fn game_dir_payload_facts(path: &Path) -> GameDirPayloadFacts {
    let mut has_payload = false;
    let mut has_zip = false;
    let mut direct_zip_paths = Vec::new();
    let mut nested_probe_signatures = Vec::new();
    let mut payload_extensions = BTreeSet::new();
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .max_depth(2)
        .into_iter()
        .filter_entry(|entry| !should_ignore_path(entry.path()))
        .filter_map(Result::ok)
    {
        if entry.file_type().is_dir() && entry.depth() == 1 {
            nested_probe_signatures.push((
                entry.path().to_path_buf(),
                GameDirSignature::from_path(entry.path()),
            ));
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let p = entry.path();
        if path_ext_eq(p, "zip") {
            has_zip = true;
            if entry.depth() == 1 {
                direct_zip_paths.push(p.to_path_buf());
            }
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
    direct_zip_paths.sort_by_cached_key(|path| path.to_string_lossy().to_ascii_lowercase());
    nested_probe_signatures.sort_by_cached_key(|(path, _)| {
        (path.to_string_lossy().to_ascii_lowercase(), path.clone())
    });
    (
        has_payload,
        has_zip,
        direct_zip_paths,
        nested_probe_signatures,
        payload_extensions,
    )
}

#[cfg(feature = "builder")]
fn game_dir_payload_facts_checked(path: &Path) -> Result<GameDirPayloadFacts, String> {
    let mut last_error = None;
    for _ in 0..2 {
        let mut has_payload = false;
        let mut has_zip = false;
        let mut direct_zip_paths = Vec::new();
        let mut nested_probe_signatures = Vec::new();
        let mut payload_extensions = BTreeSet::new();
        let mut failed = None;
        for entry in walkdir::WalkDir::new(path)
            .follow_links(false)
            .max_depth(2)
            .into_iter()
            .filter_entry(|entry| !should_ignore_path(entry.path()))
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    failed = Some(error.to_string());
                    break;
                }
            };
            if entry.file_type().is_dir() && entry.depth() == 1 {
                nested_probe_signatures.push((
                    entry.path().to_path_buf(),
                    GameDirSignature::from_path(entry.path()),
                ));
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let entry_path = entry.path();
            if path_ext_eq(entry_path, "zip") {
                has_zip = true;
                if entry.depth() == 1 {
                    direct_zip_paths.push(entry_path.to_path_buf());
                }
            } else {
                has_payload = true;
                if let Some(ext) = entry_path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.to_ascii_lowercase())
                {
                    payload_extensions.insert(ext);
                }
            }
        }
        if let Some(error) = failed {
            last_error = Some(error);
            crate::cooperative_work::checkpoint();
            continue;
        }
        direct_zip_paths
            .sort_by_cached_key(|entry_path| entry_path.to_string_lossy().to_ascii_lowercase());
        nested_probe_signatures.sort_by_cached_key(|(entry_path, _)| {
            (
                entry_path.to_string_lossy().to_ascii_lowercase(),
                entry_path.clone(),
            )
        });
        return Ok((
            has_payload,
            has_zip,
            direct_zip_paths,
            nested_probe_signatures,
            payload_extensions,
        ));
    }
    Err(format!(
        "enumerate {}: {}",
        path.display(),
        last_error.unwrap_or_else(|| "unknown directory error".to_string())
    ))
}

#[cfg(feature = "builder")]
fn read_dir_entries_checked(path: &Path) -> Result<Option<Vec<std::fs::DirEntry>>, String> {
    let mut last_error = None;
    for _ in 0..2 {
        match std::fs::read_dir(path) {
            Ok(entries) => match entries.collect::<Result<Vec<_>, _>>() {
                Ok(entries) => return Ok(Some(entries)),
                Err(error) => last_error = Some(error),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => last_error = Some(error),
        }
        crate::cooperative_work::checkpoint();
    }
    Err(format!(
        "enumerate {}: {}",
        path.display(),
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unknown directory error".to_string())
    ))
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
        assert!(
            dirs.iter()
                .any(|dir| { dir.name == "Empty" && !dir.has_payloadish_files() })
        );
        assert!(!dirs.iter().any(|dir| dir.name == "screenshot-magik"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cold_game_dir_headers_defer_metadata_to_the_namespace_scan() {
        let root = unique_temp_dir("discovery-game-dir-header-signatures");
        std::fs::create_dir_all(root.join("games/NES")).expect("create NES dir");
        let roots = vec![root.display().to_string()];

        let cold = top_level_game_dir_headers_for_roots_excluding(&roots, &BTreeSet::new());
        let probe = top_level_game_dir_probe_headers_for_roots_excluding(&roots, &BTreeSet::new());

        assert_eq!(cold.len(), 1);
        assert_eq!(cold[0].signature, GameDirSignature::Unavailable);
        assert!(matches!(
            probe[0].signature,
            GameDirSignature::Present { .. }
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(feature = "builder")]
    #[test]
    fn checked_game_dir_headers_reuse_only_exact_confirmed_directories() {
        let root = unique_temp_dir("discovery-checked-game-dir-headers");
        let games = root.join("games");
        std::fs::create_dir_all(games.join("NES")).expect("create NES dir");
        std::fs::write(games.join("not-a-directory"), b"file").expect("write file");
        #[cfg(unix)]
        std::os::unix::fs::symlink(games.join("NES"), games.join("NES-link"))
            .expect("create symlink");

        let roots = vec![root.display().to_string()];
        let headers =
            top_level_game_dir_headers_for_roots_excluding_checked(&roots, &BTreeSet::new())
                .expect("checked headers");

        let nes = headers
            .iter()
            .find(|header| header.name == "NES")
            .expect("NES header");
        assert!(nes.confirmed_directory);
        assert!(matches!(nes.signature, GameDirSignature::Present { .. }));
        assert!(
            !headers
                .iter()
                .any(|header| header.name == "not-a-directory")
        );
        #[cfg(unix)]
        assert!(!headers.iter().any(|header| header.name == "NES-link"));

        let plan =
            crate::launch_profiles::CatalogScanPlan::try_for_roots(&roots).expect("checked plan");
        assert!(plan.header_for_known_game_dir(&root, "NES").is_some());
        assert!(plan.header_for_known_game_dir(&root, "nes").is_none());

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
