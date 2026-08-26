// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Disposable source state for the independent fast catalog.
//!
//! The watch index is intentionally separate from cached rows. An unchanged
//! refresh reads and validates only the manifest and watch indexes; large row
//! snapshots are opened only after a source change is proven.

use crate::fast_five_catalog::{FastFiveGameVariant, FastFiveSnapshot, FastFiveSystem};
use crate::system_shard::SystemGame;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const REFRESH_SCHEMA: u32 = 1;
const ENVELOPE_VERSION: u32 = 1;
const ENVELOPE_BYTES: usize = 64;
const MANIFEST_MAGIC: &[u8; 8] = b"MGKRFSMF";
const WATCH_MAGIC: &[u8; 8] = b"MGKRFSWI";
const ROWS_MAGIC: &[u8; 8] = b"MGKRFSRW";
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_WATCH_BYTES: usize = 16 * 1024 * 1024;
const MAX_ROWS_BYTES: usize = 128 * 1024 * 1024;
const STATE_DIRECTORY: &str = "fast-refresh-v1";
const MANIFEST_A: &str = "manifest-a.bin";
const MANIFEST_B: &str = "manifest-b.bin";

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
    pub rows_path: String,
    pub rows_sha256: String,
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
    pub rows: FastSystemRowsSnapshot,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct FastRefreshCaptureReport {
    pub elapsed_us: u64,
    pub directories: usize,
    pub containers: usize,
    pub systems: usize,
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
    pub systems: usize,
    pub unchanged: usize,
    pub changed: usize,
    pub rescans: usize,
    pub row_snapshots_opened: usize,
    pub artifact_writes: usize,
    pub checks: Vec<FastSystemSourceCheck>,
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
    pub row_snapshots_opened: usize,
    pub artifact_systems_written: usize,
    pub catalog_generation: u64,
    pub refresh_generation: u64,
    pub games: u64,
    pub system_reports: Vec<FastCatalogSystemRefreshReport>,
    pub plan: FastRefreshPlanReport,
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
            validate_relative_path(&system.rows_path)?;
            validate_sha256(&system.watch_sha256, "watch checksum")?;
            validate_sha256(&system.rows_sha256, "row checksum")?;
            validate_sha256(&system.source_fingerprint, "source fingerprint")?;
            validate_sha256(&system.row_fingerprint, "row fingerprint")?;
        }
        Ok(())
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
        let prefix = format!("{expected_system_id}\u{1f}");
        let mut keys = BTreeSet::new();
        for game in &self.games {
            if !game.stable_key.starts_with(&prefix)
                || game.title.trim().is_empty()
                || game.launch_ref.trim().is_empty()
                || !keys.insert(game.stable_key.as_str())
            {
                return Err(format!("invalid cached row in {expected_system_id}"));
            }
        }
        for variant in &self.variants {
            if !variant.game.stable_key.starts_with(&prefix)
                || !keys.insert(variant.game.stable_key.as_str())
            {
                return Err(format!("invalid cached variant in {expected_system_id}"));
            }
        }
        Ok(())
    }
}

pub fn refresh_state_root(catalog_root: &Path) -> PathBuf {
    catalog_root.join("state").join(STATE_DIRECTORY)
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
    let watch = decode_envelope(&bytes, WATCH_MAGIC, MAX_WATCH_BYTES)?;
    watch.validate(&reference.system_id)?;
    Ok(watch)
}

pub fn read_system_rows(
    catalog_root: &Path,
    reference: &FastRefreshSystemRef,
) -> Result<FastSystemRowsSnapshot, String> {
    let path = refresh_state_root(catalog_root).join(&reference.rows_path);
    let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    verify_file_checksum(&bytes, &reference.rows_sha256, &path)?;
    let rows = decode_envelope(&bytes, ROWS_MAGIC, MAX_ROWS_BYTES)?;
    rows.validate(&reference.system_id)?;
    Ok(rows)
}

pub fn publish_refresh_state(
    catalog_root: &Path,
    generation: u64,
    catalog_generation: u64,
    catalog_fingerprint: String,
    builder_identity: String,
    systems: &[FastRefreshSystemState],
) -> Result<FastRefreshManifest, String> {
    if generation == 0 {
        return Err("fast refresh generation must be non-zero".to_string());
    }
    let root = refresh_state_root(catalog_root);
    fs::create_dir_all(root.join("systems"))
        .map_err(|error| format!("create refresh state root: {error}"))?;
    let mut references = Vec::with_capacity(systems.len());
    for state in systems {
        state.watch.validate(&state.rows.system_id)?;
        state.rows.validate(&state.watch.system_id)?;
        let system_dir = root.join("systems").join(&state.watch.system_id);
        fs::create_dir_all(&system_dir)
            .map_err(|error| format!("create {} refresh state: {error}", state.watch.system_id))?;
        let watch_relative = format!("systems/{}/{generation}.watch", state.watch.system_id);
        let rows_relative = format!("systems/{}/{generation}.rows", state.watch.system_id);
        let watch_bytes = encode_envelope(&state.watch, WATCH_MAGIC)?;
        let rows_bytes = encode_envelope(&state.rows, ROWS_MAGIC)?;
        write_new_file(&root.join(&watch_relative), &watch_bytes)?;
        write_new_file(&root.join(&rows_relative), &rows_bytes)?;
        references.push(FastRefreshSystemRef {
            system_id: state.watch.system_id.clone(),
            watch_path: watch_relative,
            watch_sha256: sha256_hex(&watch_bytes),
            rows_path: rows_relative,
            rows_sha256: sha256_hex(&rows_bytes),
            source_fingerprint: source_fingerprint(&state.watch),
            row_fingerprint: row_fingerprint(&state.rows)?,
            games: state.rows.games.len().try_into().unwrap_or(u64::MAX),
            variants: state.rows.variants.len().try_into().unwrap_or(u64::MAX),
        });
    }
    references.sort_by(|left, right| left.system_id.cmp(&right.system_id));
    let manifest = FastRefreshManifest::new(
        generation,
        catalog_generation,
        catalog_fingerprint,
        builder_identity,
        references,
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

pub fn publish_refresh_update(
    catalog_root: &Path,
    previous: &FastRefreshManifest,
    catalog_generation: u64,
    catalog_fingerprint: String,
    updated: &[FastRefreshSystemState],
) -> Result<FastRefreshManifest, String> {
    let generation = previous
        .generation
        .checked_add(1)
        .ok_or_else(|| "fast refresh generation overflow".to_string())?;
    let root = refresh_state_root(catalog_root);
    fs::create_dir_all(root.join("systems"))
        .map_err(|error| format!("create refresh state root: {error}"))?;
    let mut references = previous
        .systems
        .iter()
        .cloned()
        .map(|reference| (reference.system_id.clone(), reference))
        .collect::<BTreeMap<_, _>>();
    for state in updated {
        state.watch.validate(&state.rows.system_id)?;
        state.rows.validate(&state.watch.system_id)?;
        let system_dir = root.join("systems").join(&state.watch.system_id);
        fs::create_dir_all(&system_dir)
            .map_err(|error| format!("create {} refresh state: {error}", state.watch.system_id))?;
        let watch_relative = format!("systems/{}/{generation}.watch", state.watch.system_id);
        let rows_relative = format!("systems/{}/{generation}.rows", state.watch.system_id);
        let watch_bytes = encode_envelope(&state.watch, WATCH_MAGIC)?;
        let rows_bytes = encode_envelope(&state.rows, ROWS_MAGIC)?;
        write_new_file(&root.join(&watch_relative), &watch_bytes)?;
        write_new_file(&root.join(&rows_relative), &rows_bytes)?;
        references.insert(
            state.watch.system_id.clone(),
            FastRefreshSystemRef {
                system_id: state.watch.system_id.clone(),
                watch_path: watch_relative,
                watch_sha256: sha256_hex(&watch_bytes),
                rows_path: rows_relative,
                rows_sha256: sha256_hex(&rows_bytes),
                source_fingerprint: source_fingerprint(&state.watch),
                row_fingerprint: row_fingerprint(&state.rows)?,
                games: state.rows.games.len().try_into().unwrap_or(u64::MAX),
                variants: state.rows.variants.len().try_into().unwrap_or(u64::MAX),
            },
        );
    }
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
    let started = std::time::Instant::now();
    snapshot.validate()?;
    let mut states = Vec::with_capacity(snapshot.systems.len());
    let mut report = FastRefreshCaptureReport::default();
    for system in &snapshot.systems {
        let watch = capture_system_watch(storage_root, &system.system_id)?;
        report.directories = report.directories.saturating_add(watch.directories.len());
        report.containers = report.containers.saturating_add(watch.containers.len());
        states.push(FastRefreshSystemState {
            watch,
            rows: FastSystemRowsSnapshot::new(
                system.system_id.clone(),
                system.games.clone(),
                system.variants.clone(),
            )?,
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
    let specification = watch_specification(storage_root, system_id)?;
    let mut directories = Vec::new();
    let mut containers = Vec::new();
    for anchor in &specification.anchors {
        if anchor.is_dir() {
            directories.push(capture_directory(anchor)?);
        }
    }
    for root in &specification.scan_roots {
        if root.is_dir() {
            capture_tree(root, &mut directories, &mut containers)?;
        }
    }
    directories.sort_by(|left, right| left.path.cmp(&right.path));
    directories.dedup_by(|left, right| left.path == right.path);
    containers.sort_by(|left, right| left.path.cmp(&right.path));
    containers.dedup_by(|left, right| left.path == right.path);
    FastSystemWatchIndex::new(
        system_id.to_string(),
        crate::fast_catalog_sources::FAST_SOURCE_ADAPTER_VERSION,
        core_profile_fingerprint(&specification.anchors)?,
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
    let manifest = read_latest_refresh_manifest(catalog_root)?;
    let active = crate::shard_registry::read_latest_manifest_lazy(
        catalog_root,
        crate::shard_registry::production_registry_limits(),
    )
    .map_err(|error| format!("read active fast catalog: {error}"))?;
    let active_fingerprint = crate::fast_five_catalog::registry_fingerprint(
        catalog_root,
        crate::shard_registry::production_registry_limits(),
    )?;
    let binding_matches = manifest.catalog_generation == active.generation
        && manifest.catalog_fingerprint == active_fingerprint
        && manifest.builder_identity
            == format!(
                "independent-fast-sources-v{}",
                crate::fast_catalog_sources::FAST_SOURCE_ADAPTER_VERSION
            );
    let mut references = manifest
        .systems
        .iter()
        .map(|reference| (reference.system_id.as_str(), reference))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut checks = Vec::with_capacity(crate::fast_five_catalog::EXPANDED_FAST_SYSTEM_IDS.len());
    for system_id in crate::fast_five_catalog::EXPANDED_FAST_SYSTEM_IDS {
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
        } else if let Some(reference) = references.remove(system_id) {
            match read_system_watch(catalog_root, reference) {
                Ok(watch) => check_watch_index(storage_root, &watch, &mut check),
                Err(error) => check.reason = format!("watch index unavailable: {error}"),
            }
        } else {
            check.reason = "system source snapshot is missing".to_string();
        }
        check.elapsed_us = system_started
            .elapsed()
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX);
        checks.push(check);
    }
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
        systems: checks.len(),
        unchanged,
        changed,
        rescans,
        row_snapshots_opened: 0,
        artifact_writes: 0,
        checks,
    })
}

pub fn execute_fast_refresh(
    storage_root: &Path,
    catalog_root: &Path,
    request: FastCatalogRefreshRequest,
) -> Result<FastCatalogRefreshReport, String> {
    let started = std::time::Instant::now();
    let plan = plan_fast_refresh(storage_root, catalog_root, request)?;
    let planning_us = started.elapsed().as_micros().try_into().unwrap_or(u64::MAX);
    let previous = read_latest_refresh_manifest(catalog_root)?;
    let active = crate::shard_registry::read_latest_manifest_lazy(
        catalog_root,
        crate::shard_registry::production_registry_limits(),
    )
    .map_err(|error| format!("read active fast catalog: {error}"))?;
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
        match crate::fast_catalog_sources::rebuild_independent_system(
            storage_root,
            &snapshot,
            &check.system_id,
        ) {
            Ok((system, source_report)) => {
                let watch = capture_system_watch(storage_root, &check.system_id)?;
                let rows = FastSystemRowsSnapshot::new(
                    system.system_id.clone(),
                    system.games.clone(),
                    system.variants.clone(),
                )?;
                let new_row_fingerprint = row_fingerprint(&rows)?;
                let rows_changed = previous_ref
                    .is_none_or(|reference| reference.row_fingerprint != new_row_fingerprint);
                if rows_changed {
                    artifact_changes.insert(check.system_id.clone());
                }
                if let Some(target) = snapshot
                    .systems
                    .iter_mut()
                    .find(|candidate| candidate.system_id == check.system_id)
                {
                    *target = system;
                }
                reports.push(FastCatalogSystemRefreshReport {
                    system_id: check.system_id.clone(),
                    outcome: if rows_changed {
                        FastCatalogSystemOutcome::Updated
                    } else {
                        FastCatalogSystemOutcome::Unchanged
                    },
                    source_status: check.status,
                    games: rows.games.len().try_into().unwrap_or(u64::MAX),
                    variants: rows.variants.len().try_into().unwrap_or(u64::MAX),
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
                updated_states.push(FastRefreshSystemState { watch, rows });
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
            crate::fast_five_catalog::FastFiveArtifactProfile::SearchOnly,
        )?;
    }
    let artifact_publish_us = artifact_started
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX);
    let snapshot_started = std::time::Instant::now();
    let active = crate::shard_registry::read_latest_manifest_lazy(
        catalog_root,
        crate::shard_registry::production_registry_limits(),
    )
    .map_err(|error| format!("read refreshed fast catalog: {error}"))?;
    let refresh_generation = if updated_states.is_empty() {
        previous.generation
    } else {
        publish_refresh_update(
            catalog_root,
            &previous,
            active.generation,
            crate::fast_five_catalog::registry_fingerprint(
                catalog_root,
                crate::shard_registry::production_registry_limits(),
            )?,
            &updated_states,
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
        elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        planning_us,
        source_rebuild_us,
        artifact_publish_us,
        snapshot_publish_us,
        systems: reports.len(),
        unchanged,
        updated,
        removed,
        failed_retained,
        row_snapshots_opened: 0,
        artifact_systems_written: artifact_changes.len(),
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
    _storage_root: &Path,
    watch: &FastSystemWatchIndex,
    check: &mut FastSystemSourceCheck,
) {
    let known_directories = watch
        .directories
        .iter()
        .map(|directory| directory.path.as_str())
        .collect::<BTreeSet<_>>();
    for root in &watch.roots {
        if Path::new(root).is_dir() != known_directories.contains(root.as_str()) {
            check.status = FastSourceCheckStatus::Changed;
            check.reason = format!("root availability changed: {root}");
            return;
        }
    }
    for directory in &watch.directories {
        check.directories_checked = check.directories_checked.saturating_add(1);
        let metadata = match fs::metadata(&directory.path) {
            Ok(metadata) if metadata.is_dir() => metadata,
            _ => {
                check.status = FastSourceCheckStatus::Changed;
                check.reason = format!("directory removed: {}", directory.path);
                return;
            }
        };
        if modified_ns(&metadata) != directory.modified_ns {
            check.status = FastSourceCheckStatus::Changed;
            check.reason = format!("directory entries changed: {}", directory.path);
            return;
        }
    }
    for container in &watch.containers {
        check.containers_checked = check.containers_checked.saturating_add(1);
        let metadata = match fs::metadata(&container.path) {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => {
                check.status = FastSourceCheckStatus::Changed;
                check.reason = format!("container removed: {}", container.path);
                return;
            }
        };
        if metadata.len() != container.size
            || modified_ns(&metadata) != container.modified_ns
            || changed_ns(&metadata) != container.changed_ns
            || inode(&metadata) != container.inode
        {
            check.status = FastSourceCheckStatus::Changed;
            check.reason = format!("container changed: {}", container.path);
            return;
        }
    }
    check.status = FastSourceCheckStatus::Unchanged;
    check.reason = "source identities match".to_string();
}

#[derive(Debug)]
struct WatchSpecification {
    scan_roots: Vec<PathBuf>,
    anchors: Vec<PathBuf>,
}

fn watch_specification(storage_root: &Path, system_id: &str) -> Result<WatchSpecification, String> {
    let games = storage_root.join("games");
    let (scan_roots, core_parent) = match system_id {
        "amiga" => (vec![games.join("Amiga")], storage_root.join("_Computer")),
        "arcade" => (
            vec![
                storage_root.join("_Arcade"),
                games.join("mame"),
                games.join("hbmame"),
            ],
            storage_root.join("_Arcade/cores"),
        ),
        "c64" => (vec![games.join("C64")], storage_root.join("_Computer")),
        "dos" => (
            vec![storage_root.join("_DOS Games"), games.join("AO486")],
            storage_root.join("_Computer"),
        ),
        "neogeo" => (vec![games.join("NEOGEO")], storage_root.join("_Console")),
        "saturn" => (vec![games.join("Saturn")], storage_root.join("_Console")),
        "snes" => (
            ["SNES", "Satellaview", "SGB2", "SNES-Sinden"]
                .into_iter()
                .map(|name| games.join(name))
                .collect(),
            storage_root.join("_Console"),
        ),
        "x68000" => (
            vec![
                storage_root.join("_Computer/_X68000 Games"),
                storage_root.join("_Computer/X68000 Games"),
                games.join("X68000"),
            ],
            storage_root.join("_Computer"),
        ),
        "zx-spectrum" => (vec![games.join("Spectrum")], storage_root.join("_Computer")),
        _ => return Err(format!("unsupported fast refresh system {system_id}")),
    };
    let mut anchors = vec![games, core_parent];
    for root in &scan_roots {
        if let Some(parent) = root.parent() {
            anchors.push(parent.to_path_buf());
        }
    }
    anchors.sort();
    anchors.dedup();
    Ok(WatchSpecification {
        scan_roots,
        anchors,
    })
}

fn capture_tree(
    root: &Path,
    directories: &mut Vec<FastWatchedDirectory>,
    containers: &mut Vec<FastWatchedContainer>,
) -> Result<(), String> {
    directories.push(capture_directory(root)?);
    let mut entries = fs::read_dir(root)
        .map_err(|error| format!("read watch directory {}: {error}", root.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("enumerate watch directory {}: {error}", root.display()))?;
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|error| format!("inspect {}: {error}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            capture_tree(&path, directories, containers)?;
        } else if file_type.is_file() && is_watched_container(&path) {
            containers.push(capture_container(&path)?);
        }
    }
    Ok(())
}

fn capture_directory(path: &Path) -> Result<FastWatchedDirectory, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("stat watch directory {}: {error}", path.display()))?;
    let mut entries = fs::read_dir(path)
        .map_err(|error| format!("read watch directory {}: {error}", path.display()))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let kind = entry.file_type().ok()?;
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
    entries.sort_by(|left, right| {
        left.0
            .to_ascii_lowercase()
            .cmp(&right.0.to_ascii_lowercase())
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

fn core_profile_fingerprint(anchors: &[PathBuf]) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest.update(b"mister-magik-fast-source-adapter-v1\0");
    for anchor in anchors {
        digest.update(anchor.to_string_lossy().as_bytes());
        digest.update([u8::from(anchor.is_dir())]);
        if anchor.is_dir() {
            let directory = capture_directory(anchor)?;
            digest.update(directory.entry_fingerprint.as_bytes());
        }
    }
    Ok(sha256_digest_hex(digest.finalize()))
}

fn is_watched_container(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "zip" | "7z" | "mgl" | "mra" | "txt"
            )
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
    rows.validate(&rows.system_id)?;
    postcard::to_allocvec(rows)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| format!("encode row fingerprint: {error}"))
}

fn encode_envelope<T: Serialize>(value: &T, magic: &[u8; 8]) -> Result<Vec<u8>, String> {
    let payload = postcard::to_allocvec(value).map_err(|error| format!("encode state: {error}"))?;
    let payload_len = u64::try_from(payload.len()).map_err(|_| "state is too large")?;
    let mut output = Vec::with_capacity(ENVELOPE_BYTES + payload.len());
    output.extend_from_slice(magic);
    output.extend_from_slice(&ENVELOPE_VERSION.to_le_bytes());
    output.extend_from_slice(&REFRESH_SCHEMA.to_le_bytes());
    output.extend_from_slice(&payload_len.to_le_bytes());
    output.extend_from_slice(&Sha256::digest(&payload));
    output.extend_from_slice(&[0; 8]);
    debug_assert_eq!(output.len(), ENVELOPE_BYTES);
    output.extend_from_slice(&payload);
    Ok(output)
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
    write_synced(path, bytes, false)
}

fn write_replace_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    write_synced(&temporary, bytes, true)?;
    fs::rename(&temporary, path).map_err(|error| format!("publish {}: {error}", path.display()))
}

fn write_synced(path: &Path, bytes: &[u8], replace: bool) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create(true);
    if replace {
        options.truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write {}: {error}", path.display()))
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
            rows: FastSystemRowsSnapshot {
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
            },
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
        assert_eq!(read_system_rows(&root, reference).unwrap().games.len(), 1);

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
                rows_path: "rows".to_string(),
                rows_sha256: "c".repeat(64),
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
        fs::create_dir_all(&games).unwrap();
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
        check_watch_index(&root, &watch, &mut check);
        assert_eq!(check.status, FastSourceCheckStatus::Unchanged);
        assert_eq!(check.directories_checked, watch.directories.len());
        assert_eq!(check.containers_checked, 0);
    }

    #[test]
    fn watch_check_detects_container_replacement_without_row_reads() {
        let root = crate::test_support::unique_temp_dir("fast-refresh-container-check");
        let games = root.join("games/SNES");
        fs::create_dir_all(&games).unwrap();
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
        check_watch_index(&root, &watch, &mut check);
        assert_eq!(check.status, FastSourceCheckStatus::Changed);
    }
}
