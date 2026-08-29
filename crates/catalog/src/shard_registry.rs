// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Dual-slot registry and immutable generation-qualified artifact publication.

use crate::catalog_classify::SystemId;
use crate::catalog_domain::ScanUnitId;
use crate::catalog_format::CatalogFormatDescriptor;
use crate::sharded_catalog::MANIFEST_SCHEMA_VERSION;
use crate::system_shard::SystemShardLimits;
#[cfg(feature = "builder")]
use crate::system_shard::open_system_shard;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
#[cfg(feature = "builder")]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::Read;
#[cfg(feature = "builder")]
use std::io::{Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
#[cfg(feature = "builder")]
use std::time::{Duration, Instant};

const MANIFEST_A: &str = "registry/manifest-a.json";
const MANIFEST_B: &str = "registry/manifest-b.json";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegistryLimits {
    pub max_manifest_bytes: usize,
    pub max_systems: usize,
    pub shard: SystemShardLimits,
}

pub fn production_registry_limits() -> RegistryLimits {
    RegistryLimits {
        max_manifest_bytes: 8 * 1024 * 1024,
        max_systems: 4096,
        shard: SystemShardLimits {
            max_sqlite_bytes: 8 * 1024 * 1024 * 1024,
            max_navigation_compressed_bytes: 512 * 1024 * 1024,
            max_navigation_decoded_bytes: 512 * 1024 * 1024,
            max_games: 2_000_000,
        },
    }
}

#[cfg(feature = "builder")]
pub fn cleanup_registry_temporary_files(storage_root: &Path) -> Result<usize, RegistryError> {
    let mut removed = 0usize;
    for scoped_root in [storage_root.join("registry"), storage_root.join("systems")] {
        if !scoped_root.exists() {
            continue;
        }
        let mut changed_directories = BTreeSet::new();
        for entry in walkdir::WalkDir::new(&scoped_root)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            if !name.starts_with('.') || !name.contains(".tmp.") {
                continue;
            }
            fs::remove_file(entry.path())
                .map_err(|error| RegistryError::with("remove stale registry temporary", error))?;
            removed = removed.saturating_add(1);
            if let Some(parent) = entry.path().parent() {
                changed_directories.insert(parent.to_path_buf());
            }
        }
        for directory in changed_directories {
            sync_directory(&directory)?;
        }
    }
    Ok(removed)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogManifest {
    pub format: Option<CatalogFormatDescriptor>,
    pub generation: u64,
    pub systems: Vec<ManifestSystem>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ManifestSystem {
    pub system_id: SystemId,
    pub display_title: String,
    pub section: String,
    pub family: String,
    pub order: u32,
    pub producers: Vec<ScanUnitId>,
    pub active: PublishedGeneration,
    pub previous: Option<PublishedGeneration>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PublishedGeneration {
    pub generation: u64,
    pub sqlite_path: PathBuf,
    pub navigation_path: PathBuf,
    pub sqlite_bytes: u64,
    pub navigation_bytes: u64,
    pub sqlite_hash: String,
    pub navigation_hash: String,
    pub games: u64,
    pub navpack: Option<PublishedNavPack>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PublishedNavPack {
    pub path: PathBuf,
    pub bytes: u64,
    pub hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoredManifest {
    schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    format: Option<CatalogFormatDescriptor>,
    generation: u64,
    systems: Vec<StoredSystem>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoredSystem {
    system_id: String,
    display_title: String,
    section: String,
    family: String,
    order: u32,
    producers: Vec<String>,
    active: StoredGeneration,
    previous: Option<StoredGeneration>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoredGeneration {
    generation: u64,
    sqlite_path: String,
    navigation_path: String,
    sqlite_bytes: u64,
    navigation_bytes: u64,
    sqlite_hash: String,
    navigation_hash: String,
    games: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    navpack: Option<StoredNavPack>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoredNavPack {
    path: String,
    bytes: u64,
    hash: String,
}

#[cfg(feature = "builder")]
pub(crate) fn publish_system_artifacts(
    storage_root: &Path,
    staged_sqlite: &Path,
    staged_navigation: &Path,
    system_id: &SystemId,
    generation: u64,
    games: u64,
    limits: RegistryLimits,
) -> Result<PublishedGeneration, RegistryError> {
    publish_system_artifacts_with_options(
        storage_root,
        staged_sqlite,
        staged_navigation,
        system_id,
        generation,
        games,
        limits,
        true,
        true,
        false,
    )
    .map(|published| published.generation)
}

#[cfg(feature = "builder")]
pub(crate) struct ArtifactPublication {
    pub(crate) generation: PublishedGeneration,
    pub(crate) copy_hash_time: Duration,
    pub(crate) copied_bytes: u64,
}

#[cfg(feature = "builder")]
#[allow(clippy::too_many_arguments)]
fn publish_system_artifacts_with_options(
    storage_root: &Path,
    staged_sqlite: &Path,
    staged_navigation: &Path,
    system_id: &SystemId,
    generation: u64,
    games: u64,
    limits: RegistryLimits,
    validate_staged: bool,
    sync_target_directory: bool,
    copy_staged: bool,
) -> Result<ArtifactPublication, RegistryError> {
    let staged_navpack = crate::system_shard::navpack_path_for_navigation(staged_navigation);
    if !copy_staged {
        ensure_staging_path(storage_root, staged_sqlite)?;
        ensure_staging_path(storage_root, staged_navigation)?;
        ensure_staging_path(storage_root, &staged_navpack)?;
    }
    if validate_staged {
        open_system_shard(
            staged_sqlite,
            staged_navigation,
            system_id,
            generation,
            limits.shard,
        )
        .map_err(|error| RegistryError::new("validate-staged", error.to_string()))?;
        let navpack = fs::read(&staged_navpack)
            .map_err(|error| RegistryError::with("read staged NavPack", error))?;
        crate::navpack::validate(
            &navpack,
            system_id.as_str(),
            generation,
            usize::try_from(games)
                .map_err(|_| RegistryError::new("validate-staged", "game count too large"))?,
        )
        .map_err(|error| RegistryError::new("validate-staged", error))?;
    }
    let relative_directory = PathBuf::from("systems").join(system_id.as_str());
    let sqlite_path = relative_directory.join(format!("{generation}.sqlite3"));
    let navigation_path = relative_directory.join(format!("{generation}.nav.lz4b"));
    let navpack_path = relative_directory.join(format!("{generation}.navpack"));
    let target_sqlite = storage_root.join(&sqlite_path);
    let target_navigation = storage_root.join(&navigation_path);
    let target_navpack = storage_root.join(&navpack_path);
    if target_sqlite.exists() || target_navigation.exists() || target_navpack.exists() {
        return Err(RegistryError::new(
            "publish-artifact",
            "immutable generation artifact already exists",
        ));
    }
    let sqlite_bytes = regular_file_size(staged_sqlite, limits.shard.max_sqlite_bytes)?;
    let navigation_bytes = regular_file_size(
        staged_navigation,
        limits.shard.max_navigation_compressed_bytes as u64,
    )?;
    let navpack_bytes = regular_file_size(&staged_navpack, limits.shard.max_sqlite_bytes)?;
    let target_directory = target_sqlite
        .parent()
        .ok_or_else(|| RegistryError::new("publish-artifact", "target has no parent"))?;
    fs::create_dir_all(target_directory)
        .map_err(|error| RegistryError::with("create system generation directory", error))?;
    let (sqlite_hash, navigation_hash, navpack_hash, copy_hash_time, copied_bytes) = if copy_staged
    {
        let copied = (|| {
            let sqlite =
                copy_staged_artifact(staged_sqlite, &target_sqlite, "SQLite", sqlite_bytes)?;
            let navigation = copy_staged_artifact(
                staged_navigation,
                &target_navigation,
                "navigation",
                navigation_bytes,
            )?;
            let navpack =
                copy_staged_artifact(&staged_navpack, &target_navpack, "NavPack", navpack_bytes)?;
            Ok((
                sqlite.hash,
                navigation.hash,
                navpack.hash,
                sqlite.copy_hash_time + navigation.copy_hash_time + navpack.copy_hash_time,
                sqlite
                    .bytes
                    .saturating_add(navigation.bytes)
                    .saturating_add(navpack.bytes),
            ))
        })();
        match copied {
            Ok(hashes) => hashes,
            Err(error) => {
                let _ = fs::remove_file(&target_sqlite);
                let _ = fs::remove_file(&target_navigation);
                let _ = fs::remove_file(&target_navpack);
                return Err(error);
            }
        }
    } else {
        let hash_started = Instant::now();
        let sqlite_hash = file_checksum(staged_sqlite)?;
        let navigation_hash = file_checksum(staged_navigation)?;
        let navpack_hash = file_checksum(&staged_navpack)?;
        let hash_time = hash_started.elapsed();
        fs::rename(staged_sqlite, &target_sqlite)
            .map_err(|error| RegistryError::with("publish immutable SQLite", error))?;
        fs::rename(staged_navigation, &target_navigation)
            .map_err(|error| RegistryError::with("publish immutable navigation", error))?;
        fs::rename(&staged_navpack, &target_navpack)
            .map_err(|error| RegistryError::with("publish immutable NavPack", error))?;
        (sqlite_hash, navigation_hash, navpack_hash, hash_time, 0)
    };
    if sync_target_directory {
        sync_directory(target_directory)?;
    }
    Ok(ArtifactPublication {
        generation: PublishedGeneration {
            generation,
            sqlite_path,
            navigation_path,
            sqlite_bytes,
            navigation_bytes,
            sqlite_hash,
            navigation_hash,
            games,
            navpack: Some(PublishedNavPack {
                path: navpack_path,
                bytes: navpack_bytes,
                hash: navpack_hash,
            }),
        },
        copy_hash_time,
        copied_bytes,
    })
}

#[cfg(feature = "builder")]
pub(crate) fn publish_prevalidated_system_artifacts_deferred(
    storage_root: &Path,
    staged_sqlite: &Path,
    staged_navigation: &Path,
    system_id: &SystemId,
    generation: u64,
    games: u64,
    limits: RegistryLimits,
) -> Result<ArtifactPublication, RegistryError> {
    let on_media_staging = storage_root.join("staging");
    let copy_staged = !staged_sqlite.starts_with(&on_media_staging)
        || !staged_navigation.starts_with(&on_media_staging);
    publish_system_artifacts_with_options(
        storage_root,
        staged_sqlite,
        staged_navigation,
        system_id,
        generation,
        games,
        limits,
        false,
        false,
        copy_staged,
    )
}

#[cfg(feature = "builder")]
fn copy_staged_artifact(
    source: &Path,
    target: &Path,
    label: &'static str,
    expected: u64,
) -> Result<CopiedArtifact, RegistryError> {
    let mode =
        artifact_copy_mode_from_value(std::env::var_os("MISTER_CATALOG_ARTIFACT_COPY").as_deref())
            .map_err(|value| {
                RegistryError::new(
                    "publish-artifact",
                    format!("invalid artifact copy mode {value}"),
                )
            })?;
    let temporary = target.with_file_name(format!(
        ".{}.tmp.{}-{}",
        target
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("artifact"),
        std::process::id(),
        crate::catalog_lease::CatalogRunId::new().as_str(),
    ));
    let mut cleanup = TemporaryCleanup(Some(temporary.clone()));
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(RegistryError::with("remove stale artifact copy", error)),
    }
    let mut input =
        File::open(source).map_err(|error| RegistryError::with("open staged artifact", error))?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| RegistryError::with("create artifact copy", error))?;
    if mode == ArtifactCopyMode::Preallocated {
        output
            .set_len(expected)
            .map_err(|error| RegistryError::with("preallocate artifact copy", error))?;
        output
            .seek(SeekFrom::Start(0))
            .map_err(|error| RegistryError::with("rewind preallocated artifact", error))?;
    }
    let mut copied = 0_u64;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut buffer = [0_u8; 256 * 1024];
    let copy_hash_started = Instant::now();
    if mode == ArtifactCopyMode::CopyFileRange {
        copied = copy_file_range_all(&input, &output, expected)?;
        hash = u64::from_str_radix(&file_checksum(&temporary)?, 16)
            .map_err(|error| RegistryError::new("publish-artifact", error.to_string()))?;
    } else {
        loop {
            crate::cooperative_work::checkpoint();
            let read = input
                .read(&mut buffer)
                .map_err(|error| RegistryError::with("read staged artifact", error))?;
            if read == 0 {
                break;
            }
            output
                .write_all(&buffer[..read])
                .map_err(|error| RegistryError::with("copy staged artifact", error))?;
            copied = copied.saturating_add(read as u64);
            update_checksum(&mut hash, &buffer[..read]);
        }
    }
    if copied != expected {
        return Err(RegistryError::new(
            "publish-artifact",
            format!("copied {label} size does not match staged artifact"),
        ));
    }
    let copy_hash_time = copy_hash_started.elapsed();
    crate::catalog_logln!(
        "catalog_artifact_copy_tsv\tlabel={}\tmode={}\tbytes={}\telapsed_us={}",
        label,
        mode.label(),
        copied,
        copy_hash_time.as_micros(),
    );
    drop(output);
    fs::rename(&temporary, target)
        .map_err(|error| RegistryError::with("commit copied artifact", error))?;
    cleanup.0 = None;
    Ok(CopiedArtifact {
        bytes: copied,
        hash: format!("{hash:016x}"),
        copy_hash_time,
    })
}

#[cfg(feature = "builder")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ArtifactCopyMode {
    #[default]
    Buffered,
    Preallocated,
    CopyFileRange,
}

#[cfg(feature = "builder")]
impl ArtifactCopyMode {
    fn label(self) -> &'static str {
        match self {
            Self::Buffered => "buffered",
            Self::Preallocated => "preallocated",
            Self::CopyFileRange => "copy-file-range",
        }
    }
}

#[cfg(feature = "builder")]
fn artifact_copy_mode_from_value(
    value: Option<&std::ffi::OsStr>,
) -> Result<ArtifactCopyMode, String> {
    match value.and_then(std::ffi::OsStr::to_str) {
        None | Some("") | Some("buffered") => Ok(ArtifactCopyMode::Buffered),
        Some("preallocated") => Ok(ArtifactCopyMode::Preallocated),
        Some("copy-file-range") => Ok(ArtifactCopyMode::CopyFileRange),
        Some(value) => Err(value.to_string()),
    }
}

#[cfg(all(feature = "builder", target_os = "linux"))]
fn copy_file_range_all(input: &File, output: &File, expected: u64) -> Result<u64, RegistryError> {
    use std::os::fd::AsRawFd;

    let mut copied = 0u64;
    while copied < expected {
        crate::cooperative_work::checkpoint();
        let remaining = expected.saturating_sub(copied).min(256 * 1024) as usize;
        let written = unsafe {
            libc::copy_file_range(
                input.as_raw_fd(),
                std::ptr::null_mut(),
                output.as_raw_fd(),
                std::ptr::null_mut(),
                remaining,
                0,
            )
        };
        if written < 0 {
            return Err(RegistryError::with(
                "copy artifact with copy_file_range",
                std::io::Error::last_os_error(),
            ));
        }
        if written == 0 {
            break;
        }
        copied = copied.saturating_add(written as u64);
    }
    Ok(copied)
}

#[cfg(all(feature = "builder", not(target_os = "linux")))]
fn copy_file_range_all(
    _input: &File,
    _output: &File,
    _expected: u64,
) -> Result<u64, RegistryError> {
    Err(RegistryError::new(
        "copy artifact with copy_file_range",
        "copy_file_range is only available on Linux",
    ))
}

#[cfg(feature = "builder")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct CopiedArtifact {
    bytes: u64,
    hash: String,
    copy_hash_time: Duration,
}

#[cfg(all(feature = "builder", target_os = "linux"))]
pub(crate) fn sync_artifact_batch(storage_root: &Path) -> Result<(), RegistryError> {
    use std::os::fd::AsRawFd;
    let directory = File::open(storage_root)
        .map_err(|error| RegistryError::with("open artifact filesystem", error))?;
    let result = unsafe { libc::syncfs(directory.as_raw_fd()) };
    if result == 0 {
        Ok(())
    } else {
        Err(RegistryError::with(
            "sync artifact filesystem",
            std::io::Error::last_os_error(),
        ))
    }
}

#[cfg(all(feature = "builder", not(target_os = "linux")))]
pub(crate) fn sync_artifact_batch(storage_root: &Path) -> Result<(), RegistryError> {
    fn sync_tree(path: &Path) -> Result<(), RegistryError> {
        for entry in fs::read_dir(path)
            .map_err(|error| RegistryError::with("read artifact directory", error))?
        {
            let entry = entry.map_err(|error| RegistryError::with("read artifact entry", error))?;
            let file_type = entry
                .file_type()
                .map_err(|error| RegistryError::with("inspect artifact entry", error))?;
            if file_type.is_dir() {
                sync_tree(&entry.path())?;
                sync_directory(&entry.path())?;
            } else if file_type.is_file() {
                File::open(entry.path())
                    .and_then(|file| file.sync_all())
                    .map_err(|error| RegistryError::with("sync artifact file", error))?;
            }
        }
        Ok(())
    }
    sync_tree(storage_root)?;
    sync_directory(storage_root)
}

#[cfg(feature = "builder")]
pub(crate) fn publish_manifest(
    storage_root: &Path,
    manifest: &CatalogManifest,
    limits: RegistryLimits,
) -> Result<PathBuf, RegistryError> {
    publish_manifest_with_hash_policy(storage_root, manifest, limits, true)
}

/// Publish a manifest after the artifact writer has supplied hashes while
/// copying immutable files. Sizes, paths, and schema are still checked; the
/// already-computed hashes are trusted so publication does not reread every
/// artifact from exFAT.
#[cfg(feature = "builder")]
pub(crate) fn publish_manifest_with_trusted_artifacts(
    storage_root: &Path,
    manifest: &CatalogManifest,
    limits: RegistryLimits,
) -> Result<PathBuf, RegistryError> {
    publish_manifest_with_hash_policy(storage_root, manifest, limits, false)
}

#[cfg(feature = "builder")]
fn publish_manifest_with_hash_policy(
    storage_root: &Path,
    manifest: &CatalogManifest,
    limits: RegistryLimits,
    verify_hashes: bool,
) -> Result<PathBuf, RegistryError> {
    validate_manifest(storage_root, manifest, limits, true, verify_hashes)?;
    let current = read_manifest_slots(storage_root, limits, verify_hashes)?;
    if current
        .iter()
        .any(|(_, existing)| existing.generation >= manifest.generation)
    {
        return Err(RegistryError::new(
            "publish-manifest",
            "manifest generation is not newer than committed state",
        ));
    }
    let target_relative = match current.as_slice() {
        [] => PathBuf::from(MANIFEST_A),
        [(path, _)] if path == Path::new(MANIFEST_A) => PathBuf::from(MANIFEST_B),
        [(path, _)] if path == Path::new(MANIFEST_B) => PathBuf::from(MANIFEST_A),
        [first, second] => {
            if first.1.generation <= second.1.generation {
                first.0.clone()
            } else {
                second.0.clone()
            }
        }
        _ => return Err(RegistryError::new("publish-manifest", "invalid slot state")),
    };
    let stored = to_stored(manifest);
    let bytes = serde_json::to_vec_pretty(&stored)
        .map_err(|error| RegistryError::with("encode manifest", error))?;
    if bytes.len() > limits.max_manifest_bytes {
        return Err(RegistryError::new(
            "publish-manifest",
            "manifest exceeds configured size limit",
        ));
    }
    let target = storage_root.join(&target_relative);
    let directory = target
        .parent()
        .ok_or_else(|| RegistryError::new("publish-manifest", "slot has no parent"))?;
    fs::create_dir_all(directory)
        .map_err(|error| RegistryError::with("create registry directory", error))?;
    let temporary = directory.join(format!(
        ".{}.tmp.{}-{}",
        target
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("manifest"),
        std::process::id(),
        crate::catalog_lease::CatalogRunId::new().as_str(),
    ));
    let mut cleanup = TemporaryCleanup(Some(temporary.clone()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| RegistryError::with("create temporary manifest", error))?;
    file.write_all(&bytes)
        .map_err(|error| RegistryError::with("write temporary manifest", error))?;
    file.sync_all()
        .map_err(|error| RegistryError::with("sync temporary manifest", error))?;
    drop(file);
    fs::rename(&temporary, &target)
        .map_err(|error| RegistryError::with("commit manifest slot", error))?;
    cleanup.0 = None;
    sync_directory(directory)?;
    Ok(target_relative)
}

pub fn read_latest_manifest(
    storage_root: &Path,
    limits: RegistryLimits,
) -> Result<CatalogManifest, RegistryError> {
    read_manifest_slots(storage_root, limits, true)?
        .into_iter()
        .max_by_key(|(_, manifest)| manifest.generation)
        .map(|(_, manifest)| manifest)
        .ok_or_else(|| RegistryError::new("read-manifest", "no valid manifest slot"))
}

/// Read only the bounded registry metadata. System artifacts are deliberately
/// untouched so launcher shell startup does not scale with installed systems.
pub fn read_latest_manifest_lazy(
    storage_root: &Path,
    limits: RegistryLimits,
) -> Result<CatalogManifest, RegistryError> {
    read_manifest_slots(storage_root, limits, false)?
        .into_iter()
        .max_by_key(|(_, manifest)| manifest.generation)
        .map(|(_, manifest)| manifest)
        .ok_or_else(|| RegistryError::new("read-manifest", "no valid manifest slot"))
}

pub fn manifest_slots_present(storage_root: &Path) -> bool {
    [MANIFEST_A, MANIFEST_B]
        .iter()
        .any(|relative| fs::symlink_metadata(storage_root.join(relative)).is_ok())
}

fn read_manifest_slots(
    storage_root: &Path,
    limits: RegistryLimits,
    validate_artifacts: bool,
) -> Result<Vec<(PathBuf, CatalogManifest)>, RegistryError> {
    let mut manifests = Vec::new();
    for relative in [PathBuf::from(MANIFEST_A), PathBuf::from(MANIFEST_B)] {
        let path = storage_root.join(&relative);
        if !path.exists() {
            continue;
        }
        let result: Result<CatalogManifest, RegistryError> = (|| {
            let bytes = read_regular_bounded(&path, limits.max_manifest_bytes)?;
            let stored: StoredManifest = serde_json::from_slice(&bytes)
                .map_err(|error| RegistryError::with("parse manifest", error))?;
            let manifest = from_stored(stored)?;
            validate_manifest(storage_root, &manifest, limits, validate_artifacts, false)?;
            Ok(manifest)
        })();
        if let Ok(manifest) = result {
            manifests.push((relative, manifest));
        }
    }
    Ok(manifests)
}

fn validate_manifest(
    storage_root: &Path,
    manifest: &CatalogManifest,
    limits: RegistryLimits,
    validate_artifacts: bool,
    verify_hashes: bool,
) -> Result<(), RegistryError> {
    if manifest.systems.len() > limits.max_systems {
        return Err(RegistryError::new(
            "validate-manifest",
            "manifest system count exceeds configured limit",
        ));
    }
    let mut previous_id: Option<&SystemId> = None;
    for system in &manifest.systems {
        if previous_id.is_some_and(|id| id >= &system.system_id) {
            return Err(RegistryError::new(
                "validate-manifest",
                "manifest system IDs are duplicate or unsorted",
            ));
        }
        previous_id = Some(&system.system_id);
        validate_manifest_system_with_options(
            storage_root,
            system,
            limits,
            validate_artifacts,
            verify_hashes,
        )?;
    }
    Ok(())
}

fn validate_manifest_system_with_options(
    storage_root: &Path,
    system: &ManifestSystem,
    limits: RegistryLimits,
    validate_artifacts: bool,
    verify_hashes: bool,
) -> Result<(), RegistryError> {
    if system.display_title.is_empty()
        || system.section.is_empty()
        || system.family.is_empty()
        || system.producers.is_empty()
    {
        return Err(RegistryError::new(
            "validate-manifest",
            "manifest system metadata is incomplete",
        ));
    }
    let unique_producers = system.producers.iter().collect::<BTreeSet<_>>();
    if unique_producers.len() != system.producers.len() {
        return Err(RegistryError::new(
            "validate-manifest",
            "manifest contains duplicate producers",
        ));
    }
    validate_generation(
        storage_root,
        &system.system_id,
        &system.active,
        limits,
        validate_artifacts,
        verify_hashes,
    )?;
    if let Some(previous) = &system.previous {
        if previous.generation >= system.active.generation {
            return Err(RegistryError::new(
                "validate-manifest",
                "previous generation is not older than active generation",
            ));
        }
        validate_generation(
            storage_root,
            &system.system_id,
            previous,
            limits,
            validate_artifacts,
            verify_hashes,
        )?;
    }
    Ok(())
}

/// Fully validates one unpublished system entry before it is retained or
/// adopted into a manifest. Unlike normal manifest reads, this verifies the
/// stored hashes and reopens each shard to validate its schema and navigation.
#[cfg(test)]
pub(crate) fn validate_published_system(
    storage_root: &Path,
    system: &ManifestSystem,
    limits: RegistryLimits,
) -> Result<(), RegistryError> {
    validate_manifest_system_with_options(storage_root, system, limits, true, true)?;
    open_system_shard(
        &storage_root.join(&system.active.sqlite_path),
        &storage_root.join(&system.active.navigation_path),
        &system.system_id,
        system.active.generation,
        limits.shard,
    )
    .map_err(|error| RegistryError::new("validate-manifest", error.to_string()))?;
    if let Some(navpack) = &system.active.navpack {
        let bytes = fs::read(storage_root.join(&navpack.path))
            .map_err(|error| RegistryError::with("read NavPack", error))?;
        crate::navpack::validate(
            &bytes,
            system.system_id.as_str(),
            system.active.generation,
            usize::try_from(system.active.games)
                .map_err(|_| RegistryError::new("validate-manifest", "game count too large"))?,
        )
        .map_err(|error| RegistryError::new("validate-manifest", error))?;
    }
    if let Some(previous) = &system.previous {
        crate::system_shard::open_verified_system_navigation_with_timing(
            &storage_root.join(&previous.navigation_path),
            &system.system_id,
            previous.generation,
            &previous.navigation_hash,
            limits.shard,
        )
        .map_err(|error| RegistryError::new("validate-manifest", error.to_string()))?;
    }
    Ok(())
}

fn validate_generation(
    storage_root: &Path,
    system_id: &SystemId,
    generation: &PublishedGeneration,
    limits: RegistryLimits,
    validate_artifacts: bool,
    verify_hashes: bool,
) -> Result<(), RegistryError> {
    let expected_directory = PathBuf::from("systems").join(system_id.as_str());
    let expected_sqlite = expected_directory.join(format!("{}.sqlite3", generation.generation));
    let expected_navigation =
        expected_directory.join(format!("{}.nav.lz4b", generation.generation));
    let expected_navpack = expected_directory.join(format!("{}.navpack", generation.generation));
    if generation.sqlite_path != expected_sqlite
        || generation.navigation_path != expected_navigation
        || !safe_relative_path(&generation.sqlite_path)
        || !safe_relative_path(&generation.navigation_path)
        || generation.navpack.as_ref().is_some_and(|navpack| {
            navpack.path != expected_navpack
                || !safe_relative_path(&navpack.path)
                || navpack.bytes > limits.shard.max_sqlite_bytes
                || !valid_hash(&navpack.hash)
        })
    {
        return Err(RegistryError::new(
            "validate-manifest",
            "manifest artifact path is not canonical",
        ));
    }
    if generation.games > limits.shard.max_games as u64
        || generation.sqlite_bytes > limits.shard.max_sqlite_bytes
        || generation.navigation_bytes > limits.shard.max_navigation_compressed_bytes as u64
        || !valid_hash(&generation.sqlite_hash)
        || !valid_hash(&generation.navigation_hash)
    {
        return Err(RegistryError::new(
            "validate-manifest",
            "manifest generation metadata exceeds limits",
        ));
    }
    if !validate_artifacts {
        return Ok(());
    }
    let sqlite = storage_root.join(&generation.sqlite_path);
    let navigation = storage_root.join(&generation.navigation_path);
    let navpack = generation
        .navpack
        .as_ref()
        .map(|published| (storage_root.join(&published.path), published));
    if regular_file_size(&sqlite, limits.shard.max_sqlite_bytes)? != generation.sqlite_bytes
        || regular_file_size(
            &navigation,
            limits.shard.max_navigation_compressed_bytes as u64,
        )? != generation.navigation_bytes
    {
        return Err(RegistryError::new(
            "validate-manifest",
            "manifest artifact size does not match file",
        ));
    }
    if let Some((path, published)) = &navpack
        && regular_file_size(path, limits.shard.max_sqlite_bytes)? != published.bytes
    {
        return Err(RegistryError::new(
            "validate-manifest",
            "manifest NavPack size does not match file",
        ));
    }
    if verify_hashes
        && (file_checksum(&sqlite)? != generation.sqlite_hash
            || file_checksum(&navigation)? != generation.navigation_hash)
    {
        return Err(RegistryError::new(
            "validate-manifest",
            "manifest artifact hash does not match file",
        ));
    }
    if verify_hashes
        && let Some((path, published)) = &navpack
        && file_checksum(path)? != published.hash
    {
        return Err(RegistryError::new(
            "validate-manifest",
            "manifest NavPack hash does not match file",
        ));
    }
    Ok(())
}

#[cfg(feature = "builder")]
fn to_stored(manifest: &CatalogManifest) -> StoredManifest {
    StoredManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        format: manifest.format.clone(),
        generation: manifest.generation,
        systems: manifest
            .systems
            .iter()
            .map(|system| StoredSystem {
                system_id: system.system_id.as_str().to_string(),
                display_title: system.display_title.clone(),
                section: system.section.clone(),
                family: system.family.clone(),
                order: system.order,
                producers: system
                    .producers
                    .iter()
                    .map(|producer| producer.as_str().to_string())
                    .collect(),
                active: generation_to_stored(&system.active),
                previous: system.previous.as_ref().map(generation_to_stored),
            })
            .collect(),
    }
}

#[cfg(feature = "builder")]
fn generation_to_stored(generation: &PublishedGeneration) -> StoredGeneration {
    StoredGeneration {
        generation: generation.generation,
        sqlite_path: path_string(&generation.sqlite_path),
        navigation_path: path_string(&generation.navigation_path),
        sqlite_bytes: generation.sqlite_bytes,
        navigation_bytes: generation.navigation_bytes,
        sqlite_hash: generation.sqlite_hash.clone(),
        navigation_hash: generation.navigation_hash.clone(),
        games: generation.games,
        navpack: generation.navpack.as_ref().map(|navpack| StoredNavPack {
            path: path_string(&navpack.path),
            bytes: navpack.bytes,
            hash: navpack.hash.clone(),
        }),
    }
}

fn from_stored(stored: StoredManifest) -> Result<CatalogManifest, RegistryError> {
    if stored.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(RegistryError::new(
            "read-manifest",
            "unsupported manifest schema version",
        ));
    }
    Ok(CatalogManifest {
        format: stored.format,
        generation: stored.generation,
        systems: stored
            .systems
            .into_iter()
            .map(|system| {
                Ok(ManifestSystem {
                    system_id: SystemId::parse(&system.system_id)
                        .map_err(|error| RegistryError::new("read-manifest", error.to_string()))?,
                    display_title: system.display_title,
                    section: system.section,
                    family: system.family,
                    order: system.order,
                    producers: system
                        .producers
                        .into_iter()
                        .map(|value| {
                            ScanUnitId::parse(&value).map_err(|error| {
                                RegistryError::new("read-manifest", error.to_string())
                            })
                        })
                        .collect::<Result<_, _>>()?,
                    active: generation_from_stored(system.active),
                    previous: system.previous.map(generation_from_stored),
                })
            })
            .collect::<Result<_, RegistryError>>()?,
    })
}

fn generation_from_stored(generation: StoredGeneration) -> PublishedGeneration {
    PublishedGeneration {
        generation: generation.generation,
        sqlite_path: PathBuf::from(generation.sqlite_path),
        navigation_path: PathBuf::from(generation.navigation_path),
        sqlite_bytes: generation.sqlite_bytes,
        navigation_bytes: generation.navigation_bytes,
        sqlite_hash: generation.sqlite_hash,
        navigation_hash: generation.navigation_hash,
        games: generation.games,
        navpack: generation.navpack.map(|navpack| PublishedNavPack {
            path: PathBuf::from(navpack.path),
            bytes: navpack.bytes,
            hash: navpack.hash,
        }),
    }
}

#[cfg(feature = "builder")]
pub(crate) fn garbage_collect_unreferenced(
    storage_root: &Path,
    manifest: &CatalogManifest,
) -> Result<Vec<PathBuf>, RegistryError> {
    garbage_collect_unreferenced_with_retained(storage_root, manifest, std::iter::empty())
}

#[cfg(feature = "builder")]
pub(crate) fn garbage_collect_unreferenced_with_retained(
    storage_root: &Path,
    manifest: &CatalogManifest,
    additional_retained: impl IntoIterator<Item = PathBuf>,
) -> Result<Vec<PathBuf>, RegistryError> {
    let mut retained = manifest
        .systems
        .iter()
        .flat_map(|system| {
            std::iter::once(&system.active)
                .chain(system.previous.iter())
                .flat_map(|generation| {
                    let mut paths = vec![
                        generation.sqlite_path.clone(),
                        generation.navigation_path.clone(),
                    ];
                    paths.extend(
                        generation
                            .navpack
                            .iter()
                            .map(|navpack| navpack.path.clone()),
                    );
                    paths
                })
        })
        .collect::<BTreeSet<_>>();
    retained.extend(additional_retained);
    let mut removed = Vec::new();
    let systems_directory = storage_root.join("systems");
    let Ok(system_directories) = fs::read_dir(&systems_directory) else {
        return Ok(removed);
    };
    for system_directory in system_directories {
        let system_directory = system_directory
            .map_err(|error| RegistryError::with("read systems directory", error))?;
        let file_type = system_directory
            .file_type()
            .map_err(|error| RegistryError::with("read system directory type", error))?;
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let Some(system_name) = system_directory.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Ok(system_id) = SystemId::parse(&system_name) else {
            continue;
        };
        if system_id.as_str() != system_name {
            continue;
        }
        let relative_directory = PathBuf::from("systems").join(system_id.as_str());
        let entries = fs::read_dir(system_directory.path())
            .map_err(|error| RegistryError::with("read system directory", error))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| RegistryError::with("read system directory", error))?;
            let file_type = entry
                .file_type()
                .map_err(|error| RegistryError::with("read generation file type", error))?;
            if !file_type.is_file() || file_type.is_symlink() {
                continue;
            }
            let relative = relative_directory.join(entry.file_name());
            if retained.contains(&relative) || !generation_artifact_name(&entry.file_name()) {
                continue;
            }
            fs::remove_file(entry.path())
                .map_err(|error| RegistryError::with("remove orphan generation", error))?;
            removed.push(relative);
        }
    }
    Ok(removed)
}

#[cfg(feature = "builder")]
fn generation_artifact_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let number = name
        .strip_suffix(".sqlite3")
        .or_else(|| name.strip_suffix(".nav.lz4b"))
        .or_else(|| name.strip_suffix(".navpack"));
    number.is_some_and(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
}

#[cfg(feature = "builder")]
fn ensure_staging_path(storage_root: &Path, path: &Path) -> Result<(), RegistryError> {
    let canonical_root = fs::canonicalize(storage_root)
        .map_err(|error| RegistryError::with("canonicalize catalog root", error))?;
    let staging = storage_root.join("staging");
    let canonical_staging = fs::canonicalize(&staging)
        .map_err(|error| RegistryError::with("canonicalize staging directory", error))?;
    let canonical_path = fs::canonicalize(path)
        .map_err(|error| RegistryError::with("canonicalize staged artifact", error))?;
    if !canonical_staging.starts_with(&canonical_root)
        || !canonical_path.starts_with(&canonical_staging)
        || canonical_path == canonical_staging
    {
        return Err(RegistryError::new(
            "publish-artifact",
            "staged artifact is outside the catalog staging directory",
        ));
    }
    Ok(())
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn valid_hash(value: &str) -> bool {
    value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn regular_file_size(path: &Path, maximum: u64) -> Result<u64, RegistryError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| RegistryError::with("stat catalog artifact", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(RegistryError::new(
            "validate-artifact",
            "catalog artifact is not a regular file",
        ));
    }
    if metadata.len() > maximum {
        return Err(RegistryError::new(
            "validate-artifact",
            "catalog artifact exceeds configured size limit",
        ));
    }
    Ok(metadata.len())
}

fn read_regular_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, RegistryError> {
    regular_file_size(path, maximum as u64)?;
    fs::read(path).map_err(|error| RegistryError::with("read manifest slot", error))
}

fn file_checksum(path: &Path) -> Result<String, RegistryError> {
    let mut file = File::open(path)
        .map_err(|error| RegistryError::with("open artifact for checksum", error))?;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| RegistryError::with("read artifact checksum", error))?;
        if read == 0 {
            break;
        }
        update_checksum(&mut hash, &buffer[..read]);
    }
    Ok(format!("{hash:016x}"))
}

fn update_checksum(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

#[cfg(feature = "builder")]
fn path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(feature = "builder")]
fn sync_directory(path: &Path) -> Result<(), RegistryError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| RegistryError::with("sync publication directory", error))
}

#[cfg(feature = "builder")]
struct TemporaryCleanup(Option<PathBuf>);

#[cfg(feature = "builder")]
impl Drop for TemporaryCleanup {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryError {
    stage: &'static str,
    message: String,
}

impl RegistryError {
    fn new(stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }

    fn with(stage: &'static str, error: impl fmt::Display) -> Self {
        Self::new(stage, error.to_string())
    }

    pub fn stage(&self) -> &'static str {
        self.stage
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.stage, self.message)
    }
}

impl Error for RegistryError {}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "builder")]
    use crate::system_shard::{SystemGame, SystemShardData, write_system_shard};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    #[cfg(feature = "builder")]
    fn copied_artifact_hashes_the_bytes_during_the_copy() {
        let root = temporary_root("copy-hash");
        let source = root.join("source");
        let target = root.join("target");
        let bytes = b"catalog artifact bytes";
        fs::write(&source, bytes).unwrap();

        let copied = copy_staged_artifact(&source, &target, "fixture", bytes.len() as u64)
            .expect("copy fixture artifact");

        assert_eq!(copied.bytes, bytes.len() as u64);
        assert_eq!(copied.hash, file_checksum(&source).unwrap());
        assert_eq!(fs::read(target).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn artifact_copy_modes_are_typed_and_default_to_buffered() {
        assert_eq!(
            artifact_copy_mode_from_value(None).unwrap(),
            ArtifactCopyMode::Buffered
        );
        assert_eq!(
            artifact_copy_mode_from_value(Some(std::ffi::OsStr::new("preallocated"))).unwrap(),
            ArtifactCopyMode::Preallocated
        );
        assert_eq!(
            artifact_copy_mode_from_value(Some(std::ffi::OsStr::new("copy-file-range"))).unwrap(),
            ArtifactCopyMode::CopyFileRange
        );
        assert!(artifact_copy_mode_from_value(Some(std::ffi::OsStr::new("automatic"))).is_err());
    }

    #[test]
    #[cfg(feature = "builder")]
    fn truncated_artifact_copy_is_removed_before_publication() {
        let root = temporary_root("copy-truncated");
        let source = root.join("source");
        let target = root.join("target");
        fs::write(&source, b"short").unwrap();

        let error = copy_staged_artifact(&source, &target, "fixture", 99)
            .expect_err("size mismatch must fail");

        assert!(error.message().contains("size does not match"));
        assert!(!target.exists());
        let stale_temporary = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".target.tmp.")
            });
        assert!(!stale_temporary);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn registry_recovery_removes_only_scoped_publication_temporaries() {
        let root = temporary_root("registry-temporary-recovery");
        fs::create_dir_all(root.join("registry")).unwrap();
        fs::create_dir_all(root.join("systems/snes")).unwrap();
        fs::write(root.join("registry/.manifest-a.tmp.crashed"), b"partial").unwrap();
        fs::write(root.join("systems/snes/.2.navpack.tmp.crashed"), b"partial").unwrap();
        fs::write(root.join("systems/snes/keep.tmp.data"), b"unrelated").unwrap();
        fs::write(root.join(".keep.tmp.crashed"), b"unrelated").unwrap();

        assert_eq!(cleanup_registry_temporary_files(&root).unwrap(), 2);
        assert!(!root.join("registry/.manifest-a.tmp.crashed").exists());
        assert!(!root.join("systems/snes/.2.navpack.tmp.crashed").exists());
        assert!(root.join("systems/snes/keep.tmp.data").exists());
        assert!(root.join(".keep.tmp.crashed").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn deferred_publication_renames_on_media_staging_without_copying() {
        let root = temporary_root("deferred-rename");
        let system_id = SystemId::parse("snes").unwrap();
        let staging = root.join("staging/run-1");
        let (sqlite, navigation) = stage_generation(&staging, &system_id, 1);

        let publication = publish_prevalidated_system_artifacts_deferred(
            &root,
            &sqlite,
            &navigation,
            &system_id,
            1,
            1,
            limits(),
        )
        .unwrap();

        assert_eq!(publication.copied_bytes, 0);
        assert!(!sqlite.exists());
        assert!(!navigation.exists());
        assert!(root.join(publication.generation.sqlite_path).is_file());
        assert!(root.join(publication.generation.navigation_path).is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn deferred_publication_still_copies_external_staging() {
        let root = temporary_root("deferred-copy-target");
        let external = temporary_root("deferred-copy-source");
        let system_id = SystemId::parse("snes").unwrap();
        let (sqlite, navigation) = stage_generation(&external, &system_id, 1);

        let publication = publish_prevalidated_system_artifacts_deferred(
            &root,
            &sqlite,
            &navigation,
            &system_id,
            1,
            1,
            limits(),
        )
        .unwrap();

        assert!(publication.copied_bytes > 0);
        assert!(sqlite.exists());
        assert!(navigation.exists());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(external).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn uncommitted_generation_does_not_replace_the_old_manifest() {
        let root = temporary_root("manifest-last");
        let system_id = SystemId::parse("snes").unwrap();
        let first = create_generation(&root, &system_id, 1);
        let first_manifest = manifest(1, first.clone(), None);
        publish_manifest(&root, &first_manifest, limits()).unwrap();

        let second = create_generation(&root, &system_id, 2);
        assert_eq!(
            read_latest_manifest(&root, limits()).unwrap(),
            first_manifest
        );

        let second_manifest = manifest(2, second, Some(first));
        publish_manifest(&root, &second_manifest, limits()).unwrap();
        assert_eq!(
            read_latest_manifest(&root, limits()).unwrap(),
            second_manifest
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn corrupt_newest_slot_falls_back_to_the_previous_valid_slot() {
        let root = temporary_root("slot-fallback");
        let system_id = SystemId::parse("snes").unwrap();
        let first = create_generation(&root, &system_id, 1);
        publish_manifest(&root, &manifest(1, first.clone(), None), limits()).unwrap();
        let second = create_generation(&root, &system_id, 2);
        publish_manifest(&root, &manifest(2, second, Some(first.clone())), limits()).unwrap();
        fs::write(root.join(MANIFEST_B), b"corrupt").unwrap();
        assert_eq!(
            read_latest_manifest(&root, limits()).unwrap(),
            manifest(1, first, None)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn garbage_collection_retains_active_and_previous_generations_only() {
        let root = temporary_root("garbage-collection");
        let system_id = SystemId::parse("snes").unwrap();
        let first = create_generation(&root, &system_id, 1);
        let second = create_generation(&root, &system_id, 2);
        let third = create_generation(&root, &system_id, 3);
        let obsolete = create_generation(&root, &SystemId::parse("gamegear").unwrap(), 1);
        let orphaned_navpack = root.join("systems/snes/4.navpack");
        fs::write(&orphaned_navpack, b"orphaned NavPack").unwrap();
        let manifest = manifest(3, third, Some(second));
        let removed = garbage_collect_unreferenced(&root, &manifest).unwrap();
        assert_eq!(removed.len(), 5);
        assert!(!root.join(first.sqlite_path).exists());
        assert!(!root.join(first.navigation_path).exists());
        assert!(!root.join(obsolete.sqlite_path).exists());
        assert!(!root.join(obsolete.navigation_path).exists());
        assert!(!orphaned_navpack.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn garbage_collection_preserves_additional_unpublished_artifacts() {
        let root = temporary_root("garbage-collection-retained");
        let system_id = SystemId::parse("snes").unwrap();
        let active = create_generation(&root, &system_id, 1);
        let unpublished = create_generation(&root, &system_id, 2);
        let obsolete = create_generation(&root, &SystemId::parse("gamegear").unwrap(), 1);
        let retained = [
            unpublished.sqlite_path.clone(),
            unpublished.navigation_path.clone(),
        ];

        let removed =
            garbage_collect_unreferenced_with_retained(&root, &manifest(1, active, None), retained)
                .unwrap();

        assert_eq!(removed.len(), 2);
        assert!(root.join(unpublished.sqlite_path).exists());
        assert!(root.join(unpublished.navigation_path).exists());
        assert!(!root.join(obsolete.sqlite_path).exists());
        assert!(!root.join(obsolete.navigation_path).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(feature = "builder")]
    fn unpublished_system_validation_checks_hashes_and_shard_contents() {
        let root = temporary_root("validate-unpublished");
        let system_id = SystemId::parse("snes").unwrap();
        let generation = create_generation(&root, &system_id, 1);
        let system = manifest(1, generation.clone(), None).systems.remove(0);

        validate_published_system(&root, &system, limits()).unwrap();

        fs::write(root.join(&generation.navigation_path), b"corrupt").unwrap();
        let error = validate_published_system(&root, &system, limits()).unwrap_err();
        assert!(
            error.message().contains("size does not match")
                || error.message().contains("hash does not match")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stored_manifest_rejects_traversal_paths() {
        let stored = StoredManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            format: Some(CatalogFormatDescriptor::current()),
            generation: 1,
            systems: vec![StoredSystem {
                system_id: "snes".to_string(),
                display_title: "SNES".to_string(),
                section: "Consoles".to_string(),
                family: "Nintendo".to_string(),
                order: 1,
                producers: vec!["snes-root".to_string()],
                active: StoredGeneration {
                    generation: 1,
                    sqlite_path: "../escape.sqlite3".to_string(),
                    navigation_path: "systems/snes/1.nav.lz4b".to_string(),
                    sqlite_bytes: 1,
                    navigation_bytes: 1,
                    sqlite_hash: "0000000000000000".to_string(),
                    navigation_hash: "0000000000000000".to_string(),
                    games: 1,
                    navpack: None,
                },
                previous: None,
            }],
        };
        let manifest = from_stored(stored).unwrap();
        let root = temporary_root("traversal");
        assert_eq!(
            validate_manifest(&root, &manifest, limits(), false, false)
                .unwrap_err()
                .message(),
            "manifest artifact path is not canonical"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "builder")]
    fn create_generation(
        root: &Path,
        system_id: &SystemId,
        generation: u64,
    ) -> PublishedGeneration {
        let staging = root.join("staging").join(format!("run-{generation}"));
        let (sqlite, navigation) = stage_generation(&staging, system_id, generation);
        publish_system_artifacts(
            root,
            &sqlite,
            &navigation,
            system_id,
            generation,
            1,
            limits(),
        )
        .unwrap()
    }

    #[cfg(feature = "builder")]
    fn stage_generation(
        staging: &Path,
        system_id: &SystemId,
        generation: u64,
    ) -> (PathBuf, PathBuf) {
        fs::create_dir_all(staging).unwrap();
        let sqlite = staging.join("system.sqlite3");
        let navigation = staging.join("system.nav.lz4b");
        let data = SystemShardData {
            system_id: system_id.clone(),
            generation,
            projection_stats: None,
            games: vec![SystemGame {
                stable_key: "one".to_string(),
                title: "Synthetic One".to_string(),
                launch_ref: "/games/SNES/One.sfc".to_string(),
                ..SystemGame::default()
            }],
        };
        write_system_shard(&sqlite, &navigation, &data, limits().shard).unwrap();
        (sqlite, navigation)
    }

    #[cfg(feature = "builder")]
    fn manifest(
        generation: u64,
        active: PublishedGeneration,
        previous: Option<PublishedGeneration>,
    ) -> CatalogManifest {
        CatalogManifest {
            format: Some(CatalogFormatDescriptor::current()),
            generation,
            systems: vec![ManifestSystem {
                system_id: SystemId::parse("snes").unwrap(),
                display_title: "SNES".to_string(),
                section: "Consoles".to_string(),
                family: "Nintendo".to_string(),
                order: 1,
                producers: vec![ScanUnitId::parse("snes-root").unwrap()],
                active,
                previous,
            }],
        }
    }

    fn limits() -> RegistryLimits {
        RegistryLimits {
            max_manifest_bytes: 1024 * 1024,
            max_systems: 100,
            shard: SystemShardLimits {
                max_sqlite_bytes: 2 * 1024 * 1024,
                max_navigation_compressed_bytes: 256 * 1024,
                max_navigation_decoded_bytes: 1024 * 1024,
                max_games: 100,
            },
        }
    }

    fn temporary_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "mister-magik-shard-registry-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        root
    }
}
