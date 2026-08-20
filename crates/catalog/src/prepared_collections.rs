// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared metadata for collections that provide their own one-click launch artifacts.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::media_metadata::{MglInspection, inspect_mgl, resolve_mgl_payload_path};

pub const PREPARED_COLLECTION_ADAPTER_VERSION: u32 = 6;

#[derive(Default)]
pub(crate) struct PreparedPayloadIndex {
    exact_files: HashSet<PathBuf>,
    ascii_files: HashMap<String, PathBuf>,
    ascii_collisions: HashSet<String>,
    complete_roots: Vec<CompletePayloadRoot>,
    lookup_files: Cell<usize>,
    lookup_missing: Cell<usize>,
    lookup_unknown: Cell<usize>,
    live_fallbacks: Cell<usize>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PreparedPayloadIndexStats {
    pub(crate) files: usize,
    pub(crate) missing: usize,
    pub(crate) unknown: usize,
    pub(crate) live_fallbacks: usize,
}

struct CompletePayloadRoot {
    exact: PathBuf,
    ascii: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FilePresence {
    File,
    NotFile,
    Unknown,
}

impl PreparedPayloadIndex {
    pub(crate) fn from_library_roots(roots: &[String]) -> Self {
        let mut index = Self::default();
        for storage_root in storage_roots_for_library_roots(roots) {
            for root in [
                storage_root.join("_DOS Games"),
                storage_root.join("games/AO486"),
            ] {
                index.add_complete_root(&root);
            }
        }
        index
    }

    pub(crate) fn file_count(&self) -> usize {
        self.exact_files.len()
    }

    pub(crate) fn complete_root_count(&self) -> usize {
        self.complete_roots.len()
    }

    pub(crate) fn lookup_stats(&self) -> PreparedPayloadIndexStats {
        PreparedPayloadIndexStats {
            files: self.lookup_files.get(),
            missing: self.lookup_missing.get(),
            unknown: self.lookup_unknown.get(),
            live_fallbacks: self.live_fallbacks.get(),
        }
    }

    pub(crate) fn resolve_0mhz_payload_path(&self, mgl_path: &Path, payload: &str) -> PathBuf {
        let local = resolve_mgl_payload_path(mgl_path, payload);
        match self.presence(&local) {
            FilePresence::File => return local,
            FilePresence::Unknown => {
                self.record_live_fallback();
                return resolve_0mhz_payload_path(mgl_path, payload);
            }
            FilePresence::NotFile => {}
        }
        if payload.starts_with('/') || payload.starts_with("games/") {
            return local;
        }
        let Some(collection_payload) = zero_mhz_collection_payload_path(mgl_path, payload) else {
            return local;
        };
        match self.presence(&collection_payload) {
            FilePresence::File => collection_payload,
            FilePresence::NotFile => local,
            FilePresence::Unknown => {
                self.record_live_fallback();
                resolve_0mhz_payload_path(mgl_path, payload)
            }
        }
    }

    pub(crate) fn path_is_file(&self, path: &Path) -> bool {
        match self.presence(path) {
            FilePresence::File => true,
            FilePresence::NotFile => false,
            FilePresence::Unknown => {
                self.record_live_fallback();
                path.is_file()
            }
        }
    }

    fn add_complete_root(&mut self, root: &Path) {
        if std::fs::symlink_metadata(root)
            .ok()
            .is_none_or(|metadata| !metadata.file_type().is_dir())
        {
            return;
        }
        let mut complete = true;
        let mut ascii_entries = HashMap::<String, PathBuf>::new();
        for (entry_index, entry) in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .enumerate()
        {
            if entry_index.is_multiple_of(16) {
                crate::cooperative_work::checkpoint();
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            if entry.path() == root {
                continue;
            }
            let normalized = lexically_normalized_path(entry.path());
            if let Some(key) = ascii_path_key(&normalized)
                && ascii_entries
                    .insert(key, normalized.clone())
                    .is_some_and(|previous| previous != normalized)
            {
                complete = false;
            }
            let kind = entry.file_type();
            if kind.is_file() {
                self.insert_file(normalized);
            } else if !kind.is_dir() {
                // Path::is_file follows symlinks and custom library roots may
                // contain other filesystem types. Keep every negative lookup
                // under such a root conservative.
                complete = false;
            }
        }
        if complete {
            let exact = lexically_normalized_path(root);
            if !self
                .complete_roots
                .iter()
                .any(|existing| existing.exact == exact)
            {
                self.complete_roots.push(CompletePayloadRoot {
                    ascii: ascii_path_key(&exact),
                    exact,
                });
            }
        }
    }

    fn insert_file(&mut self, path: PathBuf) {
        self.exact_files.insert(path.clone());
        let Some(key) = ascii_path_key(&path) else {
            return;
        };
        if self
            .ascii_files
            .insert(key.clone(), path.clone())
            .is_some_and(|previous| previous != path)
        {
            self.ascii_collisions.insert(key);
        }
    }

    fn presence(&self, path: &Path) -> FilePresence {
        let exact = lexically_normalized_path(path);
        if self.exact_files.contains(&exact) {
            return self.record_presence(FilePresence::File);
        }
        let Some(ascii) = ascii_path_key(&exact) else {
            return self.record_presence(FilePresence::Unknown);
        };
        if self.ascii_collisions.contains(&ascii) {
            return self.record_presence(FilePresence::Unknown);
        }
        if self.ascii_files.contains_key(&ascii) {
            // The mounted MiSTer exFAT volume is case-insensitive, while host
            // fixtures may be case-sensitive. A folded-only match therefore
            // needs the live filesystem fallback to preserve both contracts.
            return self.record_presence(FilePresence::Unknown);
        }
        if self.complete_roots.iter().any(|root| {
            root.ascii
                .as_deref()
                .is_some_and(|root| ascii_path_is_within(&ascii, root))
        }) {
            return self.record_presence(FilePresence::NotFile);
        }
        self.record_presence(FilePresence::Unknown)
    }

    fn record_presence(&self, presence: FilePresence) -> FilePresence {
        let counter = match presence {
            FilePresence::File => &self.lookup_files,
            FilePresence::NotFile => &self.lookup_missing,
            FilePresence::Unknown => &self.lookup_unknown,
        };
        counter.set(counter.get().saturating_add(1));
        presence
    }

    fn record_live_fallback(&self) {
        self.live_fallbacks
            .set(self.live_fallbacks.get().saturating_add(1));
    }
}

fn lexically_normalized_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => {
                normalized.push(prefix.as_os_str());
            }
            std::path::Component::RootDir => normalized.push(Path::new("/")),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let _ = normalized.pop();
            }
            std::path::Component::Normal(value) => normalized.push(value),
        }
    }
    normalized
}

fn ascii_path_key(path: &Path) -> Option<String> {
    path.to_str()
        .filter(|path| path.is_ascii())
        .map(str::to_ascii_lowercase)
}

fn ascii_path_is_within(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

pub fn storage_roots_for_library_roots(roots: &[String]) -> Vec<PathBuf> {
    let mut storage_roots = Vec::new();
    for root in roots {
        let storage_root = storage_root_for_library_root(Path::new(root));
        if !storage_roots.contains(&storage_root) {
            storage_roots.push(storage_root);
        }
    }
    storage_roots
}

fn storage_root_for_library_root(root: &Path) -> PathBuf {
    let mut prefix = PathBuf::new();
    for component in root.components() {
        let value = component.as_os_str().to_string_lossy();
        if matches!(
            value.as_ref(),
            "games" | "_Arcade" | "_Games" | "_DOS Games" | "_LLAPI" | "_Computer"
        ) {
            return prefix;
        }
        prefix.push(component.as_os_str());
    }
    root.to_path_buf()
}

#[cfg(test)]
mod cooperative_tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn prepared_payload_walk_respects_background_gate() {
        let _test_lock = crate::cooperative_work::TEST_LOCK.lock().unwrap();
        let root = crate::test_support::unique_temp_dir("prepared-payload-gate");
        let payload_root = root.join("_DOS Games");
        std::fs::create_dir_all(&payload_root).unwrap();
        for index in 0..40 {
            std::fs::write(payload_root.join(format!("game-{index}.vhd")), b"x").unwrap();
        }
        crate::cooperative_work::set_background_allowed(false);
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker_root = root.clone();
        let worker = std::thread::spawn(move || {
            let _scope = crate::cooperative_work::BackgroundScope::enter();
            started_tx.send(()).unwrap();
            let index =
                PreparedPayloadIndex::from_library_roots(&[worker_root.display().to_string()]);
            done_tx.send(index.file_count()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(30)).is_err());
        crate::cooperative_work::set_background_allowed(true);
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 40);
        worker.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PreparedCollectionId {
    AmigaVision,
    ZeroMhz,
    Neon68k,
    OneLoad64,
}

impl PreparedCollectionId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AmigaVision => "amigavision",
            Self::ZeroMhz => "0mhz",
            Self::Neon68k => "neon68k",
            Self::OneLoad64 => "oneload64",
        }
    }
}

impl fmt::Display for PreparedCollectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LaunchQuality {
    Prepared,
    Generic,
}

impl LaunchQuality {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Generic => "generic",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PreparedLaunchProvenance {
    pub collection_id: PreparedCollectionId,
    pub launch_quality: LaunchQuality,
    pub adapter_version: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedLaunchDiagnostic {
    pub(crate) collection_id: PreparedCollectionId,
    pub(crate) status: &'static str,
    pub(crate) reason: String,
}

impl PreparedLaunchProvenance {
    pub const fn prepared(collection_id: PreparedCollectionId) -> Self {
        Self {
            collection_id,
            launch_quality: LaunchQuality::Prepared,
            adapter_version: PREPARED_COLLECTION_ADAPTER_VERSION,
        }
    }
}

pub(crate) fn validate_0mhz_mgl(path: &Path) -> Result<MglInspection, String> {
    let inspection = inspect_mgl(path)?;
    validate_0mhz_mgl_inspection(path, &inspection)?;
    Ok(inspection)
}

pub(crate) fn validate_0mhz_mgl_inspection(
    path: &Path,
    inspection: &MglInspection,
) -> Result<(), String> {
    validate_0mhz_mgl_inspection_with(inspection, |payload| {
        let path = resolve_0mhz_payload_path(path, payload);
        let exists = path.is_file();
        (path, exists)
    })
}

pub(crate) fn validate_0mhz_mgl_inspection_with_index(
    path: &Path,
    inspection: &MglInspection,
    index: &PreparedPayloadIndex,
) -> Result<(), String> {
    validate_0mhz_mgl_inspection_with(inspection, |payload| {
        let path = index.resolve_0mhz_payload_path(path, payload);
        let exists = index.path_is_file(&path);
        (path, exists)
    })
}

fn validate_0mhz_mgl_inspection_with(
    inspection: &MglInspection,
    mut resolve: impl FnMut(&str) -> (PathBuf, bool),
) -> Result<(), String> {
    let rbf = inspection
        .rbf
        .as_deref()
        .ok_or_else(|| "0MHz MGL has no RBF".to_string())?;
    if !crate::library_db::normalize_id(rbf).ends_with("ao486") {
        return Err(format!("0MHz MGL targets {rbf}, expected AO486"));
    }
    if inspection.files.is_empty() {
        return Err("0MHz MGL has no file mount actions".to_string());
    }
    if inspection.reset_count == 0 {
        return Err("0MHz MGL has no reset action".to_string());
    }
    for action in &inspection.files {
        let (payload, exists) = resolve(&action.path);
        if !exists {
            return Err(format!(
                "0MHz MGL payload is missing: {}",
                payload.display()
            ));
        }
    }
    Ok(())
}

pub(crate) fn resolve_0mhz_payload_path(mgl_path: &Path, payload: &str) -> PathBuf {
    resolve_0mhz_payload_path_with(mgl_path, payload, Path::is_file)
}

fn resolve_0mhz_payload_path_with(
    mgl_path: &Path,
    payload: &str,
    mut is_file: impl FnMut(&Path) -> bool,
) -> PathBuf {
    let local = resolve_mgl_payload_path(mgl_path, payload);
    if is_file(&local) || payload.starts_with('/') || payload.starts_with("games/") {
        return local;
    }

    let Some(collection_payload) = zero_mhz_collection_payload_path(mgl_path, payload) else {
        return local;
    };
    if is_file(&collection_payload) {
        collection_payload
    } else {
        local
    }
}

fn zero_mhz_collection_payload_path(mgl_path: &Path, payload: &str) -> Option<PathBuf> {
    let dos_games_root = mgl_path.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("_DOS Games"))
    })?;
    let storage_root = dos_games_root.parent()?;
    Some(storage_root.join("games/AO486").join(payload))
}

pub(crate) fn validate_neon68k_mgl(path: &Path) -> Result<MglInspection, String> {
    let inspection = inspect_mgl(path)?;
    validate_neon68k_mgl_inspection(path, &inspection)?;
    Ok(inspection)
}

pub(crate) fn is_neon68k_launcher_root(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("_X68000 Games") || name.eq_ignore_ascii_case("X68000 Games")
        })
        && path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("_Computer"))
}

pub(crate) fn is_followable_neon68k_launcher_root_symlink(path: &Path) -> bool {
    is_neon68k_launcher_root(path)
        && std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        && std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

pub(crate) fn neon68k_launcher_root_is_available(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
        || is_followable_neon68k_launcher_root_symlink(path)
}

pub(crate) fn neon68k_launcher_roots_for_library_root(configured_root: &Path) -> Vec<PathBuf> {
    let name = configured_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if [
        "_Arcade",
        "_Games",
        "_DOS Games",
        "_Console (autoboot)",
        "_LLAPI",
        "_X68000 Games",
        "X68000 Games",
    ]
    .iter()
    .any(|candidate| name.eq_ignore_ascii_case(candidate))
    {
        return Vec::new();
    }
    let storage_root = if name.eq_ignore_ascii_case("games") {
        configured_root.parent().unwrap_or(configured_root)
    } else {
        configured_root
    };
    vec![
        storage_root.join("_Computer/_X68000 Games"),
        storage_root.join("_Computer/X68000 Games"),
    ]
}

pub(crate) fn neon68k_payload_signature_for_library_root(
    configured_root: &Path,
) -> Option<PathBuf> {
    let name = configured_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if [
        "_Arcade",
        "_Games",
        "_DOS Games",
        "_Console (autoboot)",
        "_LLAPI",
        "_X68000 Games",
        "X68000 Games",
    ]
    .iter()
    .any(|candidate| name.eq_ignore_ascii_case(candidate))
    {
        return None;
    }
    let games_root = if name.eq_ignore_ascii_case("games") {
        configured_root.to_path_buf()
    } else {
        configured_root.join("games")
    };
    Some(games_root.join("X68000/boot3.vhd"))
}

pub(crate) fn neon68k_duplicate_alias_path(root: &Path, path: &Path) -> bool {
    is_neon68k_launcher_root(root)
        && path
            .strip_prefix(root)
            .ok()
            .and_then(|relative| relative.components().next())
            .and_then(|component| component.as_os_str().to_str())
            .is_some_and(|component| component.eq_ignore_ascii_case("_Genre"))
}

pub(crate) fn validate_neon68k_mgl_inspection(
    path: &Path,
    inspection: &MglInspection,
) -> Result<(), String> {
    let rbf = inspection
        .rbf
        .as_deref()
        .ok_or_else(|| "Neon68K MGL has no RBF".to_string())?;
    if !crate::library_db::normalize_id(rbf).ends_with("x68000") {
        return Err(format!("Neon68K MGL targets {rbf}, expected X68000"));
    }
    if inspection
        .setname
        .as_deref()
        .is_none_or(|setname| setname.trim().is_empty())
    {
        return Err("Neon68K MGL has no setname".to_string());
    }
    let mut hdf_count = 0usize;
    for action in &inspection.files {
        let payload = resolve_neon68k_payload_path(path, &action.path);
        if !payload.is_file() {
            return Err(format!(
                "Neon68K MGL payload is missing: {}",
                payload.display()
            ));
        }
        if payload
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("hdf"))
        {
            hdf_count = hdf_count.saturating_add(1);
        }
    }
    if hdf_count == 0 {
        return Err("Neon68K MGL has no HDF mount action".to_string());
    }
    Ok(())
}

pub(crate) fn resolve_neon68k_payload_path(mgl_path: &Path, payload: &str) -> PathBuf {
    let local = resolve_mgl_payload_path(mgl_path, payload);
    if local.is_file() {
        return local;
    }
    let Some(storage_root) = mgl_path.ancestors().find_map(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("_Computer"))
            .then(|| ancestor.parent())
            .flatten()
    }) else {
        return local;
    };
    let collection_payload = storage_root.join("games/X68000").join(payload);
    if collection_payload.is_file() {
        collection_payload
    } else {
        local
    }
}

pub(crate) fn neon68k_source_category(path: &Path) -> Option<String> {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .find_map(|component| {
            let normalized = component.to_ascii_lowercase();
            if normalized.contains("keyboard") || normalized.contains("mouse") {
                Some("Keyboard + Mouse".to_string())
            } else if normalized.contains("major") && normalized.contains("bug") {
                Some("Major Bugs".to_string())
            } else if normalized.contains("minor") && normalized.contains("bug") {
                Some("Minor Bugs".to_string())
            } else {
                None
            }
        })
}

pub(crate) fn oneload64_provenance(path: &Path) -> Option<PreparedLaunchProvenance> {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("crt"))
    {
        return None;
    }
    let install_root = path.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_oneload64_install_name)
    })?;
    if !oneload64_root_has_signature(install_root) || oneload64_path_is_excluded(path, install_root)
    {
        return None;
    }
    Some(PreparedLaunchProvenance::prepared(
        PreparedCollectionId::OneLoad64,
    ))
}

fn oneload64_root_has_signature(root: &Path) -> bool {
    // A catalog build runs in a fresh standalone process. The install root
    // cannot meaningfully change underneath that one scan, so key this
    // process-local fact by path instead of statting the same exFAT directory
    // once for every CRT payload.
    type SignatureCache = std::collections::HashMap<std::path::PathBuf, bool>;
    static CACHE: OnceLock<Mutex<SignatureCache>> = OnceLock::new();
    let key = root.to_path_buf();
    let cache = CACHE.get_or_init(|| Mutex::new(SignatureCache::new()));
    if let Some(cached) = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .copied()
    {
        return cached;
    }
    let valid = std::fs::read_dir(root).ok().is_some_and(|entries| {
        entries.flatten().any(|entry| {
            entry.file_type().ok().is_some_and(|kind| kind.is_dir())
                && matches!(
                    compact_name(&entry.file_name().to_string_lossy()).as_str(),
                    "multiload64" | "dumps" | "alternativeformats"
                )
        })
    });
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, valid);
    valid
}

fn oneload64_path_is_excluded(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).ok().is_some_and(|relative| {
        relative.components().any(|component| {
            matches!(
                compact_name(&component.as_os_str().to_string_lossy()).as_str(),
                "dumps" | "alternativeformats" | "extras" | "docs" | "documentation"
            )
        })
    })
}

fn compact_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_oneload64_install_name(value: &str) -> bool {
    compact_name(value).starts_with("oneload64")
}

pub fn validate_prepared_launch_path(path: &Path) -> Result<bool, String> {
    let is_mgl = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mgl"));
    if is_mgl && path_has_component(path, "_DOS Games") {
        validate_0mhz_mgl(path)?;
        return Ok(true);
    }
    if is_mgl && path_has_neon68k_launcher_component(path) {
        validate_neon68k_mgl(path)?;
        return Ok(true);
    }
    let oneload64_install = path.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_oneload64_install_name)
    });
    if let Some(install_root) = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("crt"))
        .then_some(oneload64_install)
        .flatten()
    {
        if !path.is_file() {
            return Err(format!(
                "prepared C64 payload is missing: {}",
                path.display()
            ));
        }
        if !oneload64_root_has_signature(install_root) {
            return Err(format!(
                "OneLoad64 installation signature is missing: {}",
                install_root.display()
            ));
        }
        if oneload64_path_is_excluded(path, install_root) {
            return Err(format!(
                "prepared C64 payload is outside the primary OneLoad64 trees: {}",
                path.display()
            ));
        }
        return Ok(true);
    }
    Ok(false)
}

pub(crate) fn diagnostic_for_candidate(
    path: &Path,
    platform_id: &str,
) -> Option<PreparedLaunchDiagnostic> {
    let is_mgl = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mgl"));
    if is_mgl && platform_id == "dos" && path_has_component(path, "_DOS Games") {
        return validate_0mhz_mgl(path)
            .err()
            .map(|reason| PreparedLaunchDiagnostic {
                collection_id: PreparedCollectionId::ZeroMhz,
                status: "invalid",
                reason,
            });
    }
    if is_mgl && path_has_neon68k_launcher_component(path) {
        return validate_neon68k_mgl(path)
            .err()
            .map(|reason| PreparedLaunchDiagnostic {
                collection_id: PreparedCollectionId::Neon68k,
                status: "invalid",
                reason,
            });
    }
    let install_root = path.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_oneload64_install_name)
    })?;
    if oneload64_path_is_excluded(path, install_root) {
        return Some(PreparedLaunchDiagnostic {
            collection_id: PreparedCollectionId::OneLoad64,
            status: "excluded",
            reason: "non-primary OneLoad64 tree".to_string(),
        });
    }
    (!oneload64_root_has_signature(install_root)).then(|| PreparedLaunchDiagnostic {
        collection_id: PreparedCollectionId::OneLoad64,
        status: "invalid",
        reason: "OneLoad64 directory is missing its collection signature".to_string(),
    })
}

fn path_has_component(path: &Path, expected: &str) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|component| component.eq_ignore_ascii_case(expected))
    })
}

fn path_has_neon68k_launcher_component(path: &Path) -> bool {
    path.components().any(|component| {
        component.as_os_str().to_str().is_some_and(|value| {
            value.eq_ignore_ascii_case("_X68000 Games")
                || value.eq_ignore_ascii_case("X68000 Games")
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mister-magik-prepared-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        dir
    }

    #[test]
    fn prepared_provenance_uses_stable_storage_values() {
        let provenance = PreparedLaunchProvenance::prepared(PreparedCollectionId::ZeroMhz);
        assert_eq!(provenance.collection_id.as_str(), "0mhz");
        assert_eq!(provenance.launch_quality.as_str(), "prepared");
        assert_eq!(
            provenance.adapter_version,
            PREPARED_COLLECTION_ADAPTER_VERSION
        );
    }

    #[test]
    fn prepared_payload_index_validates_split_0mhz_layout_without_live_fallback() {
        let storage = fixture_dir("payload-index-split");
        let launchers = storage.join("_DOS Games");
        let payload = storage.join("games/AO486/media/doom/doom.vhd");
        std::fs::create_dir_all(&launchers).expect("create launcher root");
        std::fs::create_dir_all(payload.parent().expect("payload parent"))
            .expect("create payload root");
        std::fs::write(&payload, b"vhd").expect("write payload");
        let mgl = launchers.join("Doom.mgl");
        std::fs::write(
            &mgl,
            r#"<mistergamedescription><rbf>AO486</rbf><file path="media/doom/doom.vhd"/><reset/></mistergamedescription>"#,
        )
        .expect("write MGL");
        let roots = vec![
            launchers.display().to_string(),
            storage.join("games").display().to_string(),
        ];

        let index = PreparedPayloadIndex::from_library_roots(&roots);
        let inspection = inspect_mgl(&mgl).expect("inspect MGL");

        assert_eq!(index.complete_root_count(), 2);
        assert_eq!(
            index.resolve_0mhz_payload_path(&mgl, "media/doom/doom.vhd"),
            payload
        );
        validate_0mhz_mgl_inspection_with_index(&mgl, &inspection, &index)
            .expect("validate from index");
        let stats = index.lookup_stats();
        assert!(stats.files >= 3);
        assert_eq!(stats.unknown, 0);
        assert_eq!(stats.live_fallbacks, 0);
        let _ = std::fs::remove_dir_all(storage);
    }

    #[test]
    fn prepared_payload_index_proves_missing_payload_inside_complete_roots() {
        let storage = fixture_dir("payload-index-missing");
        let launchers = storage.join("_DOS Games");
        let payload_root = storage.join("games/AO486");
        std::fs::create_dir_all(&launchers).expect("create launcher root");
        std::fs::create_dir_all(&payload_root).expect("create payload root");
        let mgl = launchers.join("Missing.mgl");
        std::fs::write(
            &mgl,
            r#"<mistergamedescription><rbf>AO486</rbf><file path="media/missing.vhd"/><reset/></mistergamedescription>"#,
        )
        .expect("write MGL");
        let roots = vec![
            launchers.display().to_string(),
            payload_root.display().to_string(),
        ];
        let index = PreparedPayloadIndex::from_library_roots(&roots);
        let inspection = inspect_mgl(&mgl).expect("inspect MGL");

        let error = validate_0mhz_mgl_inspection_with_index(&mgl, &inspection, &index)
            .expect_err("missing payload must fail");

        assert!(error.contains(&format!(
            "{}",
            launchers.join("media/missing.vhd").display()
        )));
        let stats = index.lookup_stats();
        assert!(stats.missing >= 2);
        assert_eq!(stats.unknown, 0);
        assert_eq!(stats.live_fallbacks, 0);
        let _ = std::fs::remove_dir_all(storage);
    }

    #[test]
    fn prepared_payload_index_falls_back_for_paths_outside_captured_roots() {
        let storage = fixture_dir("payload-index-outside");
        let launchers = storage.join("_DOS Games");
        let payload_root = storage.join("games/AO486");
        let outside = storage.join("imports/External.vhd");
        std::fs::create_dir_all(&launchers).expect("create launcher root");
        std::fs::create_dir_all(&payload_root).expect("create payload root");
        std::fs::create_dir_all(outside.parent().expect("outside parent"))
            .expect("create outside root");
        std::fs::write(&outside, b"vhd").expect("write outside payload");
        let mgl = launchers.join("External.mgl");
        std::fs::write(
            &mgl,
            format!(
                r#"<mistergamedescription><rbf>AO486</rbf><file path="{}"/><reset/></mistergamedescription>"#,
                outside.display()
            ),
        )
        .expect("write MGL");
        let roots = vec![
            launchers.display().to_string(),
            payload_root.display().to_string(),
        ];
        let index = PreparedPayloadIndex::from_library_roots(&roots);
        let inspection = inspect_mgl(&mgl).expect("inspect MGL");

        validate_0mhz_mgl_inspection_with_index(&mgl, &inspection, &index)
            .expect("outside payload uses live fallback");

        let stats = index.lookup_stats();
        assert!(stats.unknown >= 2);
        assert!(stats.live_fallbacks >= 2);
        let _ = std::fs::remove_dir_all(storage);
    }

    #[test]
    fn zero_mhz_validation_accepts_vhd_and_multi_image_launchers() {
        let dir = fixture_dir("0mhz-valid");
        std::fs::write(dir.join("doom.vhd"), b"vhd").expect("write vhd");
        std::fs::write(dir.join("disc.chd"), b"chd").expect("write chd");
        let mgl = dir.join("Doom.mgl");
        std::fs::write(
            &mgl,
            r#"<mistergamedescription>
                <rbf>_Computer/AO486</rbf>
                <file delay="1" type="s" index="2" path="doom.vhd"/>
                <file delay="1" type="s" index="4">disc.chd</file>
                <reset delay="1"/>
            </mistergamedescription>"#,
        )
        .expect("write mgl");

        let inspection = validate_0mhz_mgl(&mgl).expect("validate 0MHz MGL");

        assert_eq!(inspection.files.len(), 2);
        assert_eq!(inspection.files[0].index, Some(2));
        assert_eq!(inspection.files[1].path, "disc.chd");
        assert_eq!(inspection.reset_count, 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn zero_mhz_validation_resolves_split_launcher_and_payload_roots() {
        let storage = fixture_dir("0mhz-split-root");
        let launchers = storage.join("_DOS Games");
        let payload = storage.join("games/AO486/media/doom/doom.vhd");
        std::fs::create_dir_all(&launchers).expect("create launcher root");
        std::fs::create_dir_all(payload.parent().expect("payload parent"))
            .expect("create payload root");
        std::fs::write(&payload, b"vhd").expect("write payload");
        let mgl = launchers.join("Doom.mgl");
        std::fs::write(
            &mgl,
            r#"<mistergamedescription>
                <rbf>_computer/ao486</rbf>
                <file type="s" index="2" path="media/doom/doom.vhd"/>
                <reset delay="1"/>
            </mistergamedescription>"#,
        )
        .expect("write mgl");

        let inspection = validate_0mhz_mgl(&mgl).expect("validate split 0MHz layout");

        assert_eq!(inspection.files.len(), 1);
        assert_eq!(
            resolve_0mhz_payload_path(&mgl, &inspection.files[0].path),
            payload
        );
        let _ = std::fs::remove_dir_all(storage);
    }

    #[test]
    fn zero_mhz_validation_rejects_wrong_core_missing_payload_and_reset() {
        let dir = fixture_dir("0mhz-invalid");
        let wrong_core = dir.join("wrong-core.mgl");
        std::fs::write(
            &wrong_core,
            r#"<mistergamedescription><rbf>Minimig</rbf><file path="game.vhd"/><reset/></mistergamedescription>"#,
        )
        .expect("write wrong core");
        assert!(
            validate_0mhz_mgl(&wrong_core)
                .expect_err("wrong core should fail")
                .contains("expected AO486")
        );

        let missing = dir.join("missing.mgl");
        std::fs::write(
            &missing,
            r#"<mistergamedescription><rbf>AO486</rbf><file path="missing.vhd"/><reset/></mistergamedescription>"#,
        )
        .expect("write missing payload");
        assert!(
            validate_0mhz_mgl(&missing)
                .expect_err("missing payload should fail")
                .contains("payload is missing")
        );

        std::fs::write(dir.join("game.vhd"), b"vhd").expect("write vhd");
        let no_reset = dir.join("no-reset.mgl");
        std::fs::write(
            &no_reset,
            r#"<mistergamedescription><rbf>AO486</rbf><file path="game.vhd"/></mistergamedescription>"#,
        )
        .expect("write no reset");
        assert!(
            validate_0mhz_mgl(&no_reset)
                .expect_err("missing reset should fail")
                .contains("no reset")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn neon68k_validation_requires_x68000_setname_and_hdf() {
        let dir = fixture_dir("neon68k-valid");
        let issue_dir = dir.join("Keyboard + Mouse");
        std::fs::create_dir_all(&issue_dir).expect("create issue dir");
        std::fs::write(issue_dir.join("game.hdf"), b"hdf").expect("write hdf");
        let mgl = issue_dir.join("Akumajou Dracula.mgl");
        std::fs::write(
            &mgl,
            r#"<mistergamedescription><rbf>_Computer/X68000</rbf><setname>Akumajou</setname><file index="0" path="game.hdf"/></mistergamedescription>"#,
        )
        .expect("write MGL");

        let inspection = validate_neon68k_mgl(&mgl).expect("validate Neon68K MGL");

        assert_eq!(inspection.setname.as_deref(), Some("Akumajou"));
        assert_eq!(
            neon68k_source_category(&mgl).as_deref(),
            Some("Keyboard + Mouse")
        );

        let missing_setname = dir.join("missing-setname.mgl");
        std::fs::write(
            &missing_setname,
            r#"<mistergamedescription><rbf>X68000</rbf><file path="Keyboard + Mouse/game.hdf"/></mistergamedescription>"#,
        )
        .expect("write missing setname MGL");
        assert!(
            validate_neon68k_mgl(&missing_setname)
                .expect_err("missing setname should fail")
                .contains("no setname")
        );

        let missing_hdf = dir.join("missing-hdf.mgl");
        std::fs::write(
            &missing_hdf,
            r#"<mistergamedescription><rbf>X68000</rbf><setname>Missing</setname><file path="missing.hdf"/></mistergamedescription>"#,
        )
        .expect("write missing HDF MGL");
        assert!(
            validate_neon68k_mgl(&missing_hdf)
                .expect_err("missing HDF should fail")
                .contains("payload is missing")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn neon68k_validation_resolves_current_split_layout() {
        let storage = fixture_dir("neon68k-split-layout");
        let launcher_dir = storage.join("_Computer/_X68000 Games/_Keyboard and Mouse");
        let payload = storage.join("games/X68000/media/After Burner 2/After Burner 2.hdf");
        std::fs::create_dir_all(&launcher_dir).expect("create launcher dir");
        std::fs::create_dir_all(payload.parent().expect("payload parent"))
            .expect("create payload dir");
        std::fs::write(&payload, b"hdf").expect("write payload");
        let mgl = launcher_dir.join("After Burner 2.mgl");
        std::fs::write(
            &mgl,
            r#"<mistergamedescription><rbf>_computer/X68000</rbf><file index="2" path="media/After Burner 2/After Burner 2.hdf"/><setname same_dir="1">X68K-After_Burner_2</setname><reset delay="1"/></mistergamedescription>"#,
        )
        .expect("write MGL");

        let inspection = validate_neon68k_mgl(&mgl).expect("validate current Neon68K layout");

        assert_eq!(
            resolve_neon68k_payload_path(&mgl, &inspection.files[0].path),
            payload
        );
        let _ = std::fs::remove_dir_all(storage);
    }

    #[test]
    fn oneload64_requires_install_signature_and_excludes_non_primary_trees() {
        let dir = fixture_dir("oneload64");
        let install = dir.join("OneLoad64 Games Collection v4");
        let multi = install.join("MultiLoad64");
        let dumps = install.join("Dumps");
        let alternatives = install.join("AlternativeFormats");
        let extras = install.join("Extras");
        for path in [&multi, &dumps, &alternatives, &extras] {
            std::fs::create_dir_all(path).expect("create collection dir");
        }
        let primary = install.join("Impossible Mission.crt");
        let multiload = multi.join("Summer Games.crt");
        let dump = dumps.join("Dump.crt");
        let alternative = alternatives.join("Alternative.crt");
        let extra = extras.join("Extra.crt");
        for path in [&primary, &multiload, &dump, &alternative, &extra] {
            std::fs::write(path, b"crt").expect("write CRT");
        }

        assert!(oneload64_provenance(&primary).is_some());
        assert!(oneload64_provenance(&multiload).is_some());
        assert!(oneload64_provenance(&dump).is_none());
        assert!(oneload64_provenance(&alternative).is_none());
        assert!(oneload64_provenance(&extra).is_none());
        assert_eq!(validate_prepared_launch_path(&primary), Ok(true));
        assert!(
            validate_prepared_launch_path(&dump)
                .expect_err("excluded prepared path should fail")
                .contains("outside the primary")
        );

        let unmarked = dir.join("General C64 CRTs/Game.crt");
        std::fs::create_dir_all(unmarked.parent().expect("unmarked parent"))
            .expect("create unmarked dir");
        std::fs::write(&unmarked, b"crt").expect("write unmarked CRT");
        assert!(oneload64_provenance(&unmarked).is_none());
        assert_eq!(validate_prepared_launch_path(&unmarked), Ok(false));
        let _ = std::fs::remove_dir_all(dir);
    }
}
