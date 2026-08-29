// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Disposable source state for the independent fast catalog.
//!
//! The watch index is intentionally separate from canonical catalog rows. An
//! unchanged refresh reads and validates only the manifest and watch indexes;
//! changed systems are rebuilt from source rather than hydrating persisted row
//! snapshots.

use crate::catalog_scan::should_ignore_path;
use crate::fast_five_catalog::{FastFiveGameVariant, FastFiveSnapshot, FastFiveSystem};
use crate::generic_system_catalog::GenericSourceWatchObservations;
use crate::system_shard::SystemGame;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const REFRESH_SCHEMA: u32 = 2;
const ENVELOPE_VERSION: u32 = 1;
const ENVELOPE_BYTES: usize = 64;
const MANIFEST_MAGIC: &[u8; 8] = b"MGKRFSMF";
const WATCH_MAGIC: &[u8; 8] = b"MGKRFSWI";
const WATCH_PACK_MAGIC: &[u8; 8] = b"MGKRFSPK";
const BUILD_INFO_MAGIC: &[u8; 8] = b"MGKRFBIN";
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_WATCH_BYTES: usize = 16 * 1024 * 1024;
const MAX_BUILD_INFO_BYTES: usize = 4096;
const STATE_DIRECTORY: &str = "fast-refresh-v2";
const MANIFEST_A: &str = "manifest-a.bin";
const MANIFEST_B: &str = "manifest-b.bin";
const BUILD_INFO_FILE: &str = "build-info.bin";
const MAX_WATCH_DEPTH: usize = 256;
const MAX_WATCH_ENTRIES: usize = 4_000_000;
const MAX_WATCH_DIRECTORY_ENTRIES: usize = 1_000_000;

fn fast_catalog_artifact_profile() -> crate::fast_five_catalog::FastFiveArtifactProfile {
    if std::env::var("MISTER_CATALOG_SEARCH_DETAIL")
        .is_ok_and(|value| value.eq_ignore_ascii_case("column"))
    {
        crate::fast_five_catalog::FastFiveArtifactProfile::SearchColumn
    } else {
        crate::fast_five_catalog::FastFiveArtifactProfile::SearchOnly
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastRefreshManifest {
    pub schema: u32,
    pub generation: u64,
    pub catalog_generation: u64,
    pub catalog_fingerprint: String,
    pub builder_identity: String,
    pub systems: Vec<FastRefreshSystemRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastRefreshSystemRef {
    pub system_id: String,
    pub watch_path: String,
    pub watch_sha256: String,
    pub source_fingerprint: String,
    pub row_fingerprint: String,
    pub games: u64,
    pub variants: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastSystemWatchIndex {
    pub schema: u32,
    pub system_id: String,
    pub adapter_version: u32,
    pub core_profile_fingerprint: String,
    pub roots: Vec<String>,
    pub directories: Vec<FastWatchedDirectory>,
    pub containers: Vec<FastWatchedContainer>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct FastSystemWatchPack {
    schema: u32,
    watches: Vec<FastSystemWatchIndex>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastWatchedDirectory {
    pub path: String,
    pub modified_ns: i128,
    pub entry_fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastWatchedContainer {
    pub path: String,
    pub size: u64,
    pub modified_ns: i128,
    pub changed_ns: i128,
    pub inode: u64,
    pub content_directory_fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastSystemRowsSnapshot {
    pub schema: u32,
    pub system_id: String,
    pub games: Vec<SystemGame>,
    pub variants: Vec<FastFiveGameVariant>,
}

#[derive(Clone, Debug)]
pub struct FastRefreshSystemState {
    pub watch: FastSystemWatchIndex,
    pub row_fingerprint: String,
    pub games: u64,
    pub variants: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct FastRefreshCaptureReport {
    pub elapsed_us: u64,
    pub directories: usize,
    pub containers: usize,
    pub systems: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct FastRefreshStatePublishReport {
    pub elapsed_us: u64,
    pub validation_us: u64,
    pub encoding_us: u64,
    pub fingerprint_us: u64,
    pub write_us: u64,
    pub manifest_us: u64,
    pub sync_us: u64,
    pub systems: usize,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FastCatalogBuildInfo {
    pub schema: u32,
    pub catalog_generation: u64,
    pub catalog_fingerprint: String,
    pub completed_unix_ms: u64,
    pub elapsed_us: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FastCatalogRefreshRequest {
    Update,
    RebuildAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FastSourceCheckStatus {
    Unchanged,
    Changed,
    Rescan,
}

#[derive(Clone, Debug, Serialize)]
pub struct FastSystemSourceCheck {
    pub system_id: String,
    pub status: FastSourceCheckStatus,
    pub directories_checked: usize,
    pub containers_checked: usize,
    pub elapsed_us: u64,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FastRefreshPlanReport {
    pub elapsed_us: u64,
    pub manifest_read_us: u64,
    pub active_read_us: u64,
    pub system_discovery_us: u64,
    pub checks_us: u64,
    pub watch_read_us: u64,
    pub metadata_probe_us: u64,
    pub metadata_parents: usize,
    pub metadata_paths: usize,
    pub systems: usize,
    pub unchanged: usize,
    pub changed: usize,
    pub rescans: usize,
    pub artifact_writes: usize,
    pub checks: Vec<FastSystemSourceCheck>,
    #[serde(skip)]
    pub(crate) previous_manifest: Option<FastRefreshManifest>,
    #[serde(skip)]
    pub(crate) active_manifest: Option<crate::shard_registry::CatalogManifest>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FastCatalogSystemOutcome {
    Unchanged,
    Updated,
    Removed,
    FailedRetained,
}

#[derive(Clone, Debug, Serialize)]
pub struct FastCatalogSystemRefreshReport {
    pub system_id: String,
    pub outcome: FastCatalogSystemOutcome,
    pub source_status: FastSourceCheckStatus,
    pub games: u64,
    pub variants: u64,
    pub elapsed_us: u64,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FastCatalogRefreshReport {
    pub request: FastCatalogRefreshRequest,
    pub elapsed_us: u64,
    pub planning_us: u64,
    pub source_rebuild_us: u64,
    pub artifact_publish_us: u64,
    pub snapshot_publish_us: u64,
    pub systems: usize,
    pub unchanged: usize,
    pub updated: usize,
    pub removed: usize,
    pub failed_retained: usize,
    pub artifact_systems_written: usize,
    pub catalog_generation: u64,
    pub refresh_generation: u64,
    pub games: u64,
    pub system_reports: Vec<FastCatalogSystemRefreshReport>,
    pub plan: FastRefreshPlanReport,
}

#[derive(Clone, Debug, Serialize)]
pub struct FastCatalogFreshBuildReport {
    pub elapsed_us: u64,
    pub source: crate::fast_catalog_sources::FastSourceBuildReport,
    pub publication: crate::fast_five_catalog::FastFivePublishReport,
    pub capture: FastRefreshCaptureReport,
    pub refresh_state_publish: FastRefreshStatePublishReport,
    pub refresh_generation: u64,
    pub system_ids: Vec<String>,
    pub build_info_persisted: bool,
}

pub fn build_fresh_catalog(
    storage_root: &Path,
    catalog_root: &Path,
) -> Result<FastCatalogFreshBuildReport, String> {
    build_fresh_catalog_with_progress(storage_root, catalog_root, |_| {}, |_| {})
}

pub fn build_fresh_catalog_with_progress(
    storage_root: &Path,
    catalog_root: &Path,
    plan_ready: impl FnMut(&[String]),
    system_complete: impl FnMut(&FastFiveSystem),
) -> Result<FastCatalogFreshBuildReport, String> {
    let started = std::time::Instant::now();
    let source_build =
        crate::fast_catalog_sources::build_independent_fast_snapshot_for_refresh_with_progress(
            storage_root,
            plan_ready,
            system_complete,
        )?;
    let crate::fast_catalog_sources::FastSourceRefreshBuild {
        snapshot,
        report: source,
        profiles,
        generic_watch_observations,
        row_fingerprints,
    } = source_build;
    let system_ids = snapshot
        .systems
        .iter()
        .map(|system| system.system_id.clone())
        .collect::<Vec<_>>();
    let publication = crate::fast_five_catalog::publish_snapshot_with_profile(
        catalog_root,
        &snapshot,
        crate::shard_registry::production_registry_limits(),
        fast_catalog_artifact_profile(),
    )?;
    let (states, capture) = capture_refresh_state_with_profiles(
        storage_root,
        &snapshot,
        &profiles,
        Some(&generic_watch_observations),
        Some(&row_fingerprints),
    )?;
    drop(snapshot);
    drop(profiles);
    drop(generic_watch_observations);
    let refresh_generation = read_latest_refresh_manifest(catalog_root)
        .map_or(1, |manifest| manifest.generation.saturating_add(1));
    let (_, refresh_state_publish) = publish_refresh_state_with_report(
        catalog_root,
        refresh_generation,
        publication.generation,
        publication.registry_fingerprint.clone(),
        format!(
            "independent-fast-sources-v{}",
            crate::fast_catalog_sources::FAST_SOURCE_ADAPTER_VERSION
        ),
        &states,
    )?;
    let build_elapsed_us = started.elapsed().as_micros().try_into().unwrap_or(u64::MAX);
    let build_info = FastCatalogBuildInfo::new(
        publication.generation,
        publication.registry_fingerprint.clone(),
        build_elapsed_us,
    )?;
    let build_info_persisted = match publish_build_info(catalog_root, &build_info) {
        Ok(()) => true,
        Err(error) => {
            crate::catalog_logln!(
                "fast_catalog_build_info_tsv\tstatus=failed\terror={}",
                error.replace('\t', " ")
            );
            false
        }
    };
    Ok(FastCatalogFreshBuildReport {
        elapsed_us: build_elapsed_us,
        source,
        publication,
        capture,
        refresh_state_publish,
        refresh_generation,
        system_ids,
        build_info_persisted,
    })
}

pub fn remove_default_catalog_artifacts() -> Result<usize, String> {
    let paths = crate::device_layout::CatalogPaths::capture_process();
    remove_catalog_artifacts(paths.sharded_catalog_dir())
}

pub fn remove_catalog_artifacts(catalog_root: &Path) -> Result<usize, String> {
    if catalog_root.file_name().and_then(|name| name.to_str()) != Some("catalog-fast-v1") {
        return Err(format!(
            "refusing to remove unexpected catalog path {}",
            catalog_root.display()
        ));
    }
    if !catalog_root.exists() {
        return Ok(0);
    }
    let entries = walkdir::WalkDir::new(catalog_root)
        .into_iter()
        .filter_map(Result::ok)
        .count();
    fs::remove_dir_all(catalog_root)
        .map_err(|error| format!("remove catalog {}: {error}", catalog_root.display()))?;
    Ok(entries)
}

impl FastRefreshManifest {
    pub fn new(
        generation: u64,
        catalog_generation: u64,
        catalog_fingerprint: String,
        builder_identity: String,
        systems: Vec<FastRefreshSystemRef>,
    ) -> Result<Self, String> {
        let manifest = Self {
            schema: REFRESH_SCHEMA,
            generation,
            catalog_generation,
            catalog_fingerprint,
            builder_identity,
            systems,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != REFRESH_SCHEMA || self.generation == 0 {
            return Err("unsupported fast refresh manifest".to_string());
        }
        validate_sha256(&self.catalog_fingerprint, "catalog fingerprint")?;
        if self.builder_identity.trim().is_empty() {
            return Err("fast refresh builder identity is empty".to_string());
        }
        let mut systems = BTreeSet::new();
        for system in &self.systems {
            if !systems.insert(system.system_id.as_str()) {
                return Err(format!("duplicate refresh system {}", system.system_id));
            }
            validate_relative_path(&system.watch_path)?;
            validate_sha256(&system.watch_sha256, "watch checksum")?;
            validate_sha256(&system.source_fingerprint, "source fingerprint")?;
            validate_sha256(&system.row_fingerprint, "row fingerprint")?;
        }
        Ok(())
    }
}

impl FastCatalogBuildInfo {
    fn new(
        catalog_generation: u64,
        catalog_fingerprint: String,
        elapsed_us: u64,
    ) -> Result<Self, String> {
        let completed_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "clock predates Unix epoch")?
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        let info = Self {
            schema: REFRESH_SCHEMA,
            catalog_generation,
            catalog_fingerprint,
            completed_unix_ms,
            elapsed_us,
        };
        info.validate()?;
        Ok(info)
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema != REFRESH_SCHEMA
            || self.catalog_generation == 0
            || self.completed_unix_ms == 0
        {
            return Err("invalid fast catalog build information".to_string());
        }
        validate_sha256(&self.catalog_fingerprint, "build catalog fingerprint")
    }
}

impl FastSystemWatchIndex {
    pub fn new(
        system_id: String,
        adapter_version: u32,
        core_profile_fingerprint: String,
        roots: Vec<String>,
        directories: Vec<FastWatchedDirectory>,
        containers: Vec<FastWatchedContainer>,
    ) -> Result<Self, String> {
        let watch = Self {
            schema: REFRESH_SCHEMA,
            system_id,
            adapter_version,
            core_profile_fingerprint,
            roots,
            directories,
            containers,
        };
        let system_id = watch.system_id.clone();
        watch.validate(&system_id)?;
        Ok(watch)
    }

    pub fn validate(&self, expected_system_id: &str) -> Result<(), String> {
        if self.schema != REFRESH_SCHEMA || self.system_id != expected_system_id {
            return Err(format!("invalid watch index for {expected_system_id}"));
        }
        validate_sha256(&self.core_profile_fingerprint, "core/profile fingerprint")?;
        validate_unique_paths(self.roots.iter().map(String::as_str), "watch root")?;
        validate_unique_paths(
            self.directories.iter().map(|entry| entry.path.as_str()),
            "watched directory",
        )?;
        validate_unique_paths(
            self.containers.iter().map(|entry| entry.path.as_str()),
            "watched container",
        )?;
        for directory in &self.directories {
            validate_sha256(&directory.entry_fingerprint, "directory fingerprint")?;
        }
        for container in &self.containers {
            validate_sha256(
                &container.content_directory_fingerprint,
                "container directory fingerprint",
            )?;
        }
        Ok(())
    }
}

impl FastSystemWatchPack {
    fn new(watches: Vec<FastSystemWatchIndex>) -> Result<Self, String> {
        let pack = Self {
            schema: REFRESH_SCHEMA,
            watches,
        };
        pack.validate()?;
        Ok(pack)
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema != REFRESH_SCHEMA {
            return Err("unsupported fast refresh watch pack".to_string());
        }
        let mut systems = BTreeSet::new();
        for watch in &self.watches {
            if !systems.insert(watch.system_id.as_str()) {
                return Err(format!(
                    "duplicate packed refresh watch {}",
                    watch.system_id
                ));
            }
            watch.validate(&watch.system_id)?;
        }
        Ok(())
    }
}

impl FastSystemRowsSnapshot {
    pub fn new(
        system_id: String,
        games: Vec<SystemGame>,
        variants: Vec<FastFiveGameVariant>,
    ) -> Result<Self, String> {
        let rows = Self {
            schema: REFRESH_SCHEMA,
            system_id,
            games,
            variants,
        };
        let system_id = rows.system_id.clone();
        rows.validate(&system_id)?;
        Ok(rows)
    }

    pub fn validate(&self, expected_system_id: &str) -> Result<(), String> {
        if self.schema != REFRESH_SCHEMA || self.system_id != expected_system_id {
            return Err(format!("invalid row snapshot for {expected_system_id}"));
        }
        validate_row_parts(expected_system_id, &self.games, &self.variants)
    }
}

#[derive(Serialize)]
struct FastSystemRowsSnapshotRef<'a> {
    schema: u32,
    system_id: &'a str,
    games: &'a [SystemGame],
    variants: &'a [FastFiveGameVariant],
}

pub fn refresh_state_root(catalog_root: &Path) -> PathBuf {
    catalog_root.join("state").join(STATE_DIRECTORY)
}

/// Remove only temporary files created by refresh publication.
///
/// A crashed builder may leave a fully-written temporary file behind.  These
/// files are never considered active state, so removing them while holding the
/// mutation lease makes the next run deterministic without touching manifests,
/// packs, or registry generations.
pub fn cleanup_refresh_temporary_files(catalog_root: &Path) -> Result<usize, String> {
    let root = refresh_state_root(catalog_root);
    if !root.exists() {
        return Ok(0);
    }
    let mut removed = 0usize;
    for entry in walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if !name.contains(".tmp-") {
            continue;
        }
        fs::remove_file(entry.path()).map_err(|error| {
            format!(
                "remove refresh temporary {}: {error}",
                entry.path().display()
            )
        })?;
        removed = removed.saturating_add(1);
    }
    if removed != 0 {
        sync_directory(&root)?;
    }
    Ok(removed)
}

pub fn build_info_path(catalog_root: &Path) -> PathBuf {
    refresh_state_root(catalog_root).join(BUILD_INFO_FILE)
}

fn publish_build_info(catalog_root: &Path, info: &FastCatalogBuildInfo) -> Result<(), String> {
    info.validate()?;
    let path = build_info_path(catalog_root);
    let bytes = encode_envelope(info, BUILD_INFO_MAGIC)?;
    write_replace_file(&path, &bytes)?;
    sync_directory(
        path.parent()
            .ok_or_else(|| "fast catalog build information has no parent".to_string())?,
    )
}

pub fn read_current_build_info(
    catalog_root: &Path,
) -> Result<Option<FastCatalogBuildInfo>, String> {
    let path = build_info_path(catalog_root);
    if !path.exists() {
        return Ok(None);
    }
    let info: FastCatalogBuildInfo = read_envelope(&path, BUILD_INFO_MAGIC, MAX_BUILD_INFO_BYTES)?;
    info.validate()?;
    let active = crate::shard_registry::read_latest_manifest_lazy(
        catalog_root,
        crate::shard_registry::production_registry_limits(),
    )
    .map_err(|error| format!("read active fast catalog for build information: {error}"))?;
    let fingerprint = crate::fast_five_catalog::registry_fingerprint(
        catalog_root,
        crate::shard_registry::production_registry_limits(),
    )?;
    if info.catalog_generation != active.generation || info.catalog_fingerprint != fingerprint {
        return Ok(None);
    }
    Ok(Some(info))
}

pub fn format_build_elapsed(elapsed_us: u64) -> String {
    let seconds = elapsed_us.saturating_add(500_000) / 1_000_000;
    if seconds == 1 {
        "1 second".to_string()
    } else {
        format!("{seconds} seconds")
    }
}

pub fn read_latest_refresh_manifest(catalog_root: &Path) -> Result<FastRefreshManifest, String> {
    let root = refresh_state_root(catalog_root);
    let mut candidates = [MANIFEST_A, MANIFEST_B]
        .into_iter()
        .filter_map(|name| {
            let path = root.join(name);
            read_envelope::<FastRefreshManifest>(&path, MANIFEST_MAGIC, MAX_MANIFEST_BYTES)
                .ok()
                .and_then(|manifest| manifest.validate().ok().map(|()| manifest))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|manifest| manifest.generation);
    candidates
        .pop()
        .ok_or_else(|| "no valid fast refresh manifest".to_string())
}

pub fn read_system_watch(
    catalog_root: &Path,
    reference: &FastRefreshSystemRef,
) -> Result<FastSystemWatchIndex, String> {
    let path = refresh_state_root(catalog_root).join(&reference.watch_path);
    let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    verify_file_checksum(&bytes, &reference.watch_sha256, &path)?;
    if let Ok(pack) =
        decode_envelope::<FastSystemWatchPack>(&bytes, WATCH_PACK_MAGIC, MAX_WATCH_BYTES)
    {
        pack.validate()?;
        let watch = pack
            .watches
            .into_iter()
            .find(|watch| watch.system_id == reference.system_id)
            .ok_or_else(|| format!("packed refresh watch is missing {}", reference.system_id))?;
        watch.validate(&reference.system_id)?;
        return Ok(watch);
    }
    let watch: FastSystemWatchIndex = decode_envelope(&bytes, WATCH_MAGIC, MAX_WATCH_BYTES)?;
    watch.validate(&reference.system_id)?;
    Ok(watch)
}

pub fn publish_refresh_state(
    catalog_root: &Path,
    generation: u64,
    catalog_generation: u64,
    catalog_fingerprint: String,
    builder_identity: String,
    systems: &[FastRefreshSystemState],
) -> Result<FastRefreshManifest, String> {
    publish_refresh_state_with_report(
        catalog_root,
        generation,
        catalog_generation,
        catalog_fingerprint,
        builder_identity,
        systems,
    )
    .map(|(manifest, _)| manifest)
}

pub fn publish_refresh_state_with_report(
    catalog_root: &Path,
    generation: u64,
    catalog_generation: u64,
    catalog_fingerprint: String,
    builder_identity: String,
    systems: &[FastRefreshSystemState],
) -> Result<(FastRefreshManifest, FastRefreshStatePublishReport), String> {
    let started = std::time::Instant::now();
    if generation == 0 {
        return Err("fast refresh generation must be non-zero".to_string());
    }
    let root = refresh_state_root(catalog_root);
    fs::create_dir_all(root.join("packs"))
        .map_err(|error| format!("create refresh state root: {error}"))?;
    let mut references = Vec::with_capacity(systems.len());
    let mut report = FastRefreshStatePublishReport {
        systems: systems.len(),
        ..FastRefreshStatePublishReport::default()
    };
    for state in systems {
        let phase_started = std::time::Instant::now();
        state.watch.validate(&state.watch.system_id)?;
        validate_sha256(&state.row_fingerprint, "row fingerprint")?;
        report.validation_us = report
            .validation_us
            .saturating_add(phase_started.elapsed().as_micros() as u64);
    }
    let pack = FastSystemWatchPack::new(systems.iter().map(|state| state.watch.clone()).collect())?;
    let encoding_started = std::time::Instant::now();
    let (pack_bytes, _) = encode_envelope_with_payload_fingerprint(&pack, WATCH_PACK_MAGIC)?;
    report.encoding_us = encoding_started.elapsed().as_micros() as u64;
    report.bytes = pack_bytes.len() as u64;
    report.files = 1;
    let fingerprint_started = std::time::Instant::now();
    let watch_sha256 = sha256_hex(&pack_bytes);
    let pack_watch_fingerprints = systems
        .iter()
        .map(|state| source_fingerprint(&state.watch))
        .collect::<Vec<_>>();
    report.fingerprint_us = fingerprint_started.elapsed().as_micros() as u64;
    let watch_relative = format!("packs/{generation}.watchpack");
    let write_started = std::time::Instant::now();
    write_new_file(&root.join(&watch_relative), &pack_bytes)?;
    report.write_us = write_started.elapsed().as_micros() as u64;
    for (state, source_fingerprint) in systems.iter().zip(pack_watch_fingerprints) {
        references.push(FastRefreshSystemRef {
            system_id: state.watch.system_id.clone(),
            watch_path: watch_relative.clone(),
            watch_sha256: watch_sha256.clone(),
            source_fingerprint,
            row_fingerprint: state.row_fingerprint.clone(),
            games: state.games,
            variants: state.variants,
        });
    }
    let batch_sync_started = std::time::Instant::now();
    sync_filesystem(&root)?;
    report.sync_us = report
        .sync_us
        .saturating_add(batch_sync_started.elapsed().as_micros() as u64);
    references.sort_by(|left, right| left.system_id.cmp(&right.system_id));
    let manifest = FastRefreshManifest::new(
        generation,
        catalog_generation,
        catalog_fingerprint,
        builder_identity,
        references,
    )?;
    let manifest_started = std::time::Instant::now();
    let bytes = encode_envelope(&manifest, MANIFEST_MAGIC)?;
    let slot = if generation.is_multiple_of(2) {
        MANIFEST_A
    } else {
        MANIFEST_B
    };
    write_replace_file(&root.join(slot), &bytes)?;
    report.manifest_us = manifest_started.elapsed().as_micros() as u64;
    let sync_started = std::time::Instant::now();
    sync_directory(&root)?;
    report.sync_us = report
        .sync_us
        .saturating_add(sync_started.elapsed().as_micros() as u64);
    report.elapsed_us = started.elapsed().as_micros() as u64;
    crate::catalog_logln!(
        "fast_catalog_refresh_publish_tsv\telapsed_us={}\tvalidation_us={}\tencoding_us={}\tfingerprint_us={}\twrite_us={}\tmanifest_us={}\tsync_us={}\tsystems={}\tfiles={}\tbytes={}",
        report.elapsed_us,
        report.validation_us,
        report.encoding_us,
        report.fingerprint_us,
        report.write_us,
        report.manifest_us,
        report.sync_us,
        report.systems,
        report.files,
        report.bytes,
    );
    Ok((manifest, report))
}

pub fn publish_refresh_update(
    catalog_root: &Path,
    previous: &FastRefreshManifest,
    catalog_generation: u64,
    catalog_fingerprint: String,
    updated: &[FastRefreshSystemState],
    removed_system_ids: &BTreeSet<String>,
) -> Result<FastRefreshManifest, String> {
    let generation = previous
        .generation
        .checked_add(1)
        .ok_or_else(|| "fast refresh generation overflow".to_string())?;
    let root = refresh_state_root(catalog_root);
    fs::create_dir_all(root.join("packs"))
        .map_err(|error| format!("create refresh state root: {error}"))?;
    let mut references = previous
        .systems
        .iter()
        .cloned()
        .map(|reference| (reference.system_id.clone(), reference))
        .collect::<BTreeMap<_, _>>();
    for system_id in removed_system_ids {
        references.remove(system_id);
    }
    for state in updated {
        state.watch.validate(&state.watch.system_id)?;
        validate_sha256(&state.row_fingerprint, "row fingerprint")?;
    }
    let packed_watches = updated
        .iter()
        .map(|state| state.watch.clone())
        .collect::<Vec<_>>();
    let (watch_relative, watch_sha256) = if packed_watches.is_empty() {
        (String::new(), String::new())
    } else {
        let pack = FastSystemWatchPack::new(packed_watches)?;
        let (pack_bytes, _) = encode_envelope_with_payload_fingerprint(&pack, WATCH_PACK_MAGIC)?;
        let watch_relative = format!("packs/{generation}.watchpack");
        write_new_file(&root.join(&watch_relative), &pack_bytes)?;
        (watch_relative, sha256_hex(&pack_bytes))
    };
    for state in updated {
        references.insert(
            state.watch.system_id.clone(),
            FastRefreshSystemRef {
                system_id: state.watch.system_id.clone(),
                watch_path: watch_relative.clone(),
                watch_sha256: watch_sha256.clone(),
                source_fingerprint: source_fingerprint(&state.watch),
                row_fingerprint: state.row_fingerprint.clone(),
                games: state.games,
                variants: state.variants,
            },
        );
    }
    sync_filesystem(&root)?;
    let manifest = FastRefreshManifest::new(
        generation,
        catalog_generation,
        catalog_fingerprint,
        previous.builder_identity.clone(),
        references.into_values().collect(),
    )?;
    let bytes = encode_envelope(&manifest, MANIFEST_MAGIC)?;
    let slot = if generation.is_multiple_of(2) {
        MANIFEST_A
    } else {
        MANIFEST_B
    };
    write_replace_file(&root.join(slot), &bytes)?;
    sync_directory(&root)?;
    Ok(manifest)
}

pub fn capture_refresh_state(
    storage_root: &Path,
    snapshot: &FastFiveSnapshot,
) -> Result<(Vec<FastRefreshSystemState>, FastRefreshCaptureReport), String> {
    let roots = [storage_root.display().to_string()];
    let profiles = crate::launch_profiles::ProfileSet::try_for_roots(&roots)?.into_profiles();
    capture_refresh_state_with_profiles(storage_root, snapshot, &profiles, None, None)
}

fn capture_refresh_state_with_profiles(
    storage_root: &Path,
    snapshot: &FastFiveSnapshot,
    profiles: &[crate::launch_profiles::LaunchProfile],
    generic_watch_observations: Option<&BTreeMap<String, GenericSourceWatchObservations>>,
    precomputed_row_fingerprints: Option<&BTreeMap<String, String>>,
) -> Result<(Vec<FastRefreshSystemState>, FastRefreshCaptureReport), String> {
    let started = std::time::Instant::now();
    snapshot.validate()?;
    let mut anchor_cache = BTreeMap::new();
    let mut states = Vec::with_capacity(snapshot.systems.len());
    let mut report = FastRefreshCaptureReport::default();
    for system in &snapshot.systems {
        let system_started = std::time::Instant::now();
        let specification =
            watch_specification_from_profiles(storage_root, &system.system_id, profiles)?;
        let watch = capture_system_watch_from_specification(
            storage_root,
            &system.system_id,
            specification,
            &mut anchor_cache,
            generic_watch_observations.and_then(|observations| observations.get(&system.system_id)),
        )?;
        crate::catalog_logln!(
            "fast_catalog_capture_tsv\tsystem={}\telapsed_us={}\tdirectories={}\tcontainers={}",
            system.system_id,
            system_started.elapsed().as_micros(),
            watch.directories.len(),
            watch.containers.len(),
        );
        report.directories = report.directories.saturating_add(watch.directories.len());
        report.containers = report.containers.saturating_add(watch.containers.len());
        let row_fingerprint = precomputed_row_fingerprints
            .and_then(|fingerprints| fingerprints.get(&system.system_id))
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| {
                row_fingerprint_parts(&system.system_id, &system.games, &system.variants)
            })?;
        states.push(FastRefreshSystemState {
            watch,
            row_fingerprint,
            games: system.games.len().try_into().unwrap_or(u64::MAX),
            variants: system.variants.len().try_into().unwrap_or(u64::MAX),
        });
    }
    report.systems = states.len();
    report.elapsed_us = started.elapsed().as_micros().try_into().unwrap_or(u64::MAX);
    Ok((states, report))
}

pub fn capture_system_watch(
    storage_root: &Path,
    system_id: &str,
) -> Result<FastSystemWatchIndex, String> {
    let roots = [storage_root.display().to_string()];
    let profiles = crate::launch_profiles::ProfileSet::try_for_roots(&roots)?.into_profiles();
    let specification = watch_specification_from_profiles(storage_root, system_id, &profiles)?;
    capture_system_watch_from_specification(
        storage_root,
        system_id,
        specification,
        &mut BTreeMap::new(),
        None,
    )
}

fn capture_system_watch_from_specification(
    storage_root: &Path,
    system_id: &str,
    specification: WatchSpecification,
    anchor_cache: &mut BTreeMap<PathBuf, FastWatchedDirectory>,
    generic_observations: Option<&GenericSourceWatchObservations>,
) -> Result<FastSystemWatchIndex, String> {
    let mut directories = Vec::new();
    let mut containers = Vec::new();
    for anchor in &specification.anchors {
        if anchor.is_dir() {
            let directory = match anchor_cache.get(anchor) {
                Some(directory) => directory.clone(),
                None => {
                    let directory = capture_directory(anchor)?;
                    anchor_cache.insert(anchor.clone(), directory.clone());
                    directory
                }
            };
            directories.push(directory);
        }
    }
    let expected_roots = specification
        .scan_roots
        .iter()
        .filter(|root| root.is_dir())
        .map(|root| root.to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();
    let reused_generic_observations = generic_observations
        .is_some_and(|observations| observations.complete && observations.roots == expected_roots);
    if let Some(observations) = generic_observations.filter(|_| reused_generic_observations) {
        directories.extend(
            observations
                .directories
                .iter()
                .map(|directory| FastWatchedDirectory {
                    path: directory.path.to_string_lossy().into_owned(),
                    modified_ns: directory.modified_ns,
                    entry_fingerprint: directory.entry_fingerprint.clone(),
                }),
        );
        for path in &observations.containers {
            if is_watched_container(system_id, path) {
                containers.push(capture_container(path)?);
            }
        }
    } else {
        for root in &specification.scan_roots {
            if root.is_dir() {
                if system_id == "arcade" && root.ends_with("_Arcade") {
                    directories.push(capture_directory(root)?);
                } else {
                    capture_tree(root, system_id, &mut directories, &mut containers)?;
                }
            }
        }
    }
    crate::catalog_logln!(
        "fast_catalog_capture_source_tsv\tsystem={}\tgeneric_observations_reused={}",
        system_id,
        u8::from(reused_generic_observations),
    );
    if system_id == "arcade" {
        for path in [
            storage_root.join("mister-magik-dev/arcade-updater-index-v1.lz4b"),
            storage_root.join("mister-magik/arcade-updater-index-v1.lz4b"),
        ] {
            if path.is_file() {
                containers.push(capture_container(&path)?);
            }
        }
    }
    if matches!(system_id, "snes" | "saturn") {
        for path in [
            storage_root.join("mister-magik-dev/mame.sqlite3"),
            storage_root.join("mister-magik/mame.sqlite3"),
        ] {
            if path.is_file() {
                containers.push(capture_container(&path)?);
                break;
            }
        }
    }
    directories.sort_by(|left, right| left.path.cmp(&right.path));
    directories.dedup_by(|left, right| left.path == right.path);
    containers.sort_by(|left, right| left.path.cmp(&right.path));
    containers.dedup_by(|left, right| left.path == right.path);
    FastSystemWatchIndex::new(
        system_id.to_string(),
        crate::fast_catalog_sources::FAST_SOURCE_ADAPTER_VERSION,
        core_profile_fingerprint(&specification.anchors, anchor_cache),
        specification
            .scan_roots
            .iter()
            .chain(&specification.anchors)
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        directories,
        containers,
    )
}

pub fn plan_fast_refresh(
    storage_root: &Path,
    catalog_root: &Path,
    request: FastCatalogRefreshRequest,
) -> Result<FastRefreshPlanReport, String> {
    let started = std::time::Instant::now();
    let phase_started = std::time::Instant::now();
    let manifest = read_latest_refresh_manifest(catalog_root)?;
    let manifest_read_us = phase_started
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX);
    let phase_started = std::time::Instant::now();
    let active = crate::shard_registry::read_latest_manifest_lazy(
        catalog_root,
        crate::shard_registry::production_registry_limits(),
    )
    .map_err(|error| format!("read active fast catalog: {error}"))?;
    let active_fingerprint = crate::fast_five_catalog::registry_fingerprint_for_manifest(&active);
    let active_read_us = phase_started
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX);
    let binding_matches = manifest.catalog_generation == active.generation
        && manifest.catalog_fingerprint == active_fingerprint
        && manifest.builder_identity
            == format!(
                "independent-fast-sources-v{}",
                crate::fast_catalog_sources::FAST_SOURCE_ADAPTER_VERSION
            );
    let references = manifest
        .systems
        .iter()
        .map(|reference| (reference.system_id.as_str(), reference))
        .collect::<std::collections::BTreeMap<_, _>>();
    let watch_started = std::time::Instant::now();
    let mut watch_indices = BTreeMap::new();
    let mut watch_errors = BTreeMap::new();
    let mut packed_watch_indices =
        BTreeMap::<(String, String), BTreeMap<String, FastSystemWatchIndex>>::new();
    if request != FastCatalogRefreshRequest::RebuildAll && binding_matches {
        for reference in &manifest.systems {
            let cache_key = (reference.watch_path.clone(), reference.watch_sha256.clone());
            if !packed_watch_indices.contains_key(&cache_key) {
                let path = refresh_state_root(catalog_root).join(&reference.watch_path);
                let packed = fs::read(&path)
                    .map_err(|error| format!("read {}: {error}", path.display()))
                    .and_then(|bytes| {
                        verify_file_checksum(&bytes, &reference.watch_sha256, &path)?;
                        let pack = decode_envelope::<FastSystemWatchPack>(
                            &bytes,
                            WATCH_PACK_MAGIC,
                            MAX_WATCH_BYTES,
                        )?;
                        pack.validate()?;
                        Ok(pack
                            .watches
                            .into_iter()
                            .map(|watch| (watch.system_id.clone(), watch))
                            .collect::<BTreeMap<_, _>>())
                    });
                if let Ok(packed) = packed {
                    packed_watch_indices.insert(cache_key.clone(), packed);
                }
            }
            if let Some(packed) = packed_watch_indices.get_mut(&cache_key) {
                match packed.remove(&reference.system_id) {
                    Some(watch) => {
                        watch_indices.insert(reference.system_id.clone(), watch);
                    }
                    None => {
                        watch_errors.insert(
                            reference.system_id.clone(),
                            format!("packed refresh watch is missing {}", reference.system_id),
                        );
                    }
                }
                continue;
            }
            match read_system_watch(catalog_root, reference) {
                Ok(watch) => {
                    watch_indices.insert(reference.system_id.clone(), watch);
                }
                Err(error) => {
                    watch_errors.insert(reference.system_id.clone(), error);
                }
            }
        }
    }
    let watch_read_us = watch_started
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX);
    let discovery_anchor_paths = discovery_anchor_paths(storage_root);
    let metadata_started = std::time::Instant::now();
    let (metadata_cache, metadata_parents, metadata_paths) =
        build_watch_metadata_cache(watch_indices.values(), &discovery_anchor_paths);
    let metadata_probe_us = metadata_started
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX);
    let build_check = |system_id: &str| {
        let system_started = std::time::Instant::now();
        let mut check = FastSystemSourceCheck {
            system_id: system_id.to_string(),
            status: FastSourceCheckStatus::Rescan,
            directories_checked: 0,
            containers_checked: 0,
            elapsed_us: 0,
            reason: String::new(),
        };
        if request == FastCatalogRefreshRequest::RebuildAll {
            check.reason = "explicit rebuild-all".to_string();
        } else if !binding_matches {
            check.reason = "refresh state is not bound to the active catalog".to_string();
        } else if let Some(watch) = watch_indices.get(system_id) {
            check_watch_index(watch, &metadata_cache, &mut check);
        } else if let Some(error) = watch_errors.get(system_id) {
            check.reason = format!("watch index unavailable: {error}");
        } else if references.contains_key(system_id) {
            check.reason = "watch index unavailable".to_string();
        } else {
            check.reason = "system source snapshot is missing".to_string();
        }
        check.elapsed_us = system_started
            .elapsed()
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX);
        check
    };
    let discovery_needed = request == FastCatalogRefreshRequest::RebuildAll
        || !binding_matches
        || !discovery_anchors_unchanged(
            &discovery_anchor_paths,
            watch_indices.values(),
            &metadata_cache,
        );
    let phase_started = std::time::Instant::now();
    let mut systems = active
        .systems
        .iter()
        .map(|system| system.system_id.as_str().to_string())
        .collect::<Vec<_>>();
    if discovery_needed {
        systems.extend(crate::fast_catalog_sources::discover_independent_system_ids(storage_root)?);
    }
    let system_discovery_us = phase_started
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX);
    systems.sort();
    systems.dedup();
    let phase_started = std::time::Instant::now();
    let checks = systems
        .iter()
        .map(|system_id| build_check(system_id))
        .collect::<Vec<_>>();
    let checks_us = phase_started
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX);
    let unchanged = checks
        .iter()
        .filter(|check| check.status == FastSourceCheckStatus::Unchanged)
        .count();
    let changed = checks
        .iter()
        .filter(|check| check.status == FastSourceCheckStatus::Changed)
        .count();
    let rescans = checks.len().saturating_sub(unchanged + changed);
    Ok(FastRefreshPlanReport {
        elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        manifest_read_us,
        active_read_us,
        system_discovery_us,
        checks_us,
        watch_read_us,
        metadata_probe_us,
        metadata_parents,
        metadata_paths,
        systems: checks.len(),
        unchanged,
        changed,
        rescans,
        artifact_writes: 0,
        checks,
        previous_manifest: Some(manifest),
        active_manifest: Some(active),
    })
}

pub fn execute_fast_refresh(
    storage_root: &Path,
    catalog_root: &Path,
    request: FastCatalogRefreshRequest,
) -> Result<FastCatalogRefreshReport, String> {
    let plan = plan_fast_refresh(storage_root, catalog_root, request)?;
    execute_planned_fast_refresh(storage_root, catalog_root, request, plan)
}

pub fn execute_planned_fast_refresh(
    storage_root: &Path,
    catalog_root: &Path,
    request: FastCatalogRefreshRequest,
    plan: FastRefreshPlanReport,
) -> Result<FastCatalogRefreshReport, String> {
    execute_planned_fast_refresh_with(
        storage_root,
        catalog_root,
        request,
        plan,
        prepare_system_refresh,
    )
}

struct PreparedSystemRefresh {
    system: FastFiveSystem,
    source_report: crate::fast_catalog_sources::FastSourceSystemReport,
    state: FastRefreshSystemState,
    row_fingerprint: String,
}

fn prepare_system_refresh(
    storage_root: &Path,
    snapshot: &FastFiveSnapshot,
    system_id: &str,
) -> Result<Option<PreparedSystemRefresh>, String> {
    let Some((system, source_report)) =
        crate::fast_catalog_sources::rebuild_independent_system(storage_root, snapshot, system_id)?
    else {
        return Ok(None);
    };
    let watch = capture_system_watch(storage_root, system_id)?;
    let row_fingerprint =
        row_fingerprint_parts(&system.system_id, &system.games, &system.variants)?;
    let games = system.games.len().try_into().unwrap_or(u64::MAX);
    let variants = system.variants.len().try_into().unwrap_or(u64::MAX);
    Ok(Some(PreparedSystemRefresh {
        system,
        source_report,
        state: FastRefreshSystemState {
            watch,
            row_fingerprint: row_fingerprint.clone(),
            games,
            variants,
        },
        row_fingerprint,
    }))
}

fn execute_planned_fast_refresh_with(
    storage_root: &Path,
    catalog_root: &Path,
    request: FastCatalogRefreshRequest,
    plan: FastRefreshPlanReport,
    mut prepare_system: impl FnMut(
        &Path,
        &FastFiveSnapshot,
        &str,
    ) -> Result<Option<PreparedSystemRefresh>, String>,
) -> Result<FastCatalogRefreshReport, String> {
    let started = std::time::Instant::now();
    let planning_us = plan.elapsed_us;
    let previous = match &plan.previous_manifest {
        Some(manifest) => manifest.clone(),
        None => read_latest_refresh_manifest(catalog_root)?,
    };
    let active = match &plan.active_manifest {
        Some(manifest) => manifest.clone(),
        None => crate::shard_registry::read_latest_manifest_lazy(
            catalog_root,
            crate::shard_registry::production_registry_limits(),
        )
        .map_err(|error| format!("read active fast catalog: {error}"))?,
    };
    let mut snapshot = FastFiveSnapshot {
        schema: crate::fast_five_catalog::FAST_FIVE_SNAPSHOT_SCHEMA.to_string(),
        source_fingerprint: "0".repeat(64),
        systems: active
            .systems
            .iter()
            .map(|system| FastFiveSystem {
                system_id: system.system_id.as_str().to_string(),
                display_title: system.display_title.clone(),
                games: Vec::new(),
                variants: Vec::new(),
            })
            .collect(),
    };
    snapshot
        .systems
        .sort_by(|left, right| left.system_id.cmp(&right.system_id));
    snapshot.validate()?;
    let previous_refs = previous
        .systems
        .iter()
        .map(|reference| (reference.system_id.as_str(), reference))
        .collect::<BTreeMap<_, _>>();
    let source_started = std::time::Instant::now();
    let mut updated_states = Vec::new();
    let mut artifact_changes = BTreeSet::new();
    let mut artifact_writes = BTreeSet::new();
    let mut removed_system_ids = BTreeSet::new();
    let mut reports = Vec::with_capacity(plan.systems);
    for check in &plan.checks {
        let system_started = std::time::Instant::now();
        let previous_ref = previous_refs.get(check.system_id.as_str()).copied();
        if check.status == FastSourceCheckStatus::Unchanged {
            reports.push(FastCatalogSystemRefreshReport {
                system_id: check.system_id.clone(),
                outcome: FastCatalogSystemOutcome::Unchanged,
                source_status: check.status,
                games: previous_ref.map_or(0, |reference| reference.games),
                variants: previous_ref.map_or(0, |reference| reference.variants),
                elapsed_us: system_started
                    .elapsed()
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
                detail: "source identities match".to_string(),
            });
            continue;
        }
        let was_active = snapshot
            .systems
            .iter()
            .any(|candidate| candidate.system_id == check.system_id);
        match prepare_system(storage_root, &snapshot, &check.system_id) {
            Ok(Some(prepared)) => {
                let PreparedSystemRefresh {
                    system,
                    source_report,
                    state,
                    row_fingerprint: new_row_fingerprint,
                } = prepared;
                let rows_changed = !was_active
                    || previous_ref
                        .is_none_or(|reference| reference.row_fingerprint != new_row_fingerprint);
                if rows_changed {
                    artifact_changes.insert(check.system_id.clone());
                    artifact_writes.insert(check.system_id.clone());
                }
                if let Some(target) = snapshot
                    .systems
                    .iter_mut()
                    .find(|candidate| candidate.system_id == check.system_id)
                {
                    *target = system;
                } else {
                    snapshot.systems.push(system);
                    snapshot
                        .systems
                        .sort_by(|left, right| left.system_id.cmp(&right.system_id));
                }
                reports.push(FastCatalogSystemRefreshReport {
                    system_id: check.system_id.clone(),
                    outcome: if rows_changed {
                        FastCatalogSystemOutcome::Updated
                    } else {
                        FastCatalogSystemOutcome::Unchanged
                    },
                    source_status: check.status,
                    games: state.games,
                    variants: state.variants,
                    elapsed_us: system_started
                        .elapsed()
                        .as_micros()
                        .try_into()
                        .unwrap_or(u64::MAX),
                    detail: format!(
                        "rescanned {} files; canonical rows {}",
                        source_report.files_visited,
                        if rows_changed { "changed" } else { "unchanged" }
                    ),
                });
                updated_states.push(state);
            }
            Ok(None) => {
                snapshot
                    .systems
                    .retain(|candidate| candidate.system_id != check.system_id);
                if was_active {
                    artifact_changes.insert(check.system_id.clone());
                    removed_system_ids.insert(check.system_id.clone());
                }
                reports.push(FastCatalogSystemRefreshReport {
                    system_id: check.system_id.clone(),
                    outcome: if was_active {
                        FastCatalogSystemOutcome::Removed
                    } else {
                        FastCatalogSystemOutcome::Unchanged
                    },
                    source_status: check.status,
                    games: 0,
                    variants: 0,
                    elapsed_us: system_started
                        .elapsed()
                        .as_micros()
                        .try_into()
                        .unwrap_or(u64::MAX),
                    detail: "no installed launchable source remains".to_string(),
                });
            }
            Err(error) => reports.push(FastCatalogSystemRefreshReport {
                system_id: check.system_id.clone(),
                outcome: FastCatalogSystemOutcome::FailedRetained,
                source_status: check.status,
                games: previous_ref.map_or(0, |reference| reference.games),
                variants: previous_ref.map_or(0, |reference| reference.variants),
                elapsed_us: system_started
                    .elapsed()
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
                detail: error,
            }),
        }
    }
    let source_rebuild_us = source_started
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX);
    let artifact_started = std::time::Instant::now();
    if !artifact_changes.is_empty() {
        crate::fast_five_catalog::publish_changed_snapshot_with_profile(
            catalog_root,
            &snapshot,
            &artifact_changes,
            crate::shard_registry::production_registry_limits(),
            fast_catalog_artifact_profile(),
        )?;
    }
    let artifact_publish_us = artifact_started
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX);
    let snapshot_started = std::time::Instant::now();
    let refresh_generation = if updated_states.is_empty() && removed_system_ids.is_empty() {
        previous.generation
    } else {
        let active = crate::shard_registry::read_latest_manifest_lazy(
            catalog_root,
            crate::shard_registry::production_registry_limits(),
        )
        .map_err(|error| format!("read refreshed fast catalog: {error}"))?;
        publish_refresh_update(
            catalog_root,
            &previous,
            active.generation,
            crate::fast_five_catalog::registry_fingerprint_for_manifest(&active),
            &updated_states,
            &removed_system_ids,
        )?
        .generation
    };
    let snapshot_publish_us = snapshot_started
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX);
    let unchanged = reports
        .iter()
        .filter(|report| report.outcome == FastCatalogSystemOutcome::Unchanged)
        .count();
    let updated = reports
        .iter()
        .filter(|report| report.outcome == FastCatalogSystemOutcome::Updated)
        .count();
    let removed = reports
        .iter()
        .filter(|report| report.outcome == FastCatalogSystemOutcome::Removed)
        .count();
    let failed_retained = reports.len().saturating_sub(unchanged + updated + removed);
    Ok(FastCatalogRefreshReport {
        request,
        elapsed_us: planning_us
            .saturating_add(started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)),
        planning_us,
        source_rebuild_us,
        artifact_publish_us,
        snapshot_publish_us,
        systems: reports.len(),
        unchanged,
        updated,
        removed,
        failed_retained,
        artifact_systems_written: artifact_writes.len(),
        catalog_generation: active.generation,
        refresh_generation,
        games: active
            .systems
            .iter()
            .map(|system| system.active.games)
            .sum(),
        system_reports: reports,
        plan,
    })
}

fn check_watch_index(
    watch: &FastSystemWatchIndex,
    metadata_cache: &HashMap<PathBuf, Option<crate::namespace_walk::KnownPathMetadata>>,
    check: &mut FastSystemSourceCheck,
) {
    let known_directories = watch
        .directories
        .iter()
        .map(|directory| directory.path.as_str())
        .collect::<BTreeSet<_>>();
    for root in &watch.roots {
        let observed_is_dir = metadata_cache
            .get(Path::new(root))
            .and_then(|metadata| *metadata)
            .is_some_and(|metadata| metadata.is_dir);
        if observed_is_dir != known_directories.contains(root.as_str()) {
            check.status = FastSourceCheckStatus::Changed;
            check.reason = format!("root availability changed: {root}");
            return;
        }
    }
    for directory in &watch.directories {
        check.directories_checked = check.directories_checked.saturating_add(1);
        let path = Path::new(&directory.path);
        let Some(observed) = metadata_cache.get(path).and_then(|metadata| *metadata) else {
            check.status = FastSourceCheckStatus::Changed;
            check.reason = format!("directory unavailable: {}", directory.path);
            return;
        };
        if !observed.is_dir || observed.modified_ns != directory.modified_ns {
            check.status = FastSourceCheckStatus::Changed;
            check.reason = format!("directory entries changed: {}", directory.path);
            return;
        }
    }
    for container in &watch.containers {
        check.containers_checked = check.containers_checked.saturating_add(1);
        let path = Path::new(&container.path);
        let Some(observed) = metadata_cache.get(path).and_then(|metadata| *metadata) else {
            check.status = FastSourceCheckStatus::Changed;
            check.reason = format!("container unavailable: {}", container.path);
            return;
        };
        if !observed.is_file
            || observed.size != container.size
            || observed.modified_ns != container.modified_ns
        {
            check.status = FastSourceCheckStatus::Changed;
            check.reason = format!("container changed: {}", container.path);
            return;
        }
    }
    check.status = FastSourceCheckStatus::Unchanged;
    check.reason = "source identities match".to_string();
}

fn build_watch_metadata_cache<'a>(
    watches: impl Iterator<Item = &'a FastSystemWatchIndex>,
    extra_paths: &[PathBuf],
) -> (
    HashMap<PathBuf, Option<crate::namespace_walk::KnownPathMetadata>>,
    usize,
    usize,
) {
    let mut grouped = BTreeMap::<PathBuf, BTreeSet<PathBuf>>::new();
    let mut add_path = |path: PathBuf| {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            grouped
                .entry(parent.to_path_buf())
                .or_default()
                .insert(path);
        }
    };
    for watch in watches {
        for path in watch
            .roots
            .iter()
            .chain(watch.directories.iter().map(|entry| &entry.path))
            .chain(watch.containers.iter().map(|entry| &entry.path))
            .map(PathBuf::from)
        {
            add_path(path);
        }
    }
    for path in extra_paths {
        add_path(path.clone());
    }
    let metadata_parents = grouped.len();
    let mut cache = HashMap::new();
    for (parent, paths) in grouped {
        let paths = paths.into_iter().collect::<Vec<_>>();
        let observations = crate::namespace_walk::probe_known_path_metadata(&parent, &paths);
        cache.extend(paths.into_iter().zip(observations));
    }
    let metadata_paths = cache.len();
    (cache, metadata_parents, metadata_paths)
}

fn discovery_anchor_paths(storage_root: &Path) -> Vec<PathBuf> {
    vec![
        storage_root.join("games"),
        storage_root.join("_Arcade"),
        storage_root.join("_DOS Games"),
        storage_root.join("_Computer"),
    ]
}

fn discovery_anchors_unchanged<'a>(
    anchors: &[PathBuf],
    watches: impl Iterator<Item = &'a FastSystemWatchIndex>,
    metadata_cache: &HashMap<PathBuf, Option<crate::namespace_walk::KnownPathMetadata>>,
) -> bool {
    let expected = watches
        .flat_map(|watch| watch.directories.iter())
        .map(|directory| (PathBuf::from(&directory.path), directory))
        .collect::<BTreeMap<_, _>>();
    anchors.iter().all(|path| {
        let observed = metadata_cache.get(path).and_then(|metadata| *metadata);
        match expected.get(path) {
            Some(directory) => observed.is_some_and(|metadata| {
                metadata.is_dir && metadata.modified_ns == directory.modified_ns
            }),
            None => observed.is_none(),
        }
    })
}

#[derive(Debug)]
struct WatchSpecification {
    scan_roots: Vec<PathBuf>,
    anchors: Vec<PathBuf>,
}

fn watch_specification_from_profiles(
    storage_root: &Path,
    system_id: &str,
    profiles: &[crate::launch_profiles::LaunchProfile],
) -> Result<WatchSpecification, String> {
    let games = storage_root.join("games");
    let (mut scan_roots, mut core_parents) = match system_id {
        "amiga" => (
            vec![games.join("Amiga")],
            vec![storage_root.join("_Computer")],
        ),
        "arcade" => (
            vec![
                storage_root.join("_Arcade"),
                games.join("mame"),
                games.join("hbmame"),
            ],
            vec![storage_root.join("_Arcade/cores")],
        ),
        "c64" => (
            vec![games.join("C64")],
            vec![storage_root.join("_Computer")],
        ),
        "dos" => (
            vec![storage_root.join("_DOS Games"), games.join("AO486")],
            vec![storage_root.join("_Computer")],
        ),
        "x68000" => (
            vec![
                storage_root.join("_Computer/_X68000 Games"),
                storage_root.join("_Computer/X68000 Games"),
                games.join("X68000"),
            ],
            vec![storage_root.join("_Computer")],
        ),
        _ => (Vec::new(), Vec::new()),
    };
    for profile in profiles
        .iter()
        .filter(|profile| profile.system_id == system_id)
    {
        scan_roots.extend(
            profile
                .game_dirs
                .iter()
                .map(|game_dir| games.join(game_dir)),
        );
        if let Some(parent) = profile
            .core_path
            .as_deref()
            .map(Path::new)
            .and_then(Path::parent)
        {
            core_parents.push(storage_root.join(parent));
        }
    }
    scan_roots.sort();
    scan_roots.dedup();
    if scan_roots.is_empty() {
        return Err(format!("no watch roots for catalog system {system_id}"));
    }
    let mut anchors = vec![games];
    anchors.append(&mut core_parents);
    anchors.sort();
    anchors.dedup();
    Ok(WatchSpecification {
        scan_roots,
        anchors,
    })
}

fn capture_tree(
    root: &Path,
    system_id: &str,
    directories: &mut Vec<FastWatchedDirectory>,
    containers: &mut Vec<FastWatchedContainer>,
) -> Result<(), String> {
    let mut visited = 0usize;
    capture_tree_at_depth(root, system_id, directories, containers, 0, &mut visited)
}

fn capture_tree_at_depth(
    root: &Path,
    system_id: &str,
    directories: &mut Vec<FastWatchedDirectory>,
    containers: &mut Vec<FastWatchedContainer>,
    depth: usize,
    visited: &mut usize,
) -> Result<(), String> {
    if depth > MAX_WATCH_DEPTH {
        return Err(format!(
            "watch snapshot exceeded directory depth limit {} at {}",
            MAX_WATCH_DEPTH,
            root.display()
        ));
    }
    let metadata = fs::metadata(root)
        .map_err(|error| format!("stat watch directory {}: {error}", root.display()))?;
    let mut entries = fs::read_dir(root)
        .map_err(|error| format!("read watch directory {}: {error}", root.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("enumerate watch directory {}: {error}", root.display()))?;
    entries.sort_by(|left, right| {
        let left = left.file_name().to_string_lossy().into_owned();
        let right = right.file_name().to_string_lossy().into_owned();
        left.to_ascii_lowercase()
            .cmp(&right.to_ascii_lowercase())
            .then_with(|| left.cmp(&right))
    });
    let mut digest = Sha256::new();
    for entry in entries {
        *visited = visited.saturating_add(1);
        if *visited > MAX_WATCH_ENTRIES {
            return Err(format!(
                "watch snapshot exceeded {} entries",
                MAX_WATCH_ENTRIES
            ));
        }
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("inspect {}: {error}", path.display()))?;
        if should_ignore_path(&path) || (file_type.is_dir() && should_prune_source_directory(&path))
        {
            continue;
        }
        let kind = if file_type.is_dir() {
            b'd'
        } else if file_type.is_file() {
            b'f'
        } else if file_type.is_symlink() {
            b'l'
        } else {
            b'o'
        };
        digest.update([kind]);
        digest.update(entry.file_name().to_string_lossy().as_bytes());
        digest.update([0]);
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if let Err(error) = capture_tree_at_depth(
                &path,
                system_id,
                directories,
                containers,
                depth.saturating_add(1),
                visited,
            ) && path.exists()
            {
                return Err(error);
            }
        } else if file_type.is_file() && is_watched_container(system_id, &path) {
            match capture_container(&path) {
                Ok(container) => containers.push(container),
                Err(error) if path.exists() => return Err(error),
                Err(_) => {}
            }
        }
    }
    directories.push(FastWatchedDirectory {
        path: root.to_string_lossy().into_owned(),
        modified_ns: modified_ns(&metadata),
        entry_fingerprint: sha256_digest_hex(digest.finalize()),
    });
    Ok(())
}

fn capture_directory(path: &Path) -> Result<FastWatchedDirectory, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("stat watch directory {}: {error}", path.display()))?;
    let raw_entries = fs::read_dir(path)
        .map_err(|error| format!("read watch directory {}: {error}", path.display()))?;
    let mut raw_count = 0usize;
    let mut entries = raw_entries
        .take(MAX_WATCH_DIRECTORY_ENTRIES.saturating_add(1))
        .map(|entry| {
            raw_count = raw_count.saturating_add(1);
            entry
        })
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let entry_path = entry.path();
            let kind = entry.file_type().ok()?;
            if should_ignore_path(&entry_path)
                || (kind.is_dir() && should_prune_source_directory(&entry_path))
            {
                return None;
            }
            let kind = if kind.is_dir() {
                b'd'
            } else if kind.is_file() {
                b'f'
            } else if kind.is_symlink() {
                b'l'
            } else {
                b'o'
            };
            Some((entry.file_name().to_string_lossy().into_owned(), kind))
        })
        .collect::<Vec<_>>();
    if raw_count > MAX_WATCH_DIRECTORY_ENTRIES {
        return Err(format!(
            "watch directory {} exceeds {} entries",
            path.display(),
            MAX_WATCH_DIRECTORY_ENTRIES
        ));
    }
    entries.sort_by(|left, right| {
        left.0
            .to_ascii_lowercase()
            .cmp(&right.0.to_ascii_lowercase())
            .then_with(|| left.0.cmp(&right.0))
    });
    let mut digest = Sha256::new();
    for (name, kind) in entries {
        digest.update([kind]);
        digest.update(name.as_bytes());
        digest.update([0]);
    }
    Ok(FastWatchedDirectory {
        path: path.to_string_lossy().into_owned(),
        modified_ns: modified_ns(&metadata),
        entry_fingerprint: sha256_digest_hex(digest.finalize()),
    })
}

fn capture_container(path: &Path) -> Result<FastWatchedContainer, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("stat watched container {}: {error}", path.display()))?;
    let mut digest = Sha256::new();
    digest.update(path.to_string_lossy().as_bytes());
    digest.update(metadata.len().to_le_bytes());
    digest.update(modified_ns(&metadata).to_le_bytes());
    digest.update(changed_ns(&metadata).to_le_bytes());
    digest.update(inode(&metadata).to_le_bytes());
    Ok(FastWatchedContainer {
        path: path.to_string_lossy().into_owned(),
        size: metadata.len(),
        modified_ns: modified_ns(&metadata),
        changed_ns: changed_ns(&metadata),
        inode: inode(&metadata),
        content_directory_fingerprint: sha256_digest_hex(digest.finalize()),
    })
}

fn core_profile_fingerprint(
    anchors: &[PathBuf],
    anchor_cache: &BTreeMap<PathBuf, FastWatchedDirectory>,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"mister-magik-fast-source-adapter-v1\0");
    for anchor in anchors {
        digest.update(anchor.to_string_lossy().as_bytes());
        digest.update([u8::from(anchor.is_dir())]);
        if let Some(directory) = anchor_cache.get(anchor) {
            digest.update(directory.entry_fingerprint.as_bytes());
        }
    }
    sha256_digest_hex(digest.finalize())
}

fn is_watched_container(system_id: &str, path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match system_id {
        "arcade" | "c64" => false,
        "amiga" => extension.is_some_and(|extension| matches!(extension.as_str(), "7z" | "txt")),
        "dos" | "x68000" => extension.is_some_and(|extension| extension == "mgl"),
        _ => extension.is_some_and(|extension| extension == "zip"),
    }
}

fn should_prune_source_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            [
                ".____padding_file",
                "__macosx",
                "images",
                "manuals",
                "media",
                "cores",
                "screenshot",
                "screenshots",
                "screenshot-magik",
                "_organized",
                "boxart",
            ]
            .iter()
            .any(|ignored| name.eq_ignore_ascii_case(ignored))
        })
}

fn modified_ns(metadata: &fs::Metadata) -> i128 {
    system_time_ns(metadata.modified().ok())
}

#[cfg(unix)]
fn changed_ns(metadata: &fs::Metadata) -> i128 {
    use std::os::unix::fs::MetadataExt;
    i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec())
}

#[cfg(not(unix))]
fn changed_ns(_metadata: &fs::Metadata) -> i128 {
    0
}

#[cfg(unix)]
fn inode(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.ino()
}

#[cfg(not(unix))]
fn inode(_metadata: &fs::Metadata) -> u64 {
    0
}

fn system_time_ns(value: Option<SystemTime>) -> i128 {
    value
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| {
            i128::from(duration.as_secs()) * 1_000_000_000 + i128::from(duration.subsec_nanos())
        })
}

fn sha256_digest_hex(bytes: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

pub fn source_fingerprint(watch: &FastSystemWatchIndex) -> String {
    let bytes = postcard::to_allocvec(watch).expect("serializable watch index");
    sha256_hex(&bytes)
}

pub fn row_fingerprint(rows: &FastSystemRowsSnapshot) -> Result<String, String> {
    row_fingerprint_parts(&rows.system_id, &rows.games, &rows.variants)
}

pub(crate) fn encode_row_fingerprint_payload_for_system(
    system: &FastFiveSystem,
) -> Result<Vec<u8>, String> {
    encode_row_fingerprint_payload(&system.system_id, &system.games, &system.variants)
}

fn row_fingerprint_parts(
    system_id: &str,
    games: &[SystemGame],
    variants: &[FastFiveGameVariant],
) -> Result<String, String> {
    validate_row_parts(system_id, games, variants)?;
    encode_row_fingerprint_payload(system_id, games, variants).map(|bytes| sha256_hex(&bytes))
}

fn encode_row_fingerprint_payload(
    system_id: &str,
    games: &[SystemGame],
    variants: &[FastFiveGameVariant],
) -> Result<Vec<u8>, String> {
    let rows = FastSystemRowsSnapshotRef {
        schema: REFRESH_SCHEMA,
        system_id,
        games,
        variants,
    };
    postcard::to_allocvec(&rows).map_err(|error| format!("encode row fingerprint: {error}"))
}

fn validate_row_parts(
    system_id: &str,
    games: &[SystemGame],
    variants: &[FastFiveGameVariant],
) -> Result<(), String> {
    let prefix = format!("{system_id}\u{1f}");
    let mut keys = BTreeSet::new();
    for game in games {
        if !game.stable_key.starts_with(&prefix)
            || game.title.trim().is_empty()
            || game.launch_ref.trim().is_empty()
            || !keys.insert(game.stable_key.as_str())
        {
            return Err(format!("invalid cached row in {system_id}"));
        }
    }
    for variant in variants {
        if !variant.game.stable_key.starts_with(&prefix)
            || !keys.insert(variant.game.stable_key.as_str())
        {
            return Err(format!("invalid cached variant in {system_id}"));
        }
    }
    Ok(())
}

fn encode_envelope<T: Serialize>(value: &T, magic: &[u8; 8]) -> Result<Vec<u8>, String> {
    encode_envelope_with_payload_fingerprint(value, magic).map(|(bytes, _)| bytes)
}

fn encode_envelope_with_payload_fingerprint<T: Serialize>(
    value: &T,
    magic: &[u8; 8],
) -> Result<(Vec<u8>, String), String> {
    let payload = postcard::to_allocvec(value).map_err(|error| format!("encode state: {error}"))?;
    let payload_len = u64::try_from(payload.len()).map_err(|_| "state is too large")?;
    let payload_digest = Sha256::digest(&payload);
    let payload_fingerprint = sha256_digest_hex(payload_digest);
    let mut output = Vec::with_capacity(ENVELOPE_BYTES + payload.len());
    output.extend_from_slice(magic);
    output.extend_from_slice(&ENVELOPE_VERSION.to_le_bytes());
    output.extend_from_slice(&REFRESH_SCHEMA.to_le_bytes());
    output.extend_from_slice(&payload_len.to_le_bytes());
    output.extend_from_slice(&payload_digest);
    output.extend_from_slice(&[0; 8]);
    debug_assert_eq!(output.len(), ENVELOPE_BYTES);
    output.extend_from_slice(&payload);
    Ok((output, payload_fingerprint))
}

fn read_envelope<T: DeserializeOwned>(
    path: &Path,
    magic: &[u8; 8],
    maximum: usize,
) -> Result<T, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("stat {}: {error}", path.display()))?;
    if metadata.len() > maximum.try_into().unwrap_or(u64::MAX) {
        return Err(format!("{} exceeds its size bound", path.display()));
    }
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    decode_envelope(&bytes, magic, maximum)
}

fn decode_envelope<T: DeserializeOwned>(
    bytes: &[u8],
    magic: &[u8; 8],
    maximum: usize,
) -> Result<T, String> {
    if bytes.len() > maximum || bytes.len() < ENVELOPE_BYTES {
        return Err("fast refresh state length is invalid".to_string());
    }
    let header = &bytes[..ENVELOPE_BYTES];
    if &header[..8] != magic
        || u32::from_le_bytes(header[8..12].try_into().expect("version bytes")) != ENVELOPE_VERSION
        || u32::from_le_bytes(header[12..16].try_into().expect("schema bytes")) != REFRESH_SCHEMA
    {
        return Err("fast refresh state header is invalid".to_string());
    }
    let payload_len = usize::try_from(u64::from_le_bytes(
        header[16..24].try_into().expect("length bytes"),
    ))
    .map_err(|_| "fast refresh payload is too large")?;
    let payload = &bytes[ENVELOPE_BYTES..];
    if payload.len() != payload_len || Sha256::digest(payload).as_slice() != &header[24..56] {
        return Err("fast refresh payload checksum differs".to_string());
    }
    postcard::from_bytes(payload).map_err(|error| format!("decode refresh state: {error}"))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.exists() {
        let existing =
            fs::read(path).map_err(|error| format!("read existing {}: {error}", path.display()))?;
        if existing == bytes {
            return Ok(());
        }
        return Err(format!(
            "immutable refresh state already exists: {}",
            path.display()
        ));
    }
    let temporary = path.with_extension(format!(
        "tmp-new-{}",
        crate::catalog_lease::CatalogRunId::new().as_str()
    ));
    let result =
        write_synced(&temporary, bytes).and_then(|()| match fs::hard_link(&temporary, path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = fs::read(path).map_err(|read_error| {
                    format!("read existing {}: {read_error}", path.display())
                })?;
                if existing == bytes {
                    Ok(())
                } else {
                    Err(format!(
                        "immutable refresh state changed concurrently: {}",
                        path.display()
                    ))
                }
            }
            Err(error) => Err(format!("publish immutable {}: {error}", path.display())),
        });
    match result {
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
        Ok(()) => {
            let cleanup = fs::remove_file(&temporary);
            if let Err(error) = cleanup {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(format!(
                        "remove refresh temporary {}: {error}",
                        temporary.display()
                    ));
                }
            }
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
            Ok(())
        }
    }
}

fn write_replace_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!(
        "tmp-{}",
        crate::catalog_lease::CatalogRunId::new().as_str()
    ));
    let result = write_synced(&temporary, bytes).and_then(|()| {
        fs::rename(&temporary, path).map_err(|error| format!("publish {}: {error}", path.display()))
    });
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create(true);
    options.create_new(true);
    let mut file = options
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write {}: {error}", path.display()))
}

fn sync_filesystem(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let directory = File::open(path)
            .map_err(|error| format!("open filesystem {}: {error}", path.display()))?;
        let result = unsafe { libc::syncfs(directory.as_raw_fd()) };
        if result != 0 {
            return Err(format!(
                "sync filesystem {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        sync_directory(path)
    }
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("sync {}: {error}", path.display()))
}

fn verify_file_checksum(bytes: &[u8], expected: &str, path: &Path) -> Result<(), String> {
    if sha256_hex(bytes) == expected {
        Ok(())
    } else {
        Err(format!("{} checksum differs", path.display()))
    }
}

fn validate_relative_path(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        })
    {
        Err(format!("unsafe refresh state path {value}"))
    } else {
        Ok(())
    }
}

fn validate_unique_paths<'a>(
    paths: impl IntoIterator<Item = &'a str>,
    label: &str,
) -> Result<(), String> {
    let mut unique = BTreeSet::new();
    for path in paths {
        if path.trim().is_empty() || !unique.insert(path) {
            return Err(format!("invalid or duplicate {label}: {path}"));
        }
    }
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> Result<(), String> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(format!("{label} is not SHA-256"))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_five_catalog::FastFiveVariantRelation;

    fn state(system_id: &str) -> FastRefreshSystemState {
        let game = SystemGame {
            stable_key: format!("{system_id}\u{1f}game"),
            title: "Game".to_string(),
            launch_ref: "/media/fat/games/Game.rom".to_string(),
            preview_archive_path: String::new(),
            preview_asset_key: String::new(),
            has_preview: false,
            year: None,
            manufacturer: String::new(),
            category: "Games".to_string(),
            players: None,
            control: String::new(),
            is_new: false,
            launch_plan: None,
        };
        let rows = FastSystemRowsSnapshot {
            schema: REFRESH_SCHEMA,
            system_id: system_id.to_string(),
            games: vec![game.clone()],
            variants: vec![FastFiveGameVariant {
                family_stable_key: game.stable_key.clone(),
                relation: FastFiveVariantRelation::LanguageEdition,
                game: SystemGame {
                    stable_key: format!("{system_id}\u{1f}variant"),
                    ..game
                },
            }],
        };
        let row_fingerprint = row_fingerprint(&rows).unwrap();
        FastRefreshSystemState {
            watch: FastSystemWatchIndex {
                schema: REFRESH_SCHEMA,
                system_id: system_id.to_string(),
                adapter_version: 1,
                core_profile_fingerprint: "1".repeat(64),
                roots: vec!["/media/fat/games".to_string()],
                directories: vec![FastWatchedDirectory {
                    path: "/media/fat/games".to_string(),
                    modified_ns: 7,
                    entry_fingerprint: "2".repeat(64),
                }],
                containers: Vec::new(),
            },
            row_fingerprint,
            games: rows.games.len() as u64,
            variants: rows.variants.len() as u64,
        }
    }

    #[test]
    fn publishes_two_slot_manifest_and_separate_system_state() {
        let root = crate::test_support::unique_temp_dir("fast-refresh-state");
        let first = publish_refresh_state(
            &root,
            1,
            10,
            "a".repeat(64),
            "builder-1".to_string(),
            &[state("snes")],
        )
        .expect("publish first state");
        assert!(refresh_state_root(&root).join(MANIFEST_B).is_file());
        assert_eq!(read_latest_refresh_manifest(&root).unwrap(), first);
        let reference = &first.systems[0];
        assert_eq!(
            read_system_watch(&root, reference).unwrap().system_id,
            "snes"
        );
        assert!(!reference.watch_path.is_empty());
        assert!(
            !refresh_state_root(&root)
                .join("systems/snes/1.rows")
                .exists()
        );
        assert_eq!(reference.row_fingerprint, state("snes").row_fingerprint);

        let second = publish_refresh_state(
            &root,
            2,
            11,
            "b".repeat(64),
            "builder-1".to_string(),
            &[state("snes")],
        )
        .expect("publish second state");
        assert!(refresh_state_root(&root).join(MANIFEST_A).is_file());
        assert_eq!(read_latest_refresh_manifest(&root).unwrap(), second);
    }

    #[test]
    fn borrowed_row_fingerprint_encoding_matches_owned_snapshot() {
        let state = state("snes");
        let game = SystemGame {
            stable_key: "snes\u{1f}game".to_string(),
            title: "Game".to_string(),
            launch_ref: "/media/fat/games/Game.rom".to_string(),
            preview_archive_path: String::new(),
            preview_asset_key: String::new(),
            has_preview: false,
            year: None,
            manufacturer: String::new(),
            category: "Games".to_string(),
            players: None,
            control: String::new(),
            is_new: false,
            launch_plan: None,
        };
        let rows = FastSystemRowsSnapshot {
            schema: REFRESH_SCHEMA,
            system_id: "snes".to_string(),
            games: vec![game.clone()],
            variants: vec![FastFiveGameVariant {
                family_stable_key: game.stable_key.clone(),
                relation: FastFiveVariantRelation::LanguageEdition,
                game: SystemGame {
                    stable_key: "snes\u{1f}variant".to_string(),
                    ..game
                },
            }],
        };
        let borrowed = FastSystemRowsSnapshotRef {
            schema: rows.schema,
            system_id: &rows.system_id,
            games: &rows.games,
            variants: &rows.variants,
        };
        assert_eq!(
            postcard::to_allocvec(&rows).unwrap(),
            postcard::to_allocvec(&borrowed).unwrap()
        );
        assert_eq!(state.row_fingerprint, row_fingerprint(&rows).unwrap());
    }

    #[test]
    fn rejects_corrupt_envelopes_and_unsafe_paths() {
        let value = state("snes").watch;
        let mut encoded = encode_envelope(&value, WATCH_MAGIC).unwrap();
        let last = encoded.len() - 1;
        encoded[last] ^= 0x55;
        assert!(
            decode_envelope::<FastSystemWatchIndex>(&encoded, WATCH_MAGIC, MAX_WATCH_BYTES)
                .is_err()
        );

        let manifest = FastRefreshManifest::new(
            1,
            1,
            "a".repeat(64),
            "builder".to_string(),
            vec![FastRefreshSystemRef {
                system_id: "snes".to_string(),
                watch_path: "../watch".to_string(),
                watch_sha256: "b".repeat(64),
                source_fingerprint: "d".repeat(64),
                row_fingerprint: "e".repeat(64),
                games: 0,
                variants: 0,
            }],
        );
        assert!(manifest.is_err());
    }

    #[test]
    fn bounded_decoder_rejects_oversized_state() {
        let encoded = vec![0; MAX_MANIFEST_BYTES + 1];
        assert!(
            decode_envelope::<FastRefreshManifest>(&encoded, MANIFEST_MAGIC, MAX_MANIFEST_BYTES)
                .is_err()
        );
    }

    #[test]
    fn watch_check_is_metadata_only_when_sources_are_unchanged() {
        let root = crate::test_support::unique_temp_dir("fast-refresh-check");
        let games = root.join("games/SNES");
        fs::create_dir_all(root.join("_Console")).unwrap();
        fs::create_dir_all(&games).unwrap();
        fs::write(root.join("_Console/SNES.rbf"), b"core").unwrap();
        fs::write(games.join("Game.sfc"), b"rom").unwrap();
        let watch = capture_system_watch(&root, "snes").unwrap();
        let mut check = FastSystemSourceCheck {
            system_id: "snes".to_string(),
            status: FastSourceCheckStatus::Rescan,
            directories_checked: 0,
            containers_checked: 0,
            elapsed_us: 0,
            reason: String::new(),
        };
        let (metadata_cache, _, _) = build_watch_metadata_cache(std::iter::once(&watch), &[]);
        check_watch_index(&watch, &metadata_cache, &mut check);
        assert_eq!(check.status, FastSourceCheckStatus::Unchanged);
        assert_eq!(check.directories_checked, watch.directories.len());
        assert_eq!(check.containers_checked, 0);
    }

    #[test]
    fn watch_check_detects_container_replacement_without_row_reads() {
        let root = crate::test_support::unique_temp_dir("fast-refresh-container-check");
        let games = root.join("games/SNES");
        fs::create_dir_all(root.join("_Console")).unwrap();
        fs::create_dir_all(&games).unwrap();
        fs::write(root.join("_Console/SNES.rbf"), b"core").unwrap();
        let archive = games.join("Games.zip");
        fs::write(&archive, b"first").unwrap();
        let watch = capture_system_watch(&root, "snes").unwrap();
        fs::write(&archive, b"replacement with a different size").unwrap();
        let mut check = FastSystemSourceCheck {
            system_id: "snes".to_string(),
            status: FastSourceCheckStatus::Rescan,
            directories_checked: 0,
            containers_checked: 0,
            elapsed_us: 0,
            reason: String::new(),
        };
        let (metadata_cache, _, _) = build_watch_metadata_cache(std::iter::once(&watch), &[]);
        check_watch_index(&watch, &metadata_cache, &mut check);
        assert_eq!(check.status, FastSourceCheckStatus::Changed);
    }

    #[test]
    fn one_pass_tree_capture_matches_directory_fingerprint_contract() {
        let root = crate::test_support::unique_temp_dir("fast-refresh-one-pass");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("Game.sfc"), b"rom").unwrap();
        fs::write(root.join("nested/Other.sfc"), b"rom").unwrap();
        let expected = capture_directory(&root).unwrap();
        let mut directories = Vec::new();
        let mut containers = Vec::new();
        capture_tree(&root, "snes", &mut directories, &mut containers).unwrap();
        let actual = directories
            .iter()
            .find(|directory| directory.path == root.to_string_lossy())
            .unwrap();
        assert_eq!(actual, &expected);
        assert!(containers.is_empty());
    }

    #[test]
    fn incremental_refresh_adds_then_removes_a_system() {
        let storage = crate::test_support::unique_temp_dir("fast-refresh-membership-storage");
        let catalog = crate::test_support::unique_temp_dir("fast-refresh-membership-catalog");
        fs::create_dir_all(storage.join("_Console")).unwrap();
        fs::create_dir_all(storage.join("games/SNES")).unwrap();
        fs::write(storage.join("_Console/SNES.rbf"), b"core").unwrap();
        fs::write(storage.join("games/SNES/Game.sfc"), b"rom").unwrap();

        build_fresh_catalog(&storage, &catalog).expect("build initial catalog");
        let initial = crate::shard_registry::read_latest_manifest_lazy(
            &catalog,
            crate::shard_registry::production_registry_limits(),
        )
        .unwrap();
        assert!(
            initial
                .systems
                .iter()
                .any(|system| system.system_id.as_str() == "snes")
        );

        fs::create_dir_all(storage.join("games/NES")).unwrap();
        fs::write(storage.join("_Console/NES.rbf"), b"core").unwrap();
        fs::write(storage.join("games/NES/Game.nes"), b"rom").unwrap();
        let added = execute_fast_refresh(&storage, &catalog, FastCatalogRefreshRequest::Update)
            .expect("add NES incrementally");
        assert!(added.system_reports.iter().any(|system| {
            system.system_id == "nes" && system.outcome == FastCatalogSystemOutcome::Updated
        }));
        let after_add = crate::shard_registry::read_latest_manifest_lazy(
            &catalog,
            crate::shard_registry::production_registry_limits(),
        )
        .unwrap();
        assert!(
            after_add
                .systems
                .iter()
                .any(|system| system.system_id.as_str() == "nes")
        );

        let unchanged = execute_fast_refresh(&storage, &catalog, FastCatalogRefreshRequest::Update)
            .expect("check unchanged catalog");
        assert!(unchanged.system_reports.iter().any(|system| {
            system.system_id == "nes" && system.outcome == FastCatalogSystemOutcome::Unchanged
        }));

        fs::remove_dir_all(storage.join("games/NES")).unwrap();
        let removed = execute_fast_refresh(&storage, &catalog, FastCatalogRefreshRequest::Update)
            .expect("remove NES incrementally");
        assert!(removed.system_reports.iter().any(|system| {
            system.system_id == "nes" && system.outcome == FastCatalogSystemOutcome::Removed
        }));
        let after_remove = crate::shard_registry::read_latest_manifest_lazy(
            &catalog,
            crate::shard_registry::production_registry_limits(),
        )
        .unwrap();
        assert!(
            after_remove
                .systems
                .iter()
                .all(|system| system.system_id.as_str() != "nes")
        );
        assert!(
            read_latest_refresh_manifest(&catalog)
                .unwrap()
                .systems
                .iter()
                .all(|system| system.system_id != "nes")
        );
    }

    #[test]
    fn per_system_preparation_failure_retains_only_that_system() {
        let storage = crate::test_support::unique_temp_dir("fast-refresh-failure-storage");
        let catalog = crate::test_support::unique_temp_dir("fast-refresh-failure-catalog");
        fs::create_dir_all(storage.join("_Console")).unwrap();
        for (system, extension) in [("SNES", "sfc"), ("NES", "nes")] {
            fs::create_dir_all(storage.join("games").join(system)).unwrap();
            fs::write(
                storage.join("_Console").join(format!("{system}.rbf")),
                b"core",
            )
            .unwrap();
            fs::write(
                storage
                    .join("games")
                    .join(system)
                    .join(format!("First.{extension}")),
                b"rom",
            )
            .unwrap();
        }
        build_fresh_catalog(&storage, &catalog).expect("build initial catalog");
        fs::write(storage.join("games/SNES/Second.sfc"), b"rom").unwrap();
        fs::write(storage.join("games/NES/Second.nes"), b"rom").unwrap();

        let plan = plan_fast_refresh(&storage, &catalog, FastCatalogRefreshRequest::RebuildAll)
            .expect("plan rebuild");
        let report = execute_planned_fast_refresh_with(
            &storage,
            &catalog,
            FastCatalogRefreshRequest::RebuildAll,
            plan,
            |storage_root, snapshot, system_id| {
                if system_id == "nes" {
                    let rebuilt = crate::fast_catalog_sources::rebuild_independent_system(
                        storage_root,
                        snapshot,
                        system_id,
                    )?;
                    assert!(rebuilt.is_some());
                    return Err("injected watch capture failure".to_string());
                }
                prepare_system_refresh(storage_root, snapshot, system_id)
            },
        )
        .expect("publish successful systems");
        assert!(report.system_reports.iter().any(|system| {
            system.system_id == "snes" && system.outcome == FastCatalogSystemOutcome::Updated
        }));
        assert!(report.system_reports.iter().any(|system| {
            system.system_id == "nes"
                && system.outcome == FastCatalogSystemOutcome::FailedRetained
                && system.detail == "injected watch capture failure"
        }));

        let active = crate::shard_registry::read_latest_manifest_lazy(
            &catalog,
            crate::shard_registry::production_registry_limits(),
        )
        .unwrap();
        assert_eq!(
            active
                .systems
                .iter()
                .find(|system| system.system_id.as_str() == "snes")
                .unwrap()
                .active
                .games,
            2
        );
        assert_eq!(
            active
                .systems
                .iter()
                .find(|system| system.system_id.as_str() == "nes")
                .unwrap()
                .active
                .games,
            1
        );
        assert_eq!(
            read_latest_refresh_manifest(&catalog)
                .unwrap()
                .systems
                .iter()
                .find(|system| system.system_id == "nes")
                .unwrap()
                .games,
            1
        );
    }

    #[test]
    fn current_build_information_rejects_stale_and_corrupt_records() {
        let storage = crate::test_support::unique_temp_dir("fast-build-info-storage");
        let catalog = crate::test_support::unique_temp_dir("fast-build-info-catalog");
        assert_eq!(read_current_build_info(&catalog).unwrap(), None);
        fs::create_dir_all(storage.join("_Console")).unwrap();
        fs::create_dir_all(storage.join("games/SNES")).unwrap();
        fs::write(storage.join("_Console/SNES.rbf"), b"core").unwrap();
        fs::write(storage.join("games/SNES/Game.sfc"), b"rom").unwrap();

        let report = build_fresh_catalog(&storage, &catalog).expect("build catalog");
        assert!(report.build_info_persisted);
        let current = read_current_build_info(&catalog)
            .expect("read build information")
            .expect("current build information");
        assert_eq!(current.catalog_generation, report.publication.generation);
        assert_eq!(
            current.catalog_fingerprint,
            report.publication.registry_fingerprint
        );

        let mut stale = current;
        stale.catalog_generation = stale.catalog_generation.saturating_add(1);
        publish_build_info(&catalog, &stale).unwrap();
        assert_eq!(read_current_build_info(&catalog).unwrap(), None);

        fs::write(build_info_path(&catalog), b"corrupt").unwrap();
        assert!(read_current_build_info(&catalog).is_err());
        assert_eq!(format_build_elapsed(1_499_999), "1 second");
        assert_eq!(format_build_elapsed(1_500_000), "2 seconds");
    }

    #[test]
    fn immutable_publication_is_atomic_and_never_clobbers_existing_bytes() {
        let root = crate::test_support::unique_temp_dir("fast-publication-atomic");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("packs/1.watchpack");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_new_file(&path, b"first").expect("initial immutable publication");
        assert_eq!(fs::read(&path).unwrap(), b"first");
        assert!(!root.join("packs/1.watchpack.tmp-new").exists());
        assert!(write_new_file(&path, b"second").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"first");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stale_refresh_temporaries_are_recovered_without_touching_state() {
        let catalog = crate::test_support::unique_temp_dir("fast-publication-recovery");
        let root = refresh_state_root(&catalog);
        fs::create_dir_all(root.join("packs")).unwrap();
        fs::write(root.join("packs/old.watchpack.tmp-crashed"), b"partial").unwrap();
        fs::write(root.join("manifest-a.bin"), b"active").unwrap();
        assert_eq!(cleanup_refresh_temporary_files(&catalog).unwrap(), 1);
        assert!(!root.join("packs/old.watchpack.tmp-crashed").exists());
        assert_eq!(fs::read(root.join("manifest-a.bin")).unwrap(), b"active");
        let _ = fs::remove_dir_all(catalog);
    }

    #[test]
    fn watch_snapshot_rejects_pathological_directory_depth() {
        let root = crate::test_support::unique_temp_dir("fast-watch-depth");
        let mut current = root.clone();
        fs::create_dir_all(&current).unwrap();
        for index in 0..=MAX_WATCH_DEPTH {
            current = current.join(format!("d{index}"));
            fs::create_dir(&current).unwrap();
        }
        let mut directories = Vec::new();
        let mut containers = Vec::new();
        let error = capture_tree(&root, "snes", &mut directories, &mut containers)
            .expect_err("pathological depth must fail closed");
        assert!(error.contains("depth limit"));
        let _ = fs::remove_dir_all(root);
    }
}
