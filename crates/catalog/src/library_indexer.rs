// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library filesystem indexing and classification.
//!
//! This module owns the full-scan product contract: walk configured roots,
//! classify launcher/payload/archive/listing candidates, emit progress/events,
//! and return a complete `LibraryScan`.

use crate::catalog_config::SCHEMA_VERSION;
use crate::catalog_progress::{CatalogProgress, report_catalog_progress};
use crate::catalog_scan::{self, DiscoveryEvent, TargetFingerprint as Fingerprint};
use crate::core_audit;
use crate::game_discovery::{
    GameDiscovery, catalog_system_id_for_discovery, discovery_from_profile_archive_entry,
    discovery_from_profile_file_with_prepared_index_and_mra_metadata,
};
use crate::launch_profiles::{self, PayloadDisposition, PayloadRule, ProfilePathClass};
use crate::library_db::{
    self, ArchiveFormat, BenchConfig, LibraryBootstrapSummary, LibraryPayloadFile, LibraryScan,
    LibraryScanEvent, ProgressCallback, ScanEventCallback,
};
use crate::media_metadata;
use crate::prepared_bundle_helper::PreparedTargetCatalogHelper;
use crate::prepared_collections::PreparedPayloadIndex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

const SCAN_PROGRESS_CANDIDATE_BATCH: usize = 50;
const BOOTSTRAP_PROGRESS_BATCH: usize = 50;
const ARCADE_MRA_READ_WORKERS: usize = 4;

struct ContributorClosure {
    expected: BTreeMap<usize, Option<String>>,
    remaining_unknown: usize,
    remaining_by_system: BTreeMap<String, usize>,
    discovered: BTreeSet<String>,
    closed: BTreeSet<String>,
    sound: bool,
}

impl ContributorClosure {
    fn new(expected: impl IntoIterator<Item = (usize, Option<String>)>) -> Self {
        let expected = expected.into_iter().collect::<BTreeMap<_, _>>();
        let remaining_unknown = expected.values().filter(|system| system.is_none()).count();
        let mut remaining_by_system = BTreeMap::<String, usize>::new();
        for system in expected.values().flatten() {
            *remaining_by_system.entry(system.clone()).or_default() += 1;
        }
        Self {
            expected,
            remaining_unknown,
            remaining_by_system,
            discovered: BTreeSet::new(),
            closed: BTreeSet::new(),
            sound: true,
        }
    }

    fn complete(&mut self, ordinal: usize, observed: &BTreeSet<String>) -> Vec<String> {
        self.discovered.extend(observed.iter().cloned());
        match self.expected.get(&ordinal).cloned().flatten() {
            Some(expected) => {
                if observed.iter().any(|system| system != &expected) {
                    self.sound = false;
                }
                if let Some(remaining) = self.remaining_by_system.get_mut(&expected) {
                    *remaining = remaining.saturating_sub(1);
                }
            }
            None => self.remaining_unknown = self.remaining_unknown.saturating_sub(1),
        }
        if !self.sound || self.remaining_unknown != 0 {
            return Vec::new();
        }
        let ready = self
            .discovered
            .iter()
            .filter(|system| {
                !self.closed.contains(*system)
                    && self.remaining_by_system.get(*system).copied().unwrap_or(0) == 0
            })
            .cloned()
            .collect::<Vec<_>>();
        self.closed.extend(ready.iter().cloned());
        ready
    }

    fn compact_detail(&self) -> String {
        format!(
            "sound={} remaining_unknown={} closed={} discovered={}",
            u8::from(self.sound),
            self.remaining_unknown,
            self.closed.len(),
            self.discovered.len(),
        )
    }
}

pub(crate) struct LibraryIndexer<'a> {
    cfg: &'a BenchConfig,
    archive_reader: crate::catalog_config::ArchiveReaderConfig,
    priority: LibraryScanPriority,
    durable_resume: bool,
    arcade_updater_index: Option<PathBuf>,
    enforce_arcade_rom_presence: bool,
}

impl<'a> LibraryIndexer<'a> {
    pub(crate) fn new(cfg: &'a BenchConfig) -> Self {
        Self::with_archive_reader(cfg, crate::catalog_config::ArchiveReaderConfig::default())
    }

    pub(crate) fn with_archive_reader(
        cfg: &'a BenchConfig,
        archive_reader: crate::catalog_config::ArchiveReaderConfig,
    ) -> Self {
        Self {
            cfg,
            archive_reader,
            priority: LibraryScanPriority::Background,
            durable_resume: library_db::env_bool("MISTER_CATALOG_DURABLE_RESUME"),
            arcade_updater_index: None,
            enforce_arcade_rom_presence: true,
        }
    }

    pub(crate) fn foreground_with_archive_reader(
        cfg: &'a BenchConfig,
        archive_reader: crate::catalog_config::ArchiveReaderConfig,
    ) -> Self {
        Self {
            cfg,
            archive_reader,
            priority: LibraryScanPriority::Foreground,
            durable_resume: library_db::env_bool("MISTER_CATALOG_DURABLE_RESUME"),
            arcade_updater_index: None,
            enforce_arcade_rom_presence: true,
        }
    }

    pub(crate) fn with_durable_resume(mut self, durable_resume: bool) -> Self {
        self.durable_resume = durable_resume;
        self
    }

    pub(crate) fn with_arcade_updater_index(mut self, path: &Path) -> Self {
        self.arcade_updater_index = Some(path.to_path_buf());
        self
    }

    pub(crate) fn with_arcade_rom_presence(mut self, enforce: bool) -> Self {
        self.enforce_arcade_rom_presence = enforce;
        self
    }

    #[cfg(test)]
    pub(crate) fn scan(&self) -> LibraryScan {
        self.scan_with_progress_and_events(None, None)
    }

    pub(crate) fn scan_with_progress_and_events(
        &self,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
    ) -> LibraryScan {
        scan_library_with_progress_and_events(
            LibraryScanExecution {
                cfg: self.cfg,
                archive_reader: &self.archive_reader,
                priority: self.priority,
                audit_mode: CoverageAuditMode::Inline,
                durable_resume: self.durable_resume,
                arcade_updater_index: self.arcade_updater_index.as_deref(),
                enforce_arcade_rom_presence: self.enforce_arcade_rom_presence,
            },
            progress,
            scan_events,
            Vec::new(),
        )
    }

    pub(crate) fn scan_without_coverage_audit_with_progress_and_events(
        &self,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
    ) -> LibraryScan {
        self.scan_without_coverage_audit_with_progress_events_and_exclusions(
            progress,
            scan_events,
            Vec::new(),
        )
    }

    pub(crate) fn scan_without_coverage_audit_with_progress_events_and_exclusions(
        &self,
        progress: ProgressCallback<'_>,
        scan_events: ScanEventCallback<'_>,
        excluded_targets: Vec<PathBuf>,
    ) -> LibraryScan {
        scan_library_with_progress_and_events(
            LibraryScanExecution {
                cfg: self.cfg,
                archive_reader: &self.archive_reader,
                priority: self.priority,
                audit_mode: CoverageAuditMode::Deferred,
                durable_resume: self.durable_resume,
                arcade_updater_index: self.arcade_updater_index.as_deref(),
                enforce_arcade_rom_presence: self.enforce_arcade_rom_presence,
            },
            progress,
            scan_events,
            excluded_targets,
        )
    }

    pub(crate) fn bootstrap_progress(
        &self,
        progress: ProgressCallback<'_>,
    ) -> LibraryBootstrapSummary {
        bootstrap_library_progress(self.cfg, progress)
    }
}

#[derive(Clone, Copy)]
enum LibraryScanPriority {
    Background,
    Foreground,
}

#[derive(Clone, Copy)]
enum CoverageAuditMode {
    Inline,
    Deferred,
}

#[derive(Default)]
struct ScanTimingStats {
    profile_match_us: u64,
    profile_match_count: usize,
    file_discovery_us: u64,
    file_discovery_count: usize,
    archive_toc_us: u64,
    archive_toc_count: usize,
    installed_collection_us: u64,
    installed_collection_count: usize,
    collection_listing_us: u64,
    collection_listing_count: usize,
    file_discovery_breakdown: HashMap<String, HashMap<String, FileDiscoveryTimingBucket>>,
}

#[derive(Default)]
struct ScanHandoffAttribution {
    receive_wait_us: u64,
    consumer_active_us: u64,
    events: usize,
    file_events: usize,
    facts_events: usize,
    runtime_events: usize,
    target_events: usize,
    files: usize,
    max_batch: usize,
}

impl ScanHandoffAttribution {
    fn compact_detail(&self, loop_us: u64) -> String {
        let accounted_us = self
            .receive_wait_us
            .saturating_add(self.consumer_active_us)
            .min(loop_us);
        format!(
            "loop_us={loop_us} receive_wait_us={} consumer_active_us={} unattributed_us={} events={} file_events={} facts_events={} runtime_events={} target_events={} files={} max_batch={} channel_capacity={}",
            self.receive_wait_us,
            self.consumer_active_us,
            loop_us.saturating_sub(accounted_us),
            self.events,
            self.file_events,
            self.facts_events,
            self.runtime_events,
            self.target_events,
            self.files,
            self.max_batch,
            catalog_scan::DISCOVERY_EVENT_BUFFER,
        )
    }
}

#[derive(Serialize, Deserialize)]
struct TargetOutput {
    game_dir_facts: Vec<crate::catalog_discovery::GameDirFact>,
    normal_files: Vec<LibraryPayloadFile>,
    containers: Vec<crate::library_db::LibraryContainer>,
    entries: Vec<crate::library_db::LibraryContainerEntry>,
    ignored_files: usize,
    discoveries: Vec<GameDiscovery>,
}

const PREPARED_HELPER_DIR_ENV: &str = "MISTER_PREPARED_BUNDLE_HELPER_DIR";
const PREPARED_HELPER_CAPTURE_DIR_ENV: &str = "MISTER_PREPARED_BUNDLE_CAPTURE_DIR";

#[derive(Default)]
struct PreparedTargetHelpers {
    reusable: HashMap<String, PreparedTargetCatalogHelper>,
    checked: usize,
    matched: usize,
    rejected: usize,
}

fn normalized_target_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn load_prepared_target_helpers(
    descriptors: &[catalog_scan::ScanTargetDescriptor],
) -> PreparedTargetHelpers {
    let Some(directory) = std::env::var_os(PREPARED_HELPER_DIR_ENV).map(PathBuf::from) else {
        return PreparedTargetHelpers::default();
    };
    let expected = descriptors
        .iter()
        .map(|descriptor| normalized_target_key(&descriptor.path))
        .collect::<BTreeSet<_>>();
    let mut result = PreparedTargetHelpers::default();
    let Ok(entries) = std::fs::read_dir(&directory) else {
        crate::catalog_logln!(
            "prepared_bundle_helper_tsv\tstate=unavailable\tdirectory={}",
            directory.display()
        );
        return result;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        result.checked = result.checked.saturating_add(1);
        let helper = std::fs::read(&path)
            .map_err(|error| format!("read-error:{error}"))
            .and_then(|bytes| PreparedTargetCatalogHelper::from_json(&bytes));
        let helper = match helper {
            Ok(helper) => helper,
            Err(reason) => {
                result.rejected = result.rejected.saturating_add(1);
                crate::catalog_logln!(
                    "prepared_bundle_helper_tsv\tstate=rejected\treason={}\tpath={}",
                    reason.replace(['\t', '\n', '\r', ' '], "_"),
                    path.display()
                );
                continue;
            }
        };
        let key = normalized_target_key(Path::new(&helper.target_path));
        if !expected.contains(&key) {
            continue;
        }
        match helper.activate() {
            Ok(()) => {
                result.matched = result.matched.saturating_add(1);
                result.reusable.insert(key, helper);
            }
            Err(reason) => {
                result.rejected = result.rejected.saturating_add(1);
                crate::catalog_logln!(
                    "prepared_bundle_helper_tsv\tstate=fallback\treason={}\tpath={}",
                    reason.replace(['\t', '\n', '\r', ' '], "_"),
                    path.display()
                );
            }
        }
    }
    result
}

fn path_relative_to_storage_root(path: &Path, storage_root: &Path) -> Option<String> {
    let relative = path.strip_prefix(storage_root).ok()?;
    let value = relative.to_str()?.replace('\\', "/");
    (!value.is_empty()).then_some(value)
}

fn prepared_target_collection_ids(discoveries: &[GameDiscovery]) -> BTreeSet<String> {
    discoveries
        .iter()
        .filter_map(|discovery| discovery.prepared)
        .map(|prepared| prepared.collection_id.as_str().to_string())
        .collect()
}

fn path_string_is_within(path: &str, root: &Path) -> bool {
    Path::new(path).starts_with(root)
}

fn oneload64_target_output(
    output: &BorrowedTargetOutput<'_>,
) -> Result<Option<(PathBuf, TargetOutput)>, String> {
    let roots = output
        .discoveries
        .iter()
        .filter(|discovery| {
            discovery.prepared.is_some_and(|prepared| {
                prepared.collection_id
                    == crate::prepared_collections::PreparedCollectionId::OneLoad64
            })
        })
        .filter_map(|discovery| {
            crate::prepared_collections::oneload64_install_root(Path::new(&discovery.source_path))
                .map(Path::to_path_buf)
        })
        .collect::<BTreeSet<_>>();
    if roots.is_empty() {
        return Ok(None);
    }
    if roots.len() != 1 {
        return Err("multiple OneLoad64 installation roots share one C64 target".to_string());
    }
    let root = roots.into_iter().next().unwrap();
    let discoveries = output
        .discoveries
        .iter()
        .filter(|discovery| path_string_is_within(&discovery.source_path, &root))
        .cloned()
        .collect::<Vec<_>>();
    if discoveries.is_empty() {
        return Ok(None);
    }
    Ok(Some((
        root.clone(),
        TargetOutput {
            game_dir_facts: Vec::new(),
            normal_files: output
                .normal_files
                .iter()
                .filter(|file| path_string_is_within(&file.path, &root))
                .cloned()
                .collect(),
            containers: output
                .containers
                .iter()
                .filter(|container| path_string_is_within(&container.file_path, &root))
                .cloned()
                .collect(),
            entries: output
                .entries
                .iter()
                .filter(|entry| path_string_is_within(&entry.file_path, &root))
                .cloned()
                .collect(),
            ignored_files: 0,
            discoveries,
        },
    )))
}

fn capture_prepared_target_helper(
    cfg: &BenchConfig,
    descriptor: &catalog_scan::ScanTargetDescriptor,
    output: &BorrowedTargetOutput<'_>,
    mut output_json: String,
) -> Result<Option<PathBuf>, String> {
    let Some(directory) = std::env::var_os(PREPARED_HELPER_CAPTURE_DIR_ENV).map(PathBuf::from)
    else {
        return Ok(None);
    };
    let collection_ids = prepared_target_collection_ids(output.discoveries);
    if collection_ids.is_empty() {
        return Ok(None);
    }
    // 0MHz is now accelerated per game from the checked-in release manifest.
    // A whole-target DOS snapshot would hide newly added/custom launchers.
    if collection_ids.contains("0mhz") {
        return Ok(None);
    }
    let partial_oneload = if collection_ids.contains("oneload64") {
        oneload64_target_output(output)?
    } else {
        None
    };
    let scan_exclusion_path = partial_oneload.as_ref().map(|(root, _)| root.as_path());
    if let Some((_, filtered)) = partial_oneload.as_ref() {
        output_json = serde_json::to_string(filtered)
            .map_err(|error| format!("encode OneLoad64 target output: {error}"))?;
    }
    let discoveries = partial_oneload
        .as_ref()
        .map_or(output.discoveries, |(_, filtered)| {
            filtered.discoveries.as_slice()
        });
    let normal_files = partial_oneload
        .as_ref()
        .map_or(output.normal_files, |(_, filtered)| {
            filtered.normal_files.as_slice()
        });
    let containers = partial_oneload
        .as_ref()
        .map_or(output.containers, |(_, filtered)| {
            filtered.containers.as_slice()
        });
    let entries = partial_oneload
        .as_ref()
        .map_or(output.entries, |(_, filtered)| filtered.entries.as_slice());
    let storage_root = crate::prepared_collections::storage_roots_for_library_roots(&cfg.roots)
        .into_iter()
        .filter(|root| descriptor.path.starts_with(root))
        .max_by_key(|root| root.components().count())
        .ok_or_else(|| {
            format!(
                "no storage root contains prepared target {}",
                descriptor.path.display()
            )
        })?;
    let mut paths = BTreeSet::<String>::new();
    for discovery in discoveries {
        for value in [
            Some(discovery.source_path.as_str()),
            discovery.covered_payload_path.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(relative) = path_relative_to_storage_root(Path::new(value), &storage_root) {
                paths.insert(relative);
            }
        }
    }
    paths.extend(
        normal_files
            .iter()
            .filter_map(|file| path_relative_to_storage_root(Path::new(&file.path), &storage_root)),
    );
    paths.extend(containers.iter().filter_map(|container| {
        path_relative_to_storage_root(Path::new(&container.file_path), &storage_root)
    }));
    paths.extend(entries.iter().filter_map(|entry| {
        path_relative_to_storage_root(Path::new(&entry.file_path), &storage_root)
    }));

    let mut exact_paths = BTreeSet::new();
    let mut payload_paths = BTreeSet::new();
    let mut inventory_extensions = BTreeSet::new();
    for relative in paths {
        let path = storage_root.join(&relative);
        if !path.is_file() {
            continue;
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !extension.is_empty() && path.starts_with(&descriptor.path) {
            inventory_extensions.insert(extension.clone());
        }
        if matches!(extension.as_str(), "mgl" | "mra" | "txt" | "json") {
            exact_paths.insert(relative);
        } else if !path.starts_with(&descriptor.path)
            || matches!(extension.as_str(), "zip" | "7z" | "lha" | "lzh" | "rar")
        {
            payload_paths.insert(relative);
        }
    }
    for collection_id in &collection_ids {
        match collection_id.as_str() {
            "amigavision" => inventory_extensions.extend(["mgl".to_string(), "txt".to_string()]),
            "0mhz" | "neon68k" => {
                inventory_extensions.insert("mgl".to_string());
            }
            "oneload64" => {
                inventory_extensions.insert("crt".to_string());
            }
            _ => {}
        }
    }
    if inventory_extensions.is_empty() {
        return Err(format!(
            "prepared target {} has no inventory extensions",
            descriptor.path.display()
        ));
    }
    let helper = PreparedTargetCatalogHelper::capture(
        &storage_root,
        &descriptor.path,
        scan_exclusion_path,
        collection_ids.into_iter().collect::<Vec<_>>().join("+"),
        output_json,
        &exact_paths.into_iter().collect::<Vec<_>>(),
        &payload_paths.into_iter().collect::<Vec<_>>(),
        &inventory_extensions.into_iter().collect::<Vec<_>>(),
    )?;
    std::fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "create prepared target helper directory {}: {error}",
            directory.display()
        )
    })?;
    let filename = format!("target-{:03}.json", descriptor.ordinal);
    let path = directory.join(filename);
    std::fs::write(&path, helper.to_json()?)
        .map_err(|error| format!("write prepared target helper {}: {error}", path.display()))?;
    Ok(Some(path))
}

#[derive(Serialize)]
struct BorrowedTargetOutput<'a> {
    game_dir_facts: &'a [crate::catalog_discovery::GameDirFact],
    normal_files: &'a [LibraryPayloadFile],
    containers: &'a [crate::library_db::LibraryContainer],
    entries: &'a [crate::library_db::LibraryContainerEntry],
    ignored_files: usize,
    discoveries: &'a [GameDiscovery],
}

#[derive(Clone, Copy)]
struct TargetOffsets {
    facts: usize,
    files: usize,
    containers: usize,
    entries: usize,
    ignored: usize,
    discoveries: usize,
    candidates: usize,
    arcade_mra_eligible: usize,
    arcade_mra_missing_rom: usize,
    arcade_mra_ambiguous: usize,
    arcade_mra_malformed: usize,
    first_discovery_reported: bool,
}

impl TargetOffsets {
    fn capture(
        facts: &[crate::catalog_discovery::GameDirFact],
        files: &[LibraryPayloadFile],
        containers: &[crate::library_db::LibraryContainer],
        entries: &[crate::library_db::LibraryContainerEntry],
        ignored: usize,
        discoveries: &[GameDiscovery],
    ) -> Self {
        Self {
            facts: facts.len(),
            files: files.len(),
            containers: containers.len(),
            entries: entries.len(),
            ignored,
            discoveries: discoveries.len(),
            candidates: 0,
            arcade_mra_eligible: 0,
            arcade_mra_missing_rom: 0,
            arcade_mra_ambiguous: 0,
            arcade_mra_malformed: 0,
            first_discovery_reported: false,
        }
    }

    fn with_counters(
        mut self,
        candidates: usize,
        arcade_mra_eligible: usize,
        arcade_mra_missing_rom: usize,
        arcade_mra_ambiguous: usize,
        arcade_mra_malformed: usize,
        first_discovery_reported: bool,
    ) -> Self {
        self.candidates = candidates;
        self.arcade_mra_eligible = arcade_mra_eligible;
        self.arcade_mra_missing_rom = arcade_mra_missing_rom;
        self.arcade_mra_ambiguous = arcade_mra_ambiguous;
        self.arcade_mra_malformed = arcade_mra_malformed;
        self.first_discovery_reported = first_discovery_reported;
        self
    }

    #[allow(clippy::too_many_arguments)]
    fn rollback<F, N, C, E, D>(
        self,
        facts: &mut Vec<F>,
        files: &mut Vec<N>,
        containers: &mut Vec<C>,
        entries: &mut Vec<E>,
        ignored: &mut usize,
        discoveries: &mut Vec<D>,
        candidates: &mut usize,
        arcade_mra_eligible: &mut usize,
        arcade_mra_missing_rom: &mut usize,
        arcade_mra_ambiguous: &mut usize,
        arcade_mra_malformed: &mut usize,
        first_discovery_reported: &mut bool,
    ) {
        facts.truncate(self.facts);
        files.truncate(self.files);
        containers.truncate(self.containers);
        entries.truncate(self.entries);
        *ignored = self.ignored;
        discoveries.truncate(self.discoveries);
        *candidates = self.candidates;
        *arcade_mra_eligible = self.arcade_mra_eligible;
        *arcade_mra_missing_rom = self.arcade_mra_missing_rom;
        *arcade_mra_ambiguous = self.arcade_mra_ambiguous;
        *arcade_mra_malformed = self.arcade_mra_malformed;
        *first_discovery_reported = self.first_discovery_reported;
    }
}

struct ResumeScan {
    journal: crate::build_progress::BuildProgressJournal,
    reusable: HashMap<u32, crate::build_progress::CompletedTarget>,
    invalidated_targets: BTreeSet<u32>,
    affected_systems: Vec<String>,
    all_published_systems: bool,
    target_count: usize,
    reused: usize,
    invalidated: usize,
    committed: usize,
    pending: Vec<crate::build_progress::CompletedTarget>,
    pending_bytes: usize,
    checkpoint: CheckpointAttribution,
}

#[derive(Clone, Debug, Default)]
struct CheckpointAttribution {
    enabled: bool,
    snapshot_us: u64,
    encode_us: u64,
    decode_us: u64,
    queued_targets: usize,
    queued_bytes: usize,
    frame_bytes: usize,
    batches: usize,
    begin_us: u64,
    compress_us: u64,
    append_us: u64,
    sync_us: u64,
    rows_us: u64,
    commit_us: u64,
    write_us: u64,
    errors: usize,
}

impl CheckpointAttribution {
    fn compact_detail(&self) -> String {
        format!(
            "enabled={} snapshot_us={} encode_us={} decode_us={} queued_targets={} queued_bytes={} frame_bytes={} batches={} begin_us={} compress_us={} append_us={} sync_us={} rows_us={} commit_us={} write_us={} errors={}",
            u8::from(self.enabled),
            self.snapshot_us,
            self.encode_us,
            self.decode_us,
            self.queued_targets,
            self.queued_bytes,
            self.frame_bytes,
            self.batches,
            self.begin_us,
            self.compress_us,
            self.append_us,
            self.sync_us,
            self.rows_us,
            self.commit_us,
            self.write_us,
            self.errors,
        )
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ResumeValidationAttribution {
    pub(crate) enabled: bool,
    pub(crate) committed_state_present: bool,
    pub(crate) committed_state_seeded: bool,
    pub(crate) open_us: u64,
    pub(crate) frame_decode_us: u64,
    pub(crate) validation_us: u64,
    pub(crate) validation_backend: &'static str,
    pub(crate) validation_join_us: u64,
    pub(crate) validation_receive_wait_us: u64,
    pub(crate) validation_consumer_us: u64,
    pub(crate) validation_events: usize,
    pub(crate) validation_file_events: usize,
    pub(crate) validation_facts_events: usize,
    pub(crate) validation_runtime_events: usize,
    pub(crate) output_decode_us: u64,
    pub(crate) output_decode_bytes: usize,
    pub(crate) output_decode_targets: usize,
    pub(crate) committed_targets: usize,
    pub(crate) validated_targets: usize,
    pub(crate) reused_targets: usize,
    pub(crate) invalidated_targets: usize,
    pub(crate) unavailable_targets: usize,
    pub(crate) error_targets: usize,
    pub(crate) setup_errors: usize,
    pub(crate) namespace: catalog_scan::NamespaceRouteAttribution,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CatalogScanAttribution {
    pub(crate) plan_us: u64,
    pub(crate) resume_us: u64,
    pub(crate) prepared_payload_us: u64,
    pub(crate) execution_pipeline_us: u64,
    pub(crate) post_pipeline_us: u64,
    pub(crate) accounted_us: u64,
    pub(crate) unattributed_us: u64,
    pub(crate) total_us: u64,
    pub(crate) resume: ResumeValidationAttribution,
    pub(crate) execution: catalog_scan::NamespaceRouteAttribution,
}

impl CatalogScanAttribution {
    pub(crate) fn compact_detail(&self) -> String {
        format!(
            "scan_total_us={} scan_accounted_us={} scan_unattributed_us={} scan_plan_us={} scan_resume_us={} scan_prepared_payload_us={} scan_execution_pipeline_us={} scan_post_pipeline_us={} resume_enabled={} resume_state_present={} resume_state_seeded={} resume_open_us={} resume_frame_decode_us={} resume_validation_us={} resume_validation_backend={} resume_validation_join_us={} resume_validation_receive_wait_us={} resume_validation_consumer_us={} resume_validation_events={} resume_validation_file_events={} resume_validation_facts_events={} resume_validation_runtime_events={} resume_output_decode_us={} resume_output_decode_bytes={} resume_output_decode_targets={} resume_committed={} resume_validated={} resume_reused={} resume_invalidated={} resume_unavailable={} resume_errors={} resume_setup_errors={} {} {}",
            self.total_us,
            self.accounted_us,
            self.unattributed_us,
            self.plan_us,
            self.resume_us,
            self.prepared_payload_us,
            self.execution_pipeline_us,
            self.post_pipeline_us,
            u8::from(self.resume.enabled),
            u8::from(self.resume.committed_state_present),
            u8::from(self.resume.committed_state_seeded),
            self.resume.open_us,
            self.resume.frame_decode_us,
            self.resume.validation_us,
            self.resume.validation_backend,
            self.resume.validation_join_us,
            self.resume.validation_receive_wait_us,
            self.resume.validation_consumer_us,
            self.resume.validation_events,
            self.resume.validation_file_events,
            self.resume.validation_facts_events,
            self.resume.validation_runtime_events,
            self.resume.output_decode_us,
            self.resume.output_decode_bytes,
            self.resume.output_decode_targets,
            self.resume.committed_targets,
            self.resume.validated_targets,
            self.resume.reused_targets,
            self.resume.invalidated_targets,
            self.resume.unavailable_targets,
            self.resume.error_targets,
            self.resume.setup_errors,
            self.resume.namespace.compact_detail("validation"),
            self.execution.compact_detail("execution"),
        )
    }
}

const RESUME_CHECKPOINT_TARGET_BATCH: usize = 16;
const RESUME_CHECKPOINT_MAX_BYTES: usize = 2 * 1024 * 1024;

fn progress_target(
    descriptor: &catalog_scan::ScanTargetDescriptor,
) -> crate::build_progress::ScanTarget {
    crate::build_progress::ScanTarget {
        ordinal: descriptor.ordinal as u32,
        key: format!("{:?}:{}", descriptor.kind, descriptor.path.display()),
        path: descriptor.path.display().to_string(),
    }
}

fn report_resume(state: &ResumeScan, phase: &str, ordinal: usize, reason: &str) {
    crate::catalog_logln!(
        "catalog_resume_tsv\tbuild_id={}\tphase={}\ttarget_ordinal={}\ttarget_count={}\tcommitted={}\treused={}\tinvalidated={}\treason={}",
        state.journal.build_id(),
        phase,
        ordinal,
        state.target_count,
        state.committed,
        state.reused,
        state.invalidated,
        reason.replace(['\t', '\n'], " ")
    );
}

fn flush_target_checkpoints(state: &mut ResumeScan) {
    if state.pending.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut state.pending);
    state.pending_bytes = 0;
    let count = pending.len();
    let ordinal = pending
        .last()
        .map_or(0, |completed| completed.target.ordinal as usize);
    match state.journal.checkpoint_targets(&pending) {
        Ok(attribution) => {
            state.committed += count;
            state.checkpoint.batches = state.checkpoint.batches.saturating_add(1);
            state.checkpoint.frame_bytes = state
                .checkpoint
                .frame_bytes
                .saturating_add(attribution.frame_bytes);
            state.checkpoint.begin_us = state
                .checkpoint
                .begin_us
                .saturating_add(attribution.begin_us);
            state.checkpoint.compress_us = state
                .checkpoint
                .compress_us
                .saturating_add(attribution.compress_us);
            state.checkpoint.append_us = state
                .checkpoint
                .append_us
                .saturating_add(attribution.append_us);
            state.checkpoint.sync_us = state.checkpoint.sync_us.saturating_add(attribution.sync_us);
            state.checkpoint.rows_us = state.checkpoint.rows_us.saturating_add(attribution.rows_us);
            state.checkpoint.commit_us = state
                .checkpoint
                .commit_us
                .saturating_add(attribution.commit_us);
            state.checkpoint.write_us = state
                .checkpoint
                .write_us
                .saturating_add(attribution.total_us);
            report_resume(
                state,
                "targets-committed",
                ordinal,
                &format!("durable-batch:{count}"),
            );
        }
        Err(error) => {
            state.checkpoint.errors = state.checkpoint.errors.saturating_add(1);
            report_resume(state, "checkpoint-failed", ordinal, &error);
        }
    }
}

fn queue_target_checkpoint(
    state: &mut ResumeScan,
    completed: crate::build_progress::CompletedTarget,
) {
    state.pending_bytes = state
        .pending_bytes
        .saturating_add(completed.output_json.len());
    state.checkpoint.queued_targets = state.checkpoint.queued_targets.saturating_add(1);
    state.checkpoint.queued_bytes = state
        .checkpoint
        .queued_bytes
        .saturating_add(completed.output_json.len());
    state.pending.push(completed);
    if state.pending.len() >= RESUME_CHECKPOINT_TARGET_BATCH
        || state.pending_bytes >= RESUME_CHECKPOINT_MAX_BYTES
    {
        flush_target_checkpoints(state);
    }
}

fn prepare_resume_scan(
    cfg: &BenchConfig,
    plan: &launch_profiles::CatalogScanPlan,
    excluded_targets: &[PathBuf],
    priority: LibraryScanPriority,
    durable_resume: bool,
) -> (Option<ResumeScan>, ResumeValidationAttribution) {
    let open_started = Instant::now();
    let mut attribution = ResumeValidationAttribution {
        enabled: durable_resume,
        ..ResumeValidationAttribution::default()
    };
    if !durable_resume {
        attribution.open_us = open_started.elapsed().as_micros() as u64;
        return (None, attribution);
    }
    let descriptors =
        catalog_scan::planned_scan_target_descriptors(&cfg.roots, plan, excluded_targets);
    let targets: Vec<_> = descriptors.iter().map(progress_target).collect();
    let active_manifest_generation = crate::shard_registry::read_latest_manifest_lazy(
        &crate::catalog_config::default_sharded_catalog_path(),
        crate::shard_registry::production_registry_limits(),
    )
    .ok()
    .map(|manifest| manifest.generation);
    let contract = crate::build_progress::BuildContract {
        active_manifest_generation,
        roots: cfg.roots.clone(),
        path_mapping: crate::catalog_config::library_path_map_from_env()
            .into_iter()
            .map(|rule| (rule.from, rule.to))
            .collect(),
        scanner_version: crate::catalog_config::SCHEMA_VERSION,
        profile_version: launch_profiles::PROFILE_SET_VERSION.to_string(),
        taxonomy_version: crate::catalog_classify::SYSTEM_TAXONOMY_VERSION.to_string(),
        namespace_backend: std::env::var("MISTER_NAMESPACE_BACKEND")
            .unwrap_or_else(|_| "default".to_string()),
        projection_contract: crate::sharded_catalog::PRODUCTION_PROJECTION_CONTRACT.to_string(),
        rom_inventory_fingerprint:
            crate::arcade_rom_inventory::ArcadeRomInventory::from_library_roots(&cfg.roots)
                .fingerprint()
                .to_string(),
    };
    let path = crate::catalog_config::default_build_progress_path();
    let committed_path = crate::catalog_config::default_builder_state_path();
    let had_committed_state = committed_path.exists();
    attribution.committed_state_present = had_committed_state;
    match crate::build_progress::seed_from_committed(&committed_path, &path) {
        Ok(seeded) => attribution.committed_state_seeded = seeded,
        Err(error) => {
            crate::catalog_logln!(
                "catalog_resume_tsv\tphase=committed-state-disabled\treason={}",
                error.replace(['\t', '\n'], " ")
            );
            attribution.setup_errors = attribution.setup_errors.saturating_add(1);
        }
    }
    let (journal, status) = match crate::build_progress::BuildProgressJournal::open_or_create(
        &path, &contract, &targets,
    ) {
        Ok(opened) => opened,
        Err(error) => {
            crate::catalog_logln!(
                "catalog_resume_tsv\tphase=journal-disabled\treason={}",
                error.replace(['\t', '\n'], " ")
            );
            attribution.open_us = open_started.elapsed().as_micros() as u64;
            attribution.setup_errors = attribution.setup_errors.saturating_add(1);
            return (None, attribution);
        }
    };
    attribution.open_us = open_started.elapsed().as_micros() as u64;
    let decode_started = Instant::now();
    let decoded = journal.completed_targets();
    let decode_us = decode_started.elapsed().as_micros() as u64;
    attribution.frame_decode_us = decode_us;
    let completed: HashMap<_, _> = decoded
        .unwrap_or_default()
        .into_iter()
        .map(|target| (target.target.ordinal, target))
        .collect();
    attribution.committed_targets = completed.len();
    let validation_started = Instant::now();
    attribution.validation_backend = "walker-native";
    let (fingerprints, validation_namespace) = if completed.is_empty() {
        (
            HashMap::new(),
            catalog_scan::NamespaceRouteAttribution::default(),
        )
    } else {
        validate_target_fingerprints_in_walker(
            cfg,
            plan,
            excluded_targets,
            priority,
            &completed,
            &mut attribution,
        )
    };
    attribution.validation_us = validation_started.elapsed().as_micros() as u64;
    attribution.validated_targets = fingerprints.len();
    attribution.unavailable_targets = completed.len().saturating_sub(fingerprints.len());
    attribution.error_targets = attribution
        .error_targets
        .saturating_add(validation_namespace.aborted_targets);
    attribution.namespace = validation_namespace;
    let reusable: HashMap<u32, crate::build_progress::CompletedTarget> = completed
        .iter()
        .filter(|(ordinal, saved)| fingerprints.get(ordinal) == Some(&saved.input_fingerprint))
        .map(|(ordinal, saved)| (*ordinal, saved.clone()))
        .collect();
    let durable_completed = completed.len();
    let invalidated_targets = completed
        .keys()
        .filter(|ordinal| !reusable.contains_key(ordinal))
        .copied()
        .collect::<BTreeSet<_>>();
    attribution.reused_targets = reusable.len();
    attribution.invalidated_targets = invalidated_targets.len();
    let affected_systems = completed
        .iter()
        .filter(|(ordinal, _)| invalidated_targets.contains(ordinal))
        .flat_map(|(_, completed)| target_output_systems(&completed.output_json))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let all_published_systems = !had_committed_state
        || matches!(
            &status,
            crate::build_progress::OpenStatus::Created
                | crate::build_progress::OpenStatus::Recreated { .. }
        );
    let state = ResumeScan {
        journal,
        reusable,
        invalidated_targets,
        affected_systems,
        all_published_systems,
        target_count: targets.len(),
        reused: 0,
        invalidated: 0,
        committed: durable_completed,
        pending: Vec::with_capacity(RESUME_CHECKPOINT_TARGET_BATCH),
        pending_bytes: 0,
        checkpoint: CheckpointAttribution {
            enabled: true,
            decode_us,
            ..CheckpointAttribution::default()
        },
    };
    report_resume(&state, "journal-open", 0, &format!("{status:?}"));
    (Some(state), attribution)
}

fn target_output_systems(output_json: &str) -> BTreeSet<String> {
    serde_json::from_str::<TargetOutput>(output_json)
        .map(|output| {
            output
                .discoveries
                .iter()
                .map(catalog_system_id_for_discovery)
                .filter(|system_id| is_reportable_catalog_system_id(system_id))
                .collect()
        })
        .unwrap_or_default()
}

fn validate_target_fingerprints_in_walker(
    cfg: &BenchConfig,
    plan: &launch_profiles::CatalogScanPlan,
    excluded_targets: &[PathBuf],
    priority: LibraryScanPriority,
    completed: &HashMap<u32, crate::build_progress::CompletedTarget>,
    attribution: &mut ResumeValidationAttribution,
) -> (
    HashMap<u32, String>,
    catalog_scan::NamespaceRouteAttribution,
) {
    let descriptors =
        catalog_scan::planned_scan_target_descriptors(cfg.roots.as_slice(), plan, excluded_targets);
    let completed_paths: BTreeSet<_> = completed
        .values()
        .map(|saved| saved.target.path.as_str())
        .collect();
    let mut validation_exclusions = excluded_targets.to_vec();
    validation_exclusions.extend(
        descriptors
            .iter()
            .filter(|descriptor| {
                !completed_paths.contains(descriptor.path.to_string_lossy().as_ref())
            })
            .map(|descriptor| descriptor.path.clone()),
    );
    let role = match priority {
        LibraryScanPriority::Background => crate::runtime_thread::RuntimeThreadRole::LibraryWalker,
        LibraryScanPriority::Foreground => {
            crate::runtime_thread::RuntimeThreadRole::LibraryWalkerForeground
        }
    };
    let worker = catalog_scan::fingerprint_resume_targets(
        cfg.roots.clone(),
        plan.clone(),
        validation_exclusions,
        role,
    );
    let join_started = Instant::now();
    let Ok(walk) = worker.join() else {
        attribution.error_targets = attribution.error_targets.saturating_add(completed.len());
        return (
            HashMap::new(),
            catalog_scan::NamespaceRouteAttribution::default(),
        );
    };
    attribution.validation_join_us = join_started.elapsed().as_micros() as u64;
    let consumer_started = Instant::now();
    let fingerprints = walk
        .fingerprints
        .into_iter()
        .filter_map(|(descriptor, fingerprint)| {
            completed
                .values()
                .find(|saved| {
                    saved.target.path == descriptor.path.to_string_lossy()
                        && saved
                            .target
                            .key
                            .starts_with(&format!("{:?}:", descriptor.kind))
                })
                .map(|saved| (saved.target.ordinal, fingerprint))
        })
        .collect();
    attribution.validation_consumer_us = consumer_started.elapsed().as_micros() as u64;
    (fingerprints, walk.attribution)
}

#[derive(Default)]
struct FileDiscoveryTimingBucket {
    elapsed_us: u64,
    calls: usize,
    max_us: u64,
}

impl ScanTimingStats {
    fn record_file_discovery(&mut self, profile_id: &str, extension: &str, elapsed_us: u64) {
        self.file_discovery_us = self.file_discovery_us.saturating_add(elapsed_us);
        self.file_discovery_count = self.file_discovery_count.saturating_add(1);

        // Avoid allocating the profile/extension strings for every file. The
        // common path performs borrowed lookups and allocates only when a new
        // aggregate bucket is first observed.
        let extensions = if let Some(extensions) = self.file_discovery_breakdown.get_mut(profile_id)
        {
            extensions
        } else {
            self.file_discovery_breakdown
                .insert(profile_id.to_string(), HashMap::new());
            self.file_discovery_breakdown
                .get_mut(profile_id)
                .expect("inserted file-discovery profile timing bucket")
        };
        let bucket = if let Some(bucket) = extensions.get_mut(extension) {
            bucket
        } else {
            extensions.insert(extension.to_string(), FileDiscoveryTimingBucket::default());
            extensions
                .get_mut(extension)
                .expect("inserted file-discovery extension timing bucket")
        };
        bucket.elapsed_us = bucket.elapsed_us.saturating_add(elapsed_us);
        bucket.calls = bucket.calls.saturating_add(1);
        bucket.max_us = bucket.max_us.max(elapsed_us);
    }

    fn report_file_discovery_breakdown(&self) {
        let mut buckets = self
            .file_discovery_breakdown
            .iter()
            .flat_map(|(profile_id, extensions)| {
                extensions
                    .iter()
                    .map(move |(extension, bucket)| (profile_id, extension, bucket))
            })
            .collect::<Vec<_>>();
        buckets.sort_unstable_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        for (profile_id, extension, bucket) in buckets {
            library_db::report_library_scan_timing(
                "file_discovery_profile_ext",
                bucket.elapsed_us,
                format!(
                    "profile={} ext={} source_class={} calls={} avg_us={} max_us={}",
                    profile_id,
                    extension,
                    file_discovery_source_class(extension),
                    bucket.calls,
                    bucket.elapsed_us / bucket.calls.max(1) as u64,
                    bucket.max_us,
                ),
            );
        }
    }
}

fn file_discovery_source_class(extension: &str) -> &'static str {
    if extension.eq_ignore_ascii_case("mra") {
        "mra-metadata"
    } else if extension.eq_ignore_ascii_case("mgl") {
        "mgl-metadata"
    } else {
        "path-derived"
    }
}

struct LibraryScanExecution<'a> {
    cfg: &'a BenchConfig,
    archive_reader: &'a crate::catalog_config::ArchiveReaderConfig,
    priority: LibraryScanPriority,
    audit_mode: CoverageAuditMode,
    durable_resume: bool,
    arcade_updater_index: Option<&'a Path>,
    enforce_arcade_rom_presence: bool,
}

fn is_arcade_bootstrap_scan(roots: &[String], durable_resume: bool) -> bool {
    !durable_resume
        && roots.len() == 1
        && roots[0].eq_ignore_ascii_case(crate::arcade_catalog::DEFAULT_ARCADE_ROOT)
}

struct ArcadeMraPrefetch {
    inspections: HashMap<PathBuf, Option<media_metadata::MraInspection>>,
    index_status: &'static str,
    index_path: Option<String>,
    index_error: Option<String>,
    index_rows: usize,
    index_file_sha256: Option<String>,
    index_hits: usize,
    index_misses: usize,
    fallback_reads: usize,
    identity_stats: usize,
    identity_stat_failures: usize,
    identity_stat_us: u64,
    index_load_us: u64,
}

fn prefetch_arcade_mra_metadata(
    events: &[DiscoveryEvent],
    updater_index_path: Option<&Path>,
) -> ArcadeMraPrefetch {
    let paths = events
        .iter()
        .filter_map(|event| match event {
            DiscoveryEvent::File(file) if file.ext.eq_ignore_ascii_case("mra") => {
                Some((file.path.clone(), file.size))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return ArcadeMraPrefetch {
            inspections: HashMap::new(),
            index_status: "unused",
            index_path: updater_index_path.map(|path| path.display().to_string()),
            index_error: None,
            index_rows: 0,
            index_file_sha256: None,
            index_hits: 0,
            index_misses: 0,
            fallback_reads: 0,
            identity_stats: 0,
            identity_stat_failures: 0,
            identity_stat_us: 0,
            index_load_us: 0,
        };
    }

    let load_started = Instant::now();
    let index_path = updater_index_path.map(|path| path.display().to_string());
    let (loaded_index, index_status, index_error, index_file_sha256) = match updater_index_path {
        None => (None, "disabled", None, None),
        Some(path) => {
            match crate::arcade_updater_index::ArcadeUpdaterIndex::read_with_file_sha256(path) {
                Ok((index, sha256)) => (Some(index), "loaded", None, Some(sha256)),
                Err(error) => {
                    let status = if std::fs::metadata(path).is_err_and(|metadata_error| {
                        metadata_error.kind() == std::io::ErrorKind::NotFound
                    }) {
                        "missing"
                    } else {
                        "invalid"
                    };
                    (None, status, Some(error), None)
                }
            }
        }
    };
    let index_load_us = load_started.elapsed().as_micros() as u64;
    let index_rows = loaded_index.as_ref().map_or(0, |index| index.rows.len());
    let indexed_rows = loaded_index
        .map(|index| {
            index
                .rows
                .into_iter()
                .map(|row| (row.path.to_ascii_lowercase(), row))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut inspections = HashMap::with_capacity(paths.len());
    let mut fallback_paths = Vec::new();
    let mut index_hits = 0usize;
    let mut identity_stats = 0usize;
    let mut identity_stat_failures = 0usize;
    let mut identity_stat_us = 0u64;
    for (path, size) in paths {
        let indexed = arcade_updater_key(&path)
            .and_then(|key| indexed_rows.get(&key))
            .filter(|row| row.primary_rom != media_metadata::PrimaryRomRequirement::Ambiguous)
            .filter(|row| {
                let observed_size = if size == 0 {
                    let stat_started = Instant::now();
                    identity_stats = identity_stats.saturating_add(1);
                    let observed = std::fs::metadata(&path).map(|metadata| metadata.len());
                    identity_stat_us =
                        identity_stat_us.saturating_add(stat_started.elapsed().as_micros() as u64);
                    match observed {
                        Ok(observed) => observed,
                        Err(_) => {
                            identity_stat_failures = identity_stat_failures.saturating_add(1);
                            return false;
                        }
                    }
                } else {
                    size
                };
                observed_size == row.size
            });
        if let Some(row) = indexed {
            inspections.insert(
                path,
                Some(media_metadata::MraInspection {
                    header: row.header.clone(),
                    primary_rom: row.primary_rom.clone(),
                    catalog_metadata: row.catalog_metadata.clone(),
                }),
            );
            index_hits = index_hits.saturating_add(1);
        } else {
            fallback_paths.push(path);
        }
    }

    let worker_count = ARCADE_MRA_READ_WORKERS.min(fallback_paths.len());
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(fallback_paths.len()));
    std::thread::scope(|scope| {
        for worker in 0..worker_count {
            let paths = &fallback_paths;
            let next = &next;
            let results = &results;
            let _ = std::thread::Builder::new()
                .name(format!("arcade-mra-read-{worker}"))
                .spawn_scoped(scope, move || {
                    crate::runtime_thread::apply_runtime_thread_policy(
                        crate::runtime_thread::RuntimeThreadRole::LibraryWalker,
                    );
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = paths.get(index) else {
                            break;
                        };
                        let inspection = media_metadata::inspect_mra_path(path).ok();
                        if let Ok(mut results) = results.lock() {
                            results.push((path.clone(), inspection));
                        }
                    }
                });
        }
    });
    inspections.extend(results.into_inner().unwrap_or_default());
    ArcadeMraPrefetch {
        inspections,
        index_status,
        index_path,
        index_error,
        index_rows,
        index_file_sha256,
        index_hits,
        index_misses: fallback_paths.len(),
        fallback_reads: fallback_paths.len(),
        identity_stats,
        identity_stat_failures,
        identity_stat_us,
        index_load_us,
    }
}

fn arcade_updater_key(path: &Path) -> Option<String> {
    let components = path
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?;
    let arcade = components
        .iter()
        .position(|component| component.eq_ignore_ascii_case("_Arcade"))?;
    Some(components[arcade..].join("/").to_ascii_lowercase())
}

fn apply_configured_target_allowlist(
    roots: &[String],
    plan: &launch_profiles::CatalogScanPlan,
    excluded_targets: &mut Vec<PathBuf>,
) {
    let Some(value) = std::env::var_os("MISTER_LIBRARY_TARGET_ALLOWLIST") else {
        return;
    };
    let allowed = std::env::split_paths(&value)
        .map(|path| path.to_string_lossy().to_ascii_lowercase())
        .filter(|path| !path.is_empty())
        .collect::<BTreeSet<_>>();
    if allowed.is_empty() {
        return;
    }
    let descriptors = catalog_scan::planned_scan_target_descriptors(roots, plan, excluded_targets);
    let mut added = 0usize;
    for descriptor in descriptors {
        let key = descriptor.path.to_string_lossy().to_ascii_lowercase();
        if !allowed.contains(&key)
            && !excluded_targets
                .iter()
                .any(|path| path.to_string_lossy().eq_ignore_ascii_case(&key))
        {
            excluded_targets.push(descriptor.path);
            added = added.saturating_add(1);
        }
    }
    library_db::report_library_scan_timing(
        "catalog_target_allowlist",
        0,
        format!("allowed={} excluded={added}", allowed.len()),
    );
}

fn scan_library_with_progress_and_events(
    execution: LibraryScanExecution<'_>,
    mut progress: ProgressCallback<'_>,
    mut scan_events: ScanEventCallback<'_>,
    mut excluded_targets: Vec<PathBuf>,
) -> LibraryScan {
    let LibraryScanExecution {
        cfg,
        archive_reader,
        priority,
        audit_mode,
        durable_resume,
        arcade_updater_index,
        enforce_arcade_rom_presence,
    } = execution;
    crate::cooperative_work::checkpoint();
    let discover_t = Instant::now();
    let plan_t = Instant::now();
    let plan = launch_profiles::CatalogScanPlan::for_roots(&cfg.roots);
    apply_configured_target_allowlist(&cfg.roots, &plan, &mut excluded_targets);
    crate::cooperative_work::checkpoint();
    let plan_us = plan_t.elapsed().as_micros() as u64;
    library_db::report_library_scan_timing(
        "catalog_scan_plan",
        plan_us,
        format!(
            "base_profiles={} installed_cores={} runtime_dirs={}",
            plan.base_profiles().len(),
            plan.installed_cores().len(),
            plan.game_dir_headers().len(),
        ),
    );
    let resume_started = Instant::now();
    let (mut resume, mut resume_attribution) =
        prepare_resume_scan(cfg, &plan, &excluded_targets, priority, durable_resume);
    let resume_us = resume_started.elapsed().as_micros() as u64;
    if let (Some(state), Some(report)) = (resume.as_ref(), scan_events.as_mut()) {
        report(LibraryScanEvent::ReconciliationPlanReady {
            system_ids: state.affected_systems.clone(),
            all_published_systems: state.all_published_systems,
        });
    }
    let target_descriptors =
        catalog_scan::planned_scan_target_descriptors(&cfg.roots, &plan, &excluded_targets);
    let target_count = target_descriptors.len();
    let mut prepared_helpers = load_prepared_target_helpers(&target_descriptors);
    library_db::report_library_scan_timing(
        "prepared_bundle_helpers",
        0,
        format!(
            "checked={} matched={} rejected={}",
            prepared_helpers.checked, prepared_helpers.matched, prepared_helpers.rejected
        ),
    );
    let mut contributor_closure =
        ContributorClosure::new(target_descriptors.iter().map(|descriptor| {
            (
                descriptor.ordinal,
                catalog_scan::profile_for_path(plan.base_profiles(), &descriptor.path)
                    .map(|profile| profile.system_id.clone()),
            )
        }));
    let prepared_payload_t = Instant::now();
    let prepared_payload_index = PreparedPayloadIndex::from_library_roots(&cfg.roots);
    let prepared_payload_us = prepared_payload_t.elapsed().as_micros() as u64;
    crate::cooperative_work::checkpoint();
    library_db::report_library_scan_timing(
        "prepared_payload_index",
        prepared_payload_us,
        format!(
            "files={} complete_roots={} release={}",
            prepared_payload_index.file_count(),
            prepared_payload_index.complete_root_count(),
            crate::prepared_release_manifest::zero_mhz_release_id().unwrap_or("unavailable"),
        ),
    );
    let mut prevalidated_targets = resume
        .as_ref()
        .map(|state| {
            state
                .reusable
                .values()
                .map(|saved| PathBuf::from(&saved.target.path))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let pruned_paths = prepared_helpers
        .reusable
        .values()
        .filter_map(|helper| helper.scan_exclusion_path.as_deref().map(PathBuf::from))
        .collect::<Vec<_>>();
    prevalidated_targets.extend(
        prepared_helpers
            .reusable
            .values()
            .filter(|helper| helper.scan_exclusion_path.is_none())
            .map(|helper| PathBuf::from(&helper.target_path)),
    );
    let pipeline_started = Instant::now();
    let rx = match priority {
        LibraryScanPriority::Background => catalog_scan::discover_files_pipelined_with_plan(
            cfg.roots.clone(),
            plan.clone(),
            excluded_targets,
            prevalidated_targets,
            pruned_paths,
            crate::runtime_thread::RuntimeThreadRole::LibraryWalker,
        ),
        LibraryScanPriority::Foreground => {
            catalog_scan::discover_files_pipelined_foreground_with_plan(
                cfg.roots.clone(),
                plan.clone(),
                excluded_targets,
                prevalidated_targets,
                pruned_paths,
            )
        }
    };
    let mut buffered_events = None;
    let rom_inventory =
        crate::arcade_rom_inventory::ArcadeRomInventory::from_library_roots(&cfg.roots);
    let (mame_roms, hbmame_roms) = rom_inventory.counts();
    library_db::report_library_scan_timing(
        "arcade_rom_inventory",
        rom_inventory.scan_us,
        format!(
            "mame={} hbmame={} fingerprint={}",
            mame_roms,
            hbmame_roms,
            rom_inventory.fingerprint()
        ),
    );
    let mut prefetched_arcade_mra = HashMap::new();
    if is_arcade_bootstrap_scan(&cfg.roots, durable_resume) {
        let events = rx.iter().collect::<Vec<_>>();
        let prefetch_t = Instant::now();
        let prefetch = prefetch_arcade_mra_metadata(&events, arcade_updater_index);
        prefetched_arcade_mra = prefetch.inspections;
        let successes = prefetched_arcade_mra
            .values()
            .filter(|inspection| inspection.is_some())
            .count();
        library_db::report_library_scan_timing(
            "arcade_mra_prefetch",
            prefetch_t.elapsed().as_micros() as u64,
            format!(
                "files={} successes={} failures={} workers={} index_status={} index_path={} index_error={} index_rows={} index_file_sha256={} index_hits={} index_misses={} fallback_reads={} identity_stats={} identity_stat_failures={} identity_stat_us={} index_load_us={}",
                prefetched_arcade_mra.len(),
                successes,
                prefetched_arcade_mra.len().saturating_sub(successes),
                ARCADE_MRA_READ_WORKERS.min(prefetch.fallback_reads),
                prefetch.index_status,
                prefetch.index_path.as_deref().unwrap_or("none"),
                prefetch
                    .index_error
                    .as_deref()
                    .map(sanitize_arcade_index_metric)
                    .as_deref()
                    .unwrap_or("none"),
                prefetch.index_rows,
                prefetch.index_file_sha256.as_deref().unwrap_or("none"),
                prefetch.index_hits,
                prefetch.index_misses,
                prefetch.fallback_reads,
                prefetch.identity_stats,
                prefetch.identity_stat_failures,
                prefetch.identity_stat_us,
                prefetch.index_load_us,
            ),
        );
        buffered_events = Some(events.into_iter());
    }
    let mut game_dir_facts = Vec::with_capacity(plan.game_dir_headers().len());
    let mut profiles = plan.base_profiles().to_vec();
    let mut discover_us = 0;
    let mut execution_attribution = catalog_scan::NamespaceRouteAttribution::default();

    let mut normal_files = Vec::new();
    let mut containers = Vec::new();
    let mut entries = Vec::new();
    let mut ignored_files = 0usize;
    let mut discoveries = Vec::new();
    let mut arcade_mra_eligible = 0usize;
    let mut arcade_mra_missing_rom = 0usize;
    let mut arcade_mra_ambiguous = 0usize;
    let mut arcade_mra_malformed = 0usize;
    let classify_t = Instant::now();
    let mut timing = ScanTimingStats::default();
    let mut idx = 0usize;
    let mut first_discovery_reported = false;
    let mut discovered_systems = BTreeSet::new();
    let mut scanning_systems = BTreeSet::new();
    let mut target_descriptor: Option<catalog_scan::ScanTargetDescriptor> = None;
    let mut target_offsets: Option<TargetOffsets> = None;
    let mut target_consumer_started_at: Option<Instant> = None;
    let mut target_consumer_first_work_us = None;
    let mut target_fingerprint = Fingerprint::new();
    let mut skip_target = false;
    let mut target_checkpointable = true;
    let mut checkpoint_attribution = CheckpointAttribution {
        enabled: durable_resume,
        ..CheckpointAttribution::default()
    };
    let mut resumed_output_decode_us = 0u64;
    let mut resumed_output_decode_bytes = 0usize;
    let mut resumed_output_decode_targets = 0usize;
    let mut handoff_attribution = ScanHandoffAttribution::default();
    let mut system_finality = BTreeMap::<String, (usize, u64, usize)>::new();
    let mut last_target_heartbeat = Instant::now();
    let handoff_loop_started = Instant::now();
    loop {
        let receive_started = Instant::now();
        let event = match buffered_events.as_mut() {
            Some(events) => events.next(),
            None => rx.recv().ok(),
        };
        handoff_attribution.receive_wait_us = handoff_attribution
            .receive_wait_us
            .saturating_add(receive_started.elapsed().as_micros() as u64);
        let Some(event) = event else {
            if let (Some(descriptor), Some(offsets)) = (target_descriptor.as_ref(), target_offsets)
            {
                offsets.rollback(
                    &mut game_dir_facts,
                    &mut normal_files,
                    &mut containers,
                    &mut entries,
                    &mut ignored_files,
                    &mut discoveries,
                    &mut idx,
                    &mut arcade_mra_eligible,
                    &mut arcade_mra_missing_rom,
                    &mut arcade_mra_ambiguous,
                    &mut arcade_mra_malformed,
                    &mut first_discovery_reported,
                );
                crate::catalog_logln!(
                    "catalog_target_abort_tsv\tordinal={}\treason=channel-terminated\tdiscoveries={}\tpath={}",
                    descriptor.ordinal,
                    discoveries.len(),
                    descriptor
                        .path
                        .display()
                        .to_string()
                        .replace(['\t', '\n', '\r', ' '], "_"),
                );
                panic!(
                    "catalog discovery channel terminated inside target {}",
                    descriptor.ordinal
                );
            }
            break;
        };
        if target_consumer_first_work_us.is_none()
            && matches!(
                &event,
                DiscoveryEvent::File(_)
                    | DiscoveryEvent::GameDirFacts(_)
                    | DiscoveryEvent::RuntimeDirectory(_)
            )
            && let Some(started) = target_consumer_started_at
        {
            target_consumer_first_work_us = Some(started.elapsed().as_micros() as u64);
        }
        handoff_attribution.events = handoff_attribution.events.saturating_add(1);
        let consumer_active_started = Instant::now();
        crate::cooperative_work::checkpoint();
        if last_target_heartbeat.elapsed() >= std::time::Duration::from_secs(30)
            && let Some(descriptor) = target_descriptor.as_ref()
        {
            report_scan_target(
                &mut scan_events,
                descriptor,
                target_count,
                priority,
                "heartbeat",
                descriptor.ordinal,
                discoveries.len(),
            );
            last_target_heartbeat = Instant::now();
        }
        let mut done = false;
        let files = match event {
            DiscoveryEvent::TargetStart(descriptor) => {
                assert!(
                    target_descriptor.is_none(),
                    "catalog target started before the prior target completed"
                );
                handoff_attribution.target_events =
                    handoff_attribution.target_events.saturating_add(1);
                report_scan_target(
                    &mut scan_events,
                    &descriptor,
                    target_count,
                    priority,
                    "started",
                    descriptor.ordinal,
                    discoveries.len(),
                );
                last_target_heartbeat = Instant::now();
                target_fingerprint = Fingerprint::for_descriptor(&descriptor);
                target_checkpointable = true;
                target_offsets = Some(
                    TargetOffsets::capture(
                        &game_dir_facts,
                        &normal_files,
                        &containers,
                        &entries,
                        ignored_files,
                        &discoveries,
                    )
                    .with_counters(
                        idx,
                        arcade_mra_eligible,
                        arcade_mra_missing_rom,
                        arcade_mra_ambiguous,
                        arcade_mra_malformed,
                        first_discovery_reported,
                    ),
                );
                skip_target = false;
                if let Some(state) = resume.as_mut()
                    && state
                        .invalidated_targets
                        .remove(&(descriptor.ordinal as u32))
                {
                    state.invalidated += 1;
                    report_resume(
                        state,
                        "target-invalidated",
                        descriptor.ordinal,
                        "fingerprint-changed",
                    );
                }
                if let Some(helper) = prepared_helpers
                    .reusable
                    .remove(&normalized_target_key(&descriptor.path))
                {
                    let partial_helper = helper.scan_exclusion_path.is_some();
                    let output_decode_started = Instant::now();
                    resumed_output_decode_bytes =
                        resumed_output_decode_bytes.saturating_add(helper.output_json.len());
                    resumed_output_decode_targets = resumed_output_decode_targets.saturating_add(1);
                    match serde_json::from_str::<TargetOutput>(&helper.output_json) {
                        Ok(output) => {
                            let first = discoveries.len();
                            game_dir_facts.extend(output.game_dir_facts);
                            normal_files.extend(output.normal_files);
                            containers.extend(output.containers);
                            entries.extend(output.entries);
                            ignored_files = ignored_files.saturating_add(output.ignored_files);
                            discoveries.extend(output.discoveries);
                            profiles = plan.finalize_profiles(&game_dir_facts);
                            report_resumed_systems(
                                &discoveries[first..],
                                &mut discovered_systems,
                                &mut scanning_systems,
                                &mut scan_events,
                            );
                            skip_target = !partial_helper;
                            if partial_helper {
                                target_checkpointable = false;
                            }
                            crate::catalog_logln!(
                                "prepared_bundle_helper_tsv\tstate=activated\tordinal={}\tdiscoveries={}\tpath={}",
                                descriptor.ordinal,
                                discoveries.len().saturating_sub(first),
                                descriptor.path.display()
                            );
                        }
                        Err(error) => {
                            crate::catalog_logln!(
                                "prepared_bundle_helper_tsv\tstate=fallback\treason=output_decode_{}\tpath={}",
                                error.to_string().replace(['\t', '\n', '\r', ' '], "_"),
                                descriptor.path.display()
                            );
                        }
                    }
                    resumed_output_decode_us = resumed_output_decode_us
                        .saturating_add(output_decode_started.elapsed().as_micros() as u64);
                }
                if !skip_target
                    && let Some(saved) = resume
                        .as_mut()
                        .and_then(|state| state.reusable.remove(&(descriptor.ordinal as u32)))
                {
                    let output_decode_started = Instant::now();
                    resumed_output_decode_bytes =
                        resumed_output_decode_bytes.saturating_add(saved.output_json.len());
                    resumed_output_decode_targets = resumed_output_decode_targets.saturating_add(1);
                    match serde_json::from_str::<TargetOutput>(&saved.output_json) {
                        Ok(output) => {
                            let first = discoveries.len();
                            game_dir_facts.extend(output.game_dir_facts);
                            normal_files.extend(output.normal_files);
                            containers.extend(output.containers);
                            entries.extend(output.entries);
                            ignored_files = ignored_files.saturating_add(output.ignored_files);
                            discoveries.extend(output.discoveries);
                            profiles = plan.finalize_profiles(&game_dir_facts);
                            report_resumed_systems(
                                &discoveries[first..],
                                &mut discovered_systems,
                                &mut scanning_systems,
                                &mut scan_events,
                            );
                            skip_target = true;
                            if let Some(state) = resume.as_mut() {
                                state.reused += 1;
                                report_resume(
                                    state,
                                    "target-reused",
                                    descriptor.ordinal,
                                    "fingerprint-match",
                                );
                            }
                        }
                        Err(error) => {
                            if let Some(state) = resume.as_mut() {
                                state.invalidated += 1;
                                report_resume(
                                    state,
                                    "target-invalidated",
                                    descriptor.ordinal,
                                    &format!("decode-error:{error}"),
                                );
                            }
                        }
                    }
                    resumed_output_decode_us = resumed_output_decode_us
                        .saturating_add(output_decode_started.elapsed().as_micros() as u64);
                }
                target_descriptor = Some(descriptor);
                target_consumer_started_at = Some(Instant::now());
                target_consumer_first_work_us = None;
                Vec::new()
            }
            DiscoveryEvent::TargetRestart(restart) => {
                handoff_attribution.target_events =
                    handoff_attribution.target_events.saturating_add(1);
                let active = target_descriptor
                    .as_ref()
                    .expect("catalog target restart must follow TargetStart");
                assert_eq!(
                    active, &restart.descriptor,
                    "catalog target restart identity must match the active target"
                );
                let offsets =
                    target_offsets.expect("catalog target restart must retain target offsets");
                offsets.rollback(
                    &mut game_dir_facts,
                    &mut normal_files,
                    &mut containers,
                    &mut entries,
                    &mut ignored_files,
                    &mut discoveries,
                    &mut idx,
                    &mut arcade_mra_eligible,
                    &mut arcade_mra_missing_rom,
                    &mut arcade_mra_ambiguous,
                    &mut arcade_mra_malformed,
                    &mut first_discovery_reported,
                );
                profiles = plan.finalize_profiles(&game_dir_facts);
                target_fingerprint = Fingerprint::for_descriptor(&restart.descriptor);
                target_checkpointable = true;
                skip_target = false;
                target_consumer_started_at = Some(Instant::now());
                target_consumer_first_work_us = None;
                crate::catalog_logln!(
                    "catalog_target_restart_tsv\tordinal={}\treason={}\tdiscoveries={}\tpath={}",
                    restart.descriptor.ordinal,
                    restart.reason.replace(['\t', '\n', '\r', ' '], "_"),
                    discoveries.len(),
                    restart
                        .descriptor
                        .path
                        .display()
                        .to_string()
                        .replace(['\t', '\n', '\r', ' '], "_"),
                );
                report_scan_target(
                    &mut scan_events,
                    &restart.descriptor,
                    target_count,
                    priority,
                    "restarted",
                    restart.descriptor.ordinal,
                    discoveries.len(),
                );
                Vec::new()
            }
            DiscoveryEvent::TargetComplete(descriptor) => {
                handoff_attribution.target_events =
                    handoff_attribution.target_events.saturating_add(1);
                let ready_us = pipeline_started.elapsed().as_micros() as u64;
                let active = target_descriptor
                    .as_ref()
                    .expect("catalog target completion must follow TargetStart");
                assert_eq!(
                    active, &descriptor,
                    "catalog target completion identity must match the active target"
                );
                let offsets =
                    target_offsets.expect("catalog target completion must retain target offsets");
                let target_systems = discoveries[offsets.discoveries..]
                    .iter()
                    .map(catalog_system_id_for_discovery)
                    .filter(|system| is_reportable_catalog_system_id(system))
                    .collect::<BTreeSet<_>>();
                for system in contributor_closure.complete(descriptor.ordinal, &target_systems) {
                    crate::catalog_logln!(
                        "catalog_system_closure_tsv\tsystem={}\tready_us={}\ttarget_ordinal={}\tremaining_unknown=0\tsemantics=conservative-contributor-set",
                        system,
                        ready_us,
                        descriptor.ordinal,
                    );
                }
                for system in &target_systems {
                    system_finality
                        .entry(system.clone())
                        .and_modify(|entry| {
                            entry.0 = descriptor.ordinal;
                            entry.1 = ready_us;
                            entry.2 = entry.2.saturating_add(1);
                        })
                        .or_insert((descriptor.ordinal, ready_us, 1));
                }
                crate::catalog_logln!(
                    "catalog_target_handoff_tsv\tordinal={}\tkind={:?}\tready_us={}\tconsumer_first_work_us={}\tconsumer_complete_us={}\tdiscoveries={}\tsystems={}\tpath={}",
                    descriptor.ordinal,
                    descriptor.kind,
                    ready_us,
                    target_consumer_first_work_us.unwrap_or(0),
                    target_consumer_started_at
                        .map(|started| started.elapsed().as_micros() as u64)
                        .unwrap_or(0),
                    discoveries.len().saturating_sub(offsets.discoveries),
                    if target_systems.is_empty() {
                        "none".to_string()
                    } else {
                        target_systems.into_iter().collect::<Vec<_>>().join(",")
                    },
                    descriptor
                        .path
                        .display()
                        .to_string()
                        .replace(['\t', '\n', '\r'], "_"),
                );
                if !skip_target
                    && target_checkpointable
                    && let Some(offsets) = target_offsets
                {
                    let output = BorrowedTargetOutput {
                        game_dir_facts: &game_dir_facts[offsets.facts..],
                        normal_files: &normal_files[offsets.files..],
                        containers: &containers[offsets.containers..],
                        entries: &entries[offsets.entries..],
                        ignored_files: ignored_files.saturating_sub(offsets.ignored),
                        discoveries: &discoveries[offsets.discoveries..],
                    };
                    if std::env::var_os(PREPARED_HELPER_CAPTURE_DIR_ENV).is_some() {
                        let capture_started = Instant::now();
                        match serde_json::to_string(&output)
                            .map_err(|error| error.to_string())
                            .and_then(|output_json| {
                                capture_prepared_target_helper(
                                    cfg,
                                    &descriptor,
                                    &output,
                                    output_json,
                                )
                            }) {
                            Ok(Some(path)) => crate::catalog_logln!(
                                "prepared_bundle_helper_tsv\tstate=captured\telapsed_us={}\tpath={}",
                                capture_started.elapsed().as_micros(),
                                path.display()
                            ),
                            Ok(None) => {}
                            Err(error) => crate::catalog_logln!(
                                "prepared_bundle_helper_tsv\tstate=capture_failed\treason={}\tpath={}",
                                error.replace(['\t', '\n', '\r', ' '], "_"),
                                descriptor.path.display()
                            ),
                        }
                    }
                    if let Some(state) = resume.as_mut() {
                        let snapshot_started = Instant::now();
                        state.checkpoint.snapshot_us = state
                            .checkpoint
                            .snapshot_us
                            .saturating_add(snapshot_started.elapsed().as_micros() as u64);
                        let encode_started = Instant::now();
                        match serde_json::to_string(&output) {
                            Ok(output_json) => {
                                state.checkpoint.encode_us = state
                                    .checkpoint
                                    .encode_us
                                    .saturating_add(encode_started.elapsed().as_micros() as u64);
                                let completed = crate::build_progress::CompletedTarget {
                                    target: progress_target(&descriptor),
                                    input_fingerprint: target_fingerprint.finish(),
                                    output_json,
                                    accumulated_stats: crate::build_progress::BuildStats {
                                        normal_files: normal_files.len() as u64,
                                        containers: containers.len() as u64,
                                        entries: entries.len() as u64,
                                        audit_rows: 0,
                                        discoveries: discoveries.len() as u64,
                                    },
                                };
                                queue_target_checkpoint(state, completed);
                            }
                            Err(error) => {
                                state.checkpoint.encode_us = state
                                    .checkpoint
                                    .encode_us
                                    .saturating_add(encode_started.elapsed().as_micros() as u64);
                                state.checkpoint.errors = state.checkpoint.errors.saturating_add(1);
                                report_resume(
                                    state,
                                    "checkpoint-failed",
                                    descriptor.ordinal,
                                    &format!("encode-error:{error}"),
                                );
                            }
                        }
                    }
                }
                report_scan_target(
                    &mut scan_events,
                    &descriptor,
                    target_count,
                    priority,
                    "completed",
                    descriptor.ordinal.saturating_add(1),
                    discoveries.len(),
                );
                target_descriptor = None;
                target_offsets = None;
                target_consumer_started_at = None;
                target_consumer_first_work_us = None;
                skip_target = false;
                target_checkpointable = true;
                Vec::new()
            }
            DiscoveryEvent::File(file) => {
                handoff_attribution.file_events = handoff_attribution.file_events.saturating_add(1);
                if skip_target {
                    Vec::new()
                } else {
                    target_fingerprint.file(&file);
                    vec![file]
                }
            }
            DiscoveryEvent::GameDirFacts(facts) => {
                handoff_attribution.facts_events =
                    handoff_attribution.facts_events.saturating_add(1);
                if !skip_target {
                    target_fingerprint.facts(&facts);
                    game_dir_facts.push(facts);
                }
                Vec::new()
            }
            DiscoveryEvent::RuntimeDirectory(runtime) => {
                handoff_attribution.runtime_events =
                    handoff_attribution.runtime_events.saturating_add(1);
                if skip_target {
                    Vec::new()
                } else {
                    target_fingerprint.facts(&runtime.facts);
                    if runtime.overflowed {
                        target_checkpointable = false;
                    }
                    for file in &runtime.files {
                        target_fingerprint.file(file);
                    }
                    game_dir_facts.push(runtime.facts);
                    profiles = plan.finalize_profiles(&game_dir_facts);
                    if runtime.overflowed {
                        library_db::report_library_scan_timing(
                            "runtime_buffer_overflow",
                            0,
                            format!("path={}", runtime.header.path.display()),
                        );
                        catalog_scan::collect_runtime_candidates_after_overflow(
                            &runtime.header,
                            &profiles,
                        )
                    } else {
                        runtime.files
                    }
                }
            }
            DiscoveryEvent::Done {
                discover_us: us,
                attribution,
                ..
            } => {
                discover_us = us;
                execution_attribution = attribution;
                done = true;
                Vec::new()
            }
        };
        handoff_attribution.files = handoff_attribution.files.saturating_add(files.len());
        handoff_attribution.max_batch = handoff_attribution.max_batch.max(files.len());
        for f in files {
            if idx.is_multiple_of(16) {
                crate::cooperative_work::checkpoint();
            }
            // Runtime directories are buffered before their profile is resolved.
            // Static walker events already satisfy this predicate, so applying it
            // here keeps the original candidate/progress accounting intact.
            if !catalog_scan::is_index_candidate(&profiles, &f.path, &f.ext) {
                continue;
            }
            if idx == 0 {
                library_db::report_library_scan_timing(
                    "first_candidate",
                    classify_t.elapsed().as_micros() as u64,
                    format!("path={}", f.path.display()),
                );
            }
            idx += 1;
            let discoveries_before = discoveries.len();
            let profile_match_t = Instant::now();
            let profile_match = catalog_scan::classify_profile_path(&profiles, &f.path);
            timing.profile_match_us += profile_match_t.elapsed().as_micros() as u64;
            timing.profile_match_count += 1;
            match profile_match {
                Some((
                    profile,
                    ProfilePathClass::Payload {
                        rule:
                            payload_rule @ PayloadRule {
                                disposition: PayloadDisposition::Playable,
                                ..
                            },
                    },
                )) => {
                    if media_metadata::is_amigavision_save_media_path(&f.path) {
                        ignored_files += 1;
                        continue;
                    }
                    let installed_t = Instant::now();
                    let installed =
                        media_metadata::installed_amigavision_discoveries_from_hdf(&f, profile);
                    timing.installed_collection_us += installed_t.elapsed().as_micros() as u64;
                    timing.installed_collection_count += 1;
                    if let Some(installed) = installed {
                        ignored_files += 1;
                        discoveries.extend(installed);
                        continue;
                    }
                    let mut has_archive_entries = false;
                    if !profile.archive_entry_rules.is_empty()
                        && let Some(format) = ArchiveFormat::from_ext(&f.ext)
                    {
                        crate::cooperative_work::checkpoint();
                        let archive_t = Instant::now();
                        let scan = catalog_scan::scan_archive_toc(&f, format, profile);
                        timing.archive_toc_us += archive_t.elapsed().as_micros() as u64;
                        timing.archive_toc_count += 1;
                        has_archive_entries = !scan.entries.is_empty();
                        for entry in scan.entries {
                            discoveries.push(discovery_from_profile_archive_entry(
                                &entry,
                                profile,
                                &entry.rule,
                            ));
                            entries.push(entry);
                        }
                        containers.push(scan.container);
                    }
                    if has_archive_entries {
                        continue;
                    }
                    normal_files.push(LibraryPayloadFile {
                        path: f.path.display().to_string(),
                    });
                    let discovery_t = Instant::now();
                    let prefetched_mra = if f.ext.eq_ignore_ascii_case("mra") {
                        let inspection = match prefetched_arcade_mra.remove(&f.path) {
                            Some(inspection) => inspection,
                            None => media_metadata::inspect_mra_path(&f.path).ok(),
                        };
                        let Some(inspection) = inspection else {
                            arcade_mra_malformed = arcade_mra_malformed.saturating_add(1);
                            ignored_files = ignored_files.saturating_add(1);
                            continue;
                        };
                        match rom_inventory.eligibility(&inspection.primary_rom) {
                            crate::arcade_rom_inventory::RomEligibility::Eligible => {
                                arcade_mra_eligible = arcade_mra_eligible.saturating_add(1);
                                Some(Some(inspection))
                            }
                            _ if !enforce_arcade_rom_presence => {
                                arcade_mra_eligible = arcade_mra_eligible.saturating_add(1);
                                Some(Some(inspection))
                            }
                            crate::arcade_rom_inventory::RomEligibility::Missing => {
                                arcade_mra_missing_rom = arcade_mra_missing_rom.saturating_add(1);
                                ignored_files = ignored_files.saturating_add(1);
                                continue;
                            }
                            crate::arcade_rom_inventory::RomEligibility::Ambiguous => {
                                arcade_mra_ambiguous = arcade_mra_ambiguous.saturating_add(1);
                                ignored_files = ignored_files.saturating_add(1);
                                continue;
                            }
                        }
                    } else {
                        None
                    };
                    discoveries.push(
                        discovery_from_profile_file_with_prepared_index_and_mra_metadata(
                            &f,
                            profile,
                            &payload_rule,
                            &profiles,
                            Some(&prepared_payload_index),
                            prefetched_mra,
                        ),
                    );
                    timing.record_file_discovery(
                        profile.id.as_str(),
                        f.ext.as_str(),
                        discovery_t.elapsed().as_micros() as u64,
                    );
                }
                Some((
                    _,
                    ProfilePathClass::Payload {
                        rule:
                            PayloadRule {
                                disposition: PayloadDisposition::AttachedMedia,
                                ..
                            },
                    },
                )) => {
                    normal_files.push(LibraryPayloadFile {
                        path: f.path.display().to_string(),
                    });
                    ignored_files += 1;
                }
                Some((profile, ProfilePathClass::Collection { rule })) => {
                    crate::cooperative_work::checkpoint();
                    if let Some(format) = ArchiveFormat::from_ext(&f.ext) {
                        containers.push(catalog_scan::scan_container_header(&f, format));
                    }
                    let collection_t = Instant::now();
                    discoveries.extend(media_metadata::collection_discoveries_from_container(
                        &f,
                        profile,
                        &rule,
                        archive_reader,
                    ));
                    timing.collection_listing_us += collection_t.elapsed().as_micros() as u64;
                    timing.collection_listing_count += 1;
                }
                Some((_profile, ProfilePathClass::Ignored { .. })) => {
                    ignored_files += 1;
                }
                Some((profile, ProfilePathClass::NotMatched))
                    if catalog_scan::is_archive_entry_container_candidate(&profiles, &f.path) =>
                {
                    if let Some(format) = ArchiveFormat::from_ext(&f.ext) {
                        let archive_t = Instant::now();
                        let scan = catalog_scan::scan_archive_toc(&f, format, profile);
                        timing.archive_toc_us += archive_t.elapsed().as_micros() as u64;
                        timing.archive_toc_count += 1;
                        for entry in scan.entries {
                            discoveries.push(discovery_from_profile_archive_entry(
                                &entry,
                                profile,
                                &entry.rule,
                            ));
                            entries.push(entry);
                        }
                        containers.push(scan.container);
                    }
                }
                Some((_, ProfilePathClass::NotMatched)) | None => {}
            }
            report_new_discovered_systems(
                &discoveries[discoveries_before..],
                &mut discovered_systems,
                &mut scan_events,
            );
            report_new_scanning_systems(
                &discoveries[discoveries_before..],
                &mut scanning_systems,
                &mut scan_events,
            );
            if discoveries.len() > discoveries_before && !first_discovery_reported {
                first_discovery_reported = true;
                library_db::report_library_scan_timing(
                    "first_discovery",
                    classify_t.elapsed().as_micros() as u64,
                    format!(
                        "candidate={} discoveries={} path={}",
                        idx,
                        discoveries.len(),
                        f.path.display()
                    ),
                );
            }
            if idx.is_multiple_of(SCAN_PROGRESS_CANDIDATE_BATCH) {
                report_catalog_progress(
                    &mut progress,
                    CatalogProgress::classifying_games_found(discoveries.len()),
                );
            }
        }
        handoff_attribution.consumer_active_us = handoff_attribution
            .consumer_active_us
            .saturating_add(consumer_active_started.elapsed().as_micros() as u64);
        if done {
            break;
        }
    }
    let handoff_loop_us = handoff_loop_started.elapsed().as_micros() as u64;
    crate::catalog_logln!(
        "catalog_scan_handoff_tsv\t{}",
        handoff_attribution.compact_detail(handoff_loop_us)
    );
    for (system, (last_target_ordinal, ready_us, targets)) in system_finality {
        crate::catalog_logln!(
            "catalog_system_finality_tsv\tsystem={}\tlast_target_ordinal={}\tready_us={}\ttargets={}\tsemantics=observed-last-contributor",
            system,
            last_target_ordinal,
            ready_us,
            targets,
        );
    }
    crate::catalog_logln!(
        "catalog_contributor_closure_tsv\t{}",
        contributor_closure.compact_detail()
    );
    let execution_pipeline_us = pipeline_started.elapsed().as_micros() as u64;
    let post_pipeline_started = Instant::now();
    if let Some(state) = resume.as_mut() {
        flush_target_checkpoints(state);
        checkpoint_attribution = state.checkpoint.clone();
    }
    crate::catalog_logln!(
        "catalog_target_checkpoint_io_tsv\t{}",
        checkpoint_attribution.compact_detail()
    );
    debug_assert!(target_descriptor.is_none());
    if discover_us == 0 {
        discover_us = discover_t.elapsed().as_micros() as u64;
    }
    library_db::report_library_scan_timing("walk", discover_us, format!("candidates={idx}"));
    library_db::report_library_scan_timing(
        "profile_match",
        timing.profile_match_us,
        format!("calls={}", timing.profile_match_count),
    );
    library_db::report_library_scan_timing(
        "installed_collection",
        timing.installed_collection_us,
        format!("calls={}", timing.installed_collection_count),
    );
    library_db::report_library_scan_timing(
        "archive_toc",
        timing.archive_toc_us,
        format!("containers={}", timing.archive_toc_count),
    );
    library_db::report_library_scan_timing(
        "collection_listings",
        timing.collection_listing_us,
        format!("collections={}", timing.collection_listing_count),
    );
    library_db::report_library_scan_timing(
        "file_discovery",
        timing.file_discovery_us,
        format!("files={}", timing.file_discovery_count),
    );
    timing.report_file_discovery_breakdown();
    library_db::report_library_scan_timing(
        "arcade_rom_filter",
        0,
        format!(
            "eligible={} missing={} ambiguous={} malformed={}",
            arcade_mra_eligible, arcade_mra_missing_rom, arcade_mra_ambiguous, arcade_mra_malformed,
        ),
    );
    let prepared_lookup = prepared_payload_index.lookup_stats();
    library_db::report_library_scan_timing(
        "prepared_payload_lookup",
        0,
        format!(
            "files={} missing={} unknown={} live_fallbacks={}",
            prepared_lookup.files,
            prepared_lookup.missing,
            prepared_lookup.unknown,
            prepared_lookup.live_fallbacks,
        ),
    );
    library_db::report_library_scan_timing(
        "classify_total",
        classify_t.elapsed().as_micros() as u64,
        format!(
            "discoveries={} normal_files={} containers={} entries={}",
            discoveries.len(),
            normal_files.len(),
            containers.len(),
            entries.len()
        ),
    );
    let profiles_t = Instant::now();
    profiles = plan.finalize_profiles(&game_dir_facts);
    library_db::report_library_scan_timing(
        "active_profiles",
        profiles_t.elapsed().as_micros() as u64,
        format!(
            "profiles={} runtime_facts={}",
            profiles.len(),
            game_dir_facts.len()
        ),
    );
    let audit_rows = match audit_mode {
        CoverageAuditMode::Inline => {
            let audit_t = Instant::now();
            let audit_rows = core_audit::audit_catalog_coverage_from_facts(
                &cfg.roots,
                &profiles,
                plan.installed_cores(),
                &game_dir_facts,
            );
            library_db::report_library_scan_timing(
                "coverage_audit",
                audit_t.elapsed().as_micros() as u64,
                format!("rows={}", audit_rows.len()),
            );
            audit_rows
        }
        CoverageAuditMode::Deferred => Vec::new(),
    };
    let post_pipeline_us = post_pipeline_started.elapsed().as_micros() as u64;
    resume_attribution.output_decode_us = resumed_output_decode_us;
    resume_attribution.output_decode_bytes = resumed_output_decode_bytes;
    resume_attribution.output_decode_targets = resumed_output_decode_targets;
    let total_us = discover_t.elapsed().as_micros() as u64;
    let accounted_us = plan_us
        .saturating_add(resume_us)
        .saturating_add(prepared_payload_us)
        .saturating_add(execution_pipeline_us)
        .saturating_add(post_pipeline_us);
    let attribution = CatalogScanAttribution {
        plan_us,
        resume_us,
        prepared_payload_us,
        execution_pipeline_us,
        post_pipeline_us,
        accounted_us,
        unattributed_us: total_us.saturating_sub(accounted_us),
        total_us,
        resume: resume_attribution,
        execution: execution_attribution,
    };
    library_db::report_library_scan_timing(
        "scan_attribution",
        total_us,
        attribution.compact_detail(),
    );
    crate::catalog_logln!(
        "catalog_scan_attribution_tsv\t{}",
        attribution.compact_detail()
    );
    LibraryScan {
        version: SCHEMA_VERSION,
        scanned_at_unix: library_db::unix_now_secs(),
        roots: cfg.roots.clone(),
        installed_cores: plan.installed_cores().to_vec(),
        game_dir_facts,
        profiles,
        normal_files,
        containers,
        entries,
        audit_rows,
        ignored_files,
        discoveries,
        discover_us,
        classify_us: classify_t.elapsed().as_micros() as u64,
        attribution,
    }
}

fn sanitize_arcade_index_metric(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_whitespace() {
                '_'
            } else {
                character
            }
        })
        .collect()
}

fn report_scan_target(
    scan_events: &mut ScanEventCallback<'_>,
    descriptor: &crate::catalog_scan::ScanTargetDescriptor,
    total: usize,
    priority: LibraryScanPriority,
    state: &str,
    completed_targets: usize,
    discoveries: usize,
) {
    let Some(report) = scan_events.as_mut() else {
        return;
    };
    report(LibraryScanEvent::TargetProgress {
        ordinal: descriptor.ordinal,
        total,
        path: descriptor.path.display().to_string(),
        target_kind: format!("{:?}", descriptor.kind).to_ascii_lowercase(),
        state: state.to_string(),
        completed_targets,
        discoveries,
        execution_mode: match priority {
            LibraryScanPriority::Background => "background_interactive",
            LibraryScanPriority::Foreground => "foreground_exclusive",
        }
        .to_string(),
        cooperative_policy: match priority {
            LibraryScanPriority::Background => "continuous_cpu0",
            LibraryScanPriority::Foreground => "unrestricted",
        }
        .to_string(),
    });
}

fn report_new_discovered_systems(
    discoveries: &[GameDiscovery],
    discovered_systems: &mut BTreeSet<String>,
    scan_events: &mut ScanEventCallback<'_>,
) {
    let Some(report) = scan_events.as_mut() else {
        return;
    };
    for discovery in discoveries {
        let system_id = catalog_system_id_for_discovery(discovery);
        if !is_reportable_catalog_system_id(&system_id) {
            continue;
        }
        if discovered_systems.insert(system_id.clone()) {
            report(LibraryScanEvent::SystemDiscovered { system_id });
        }
    }
}

fn report_new_scanning_systems(
    discoveries: &[GameDiscovery],
    scanning_systems: &mut BTreeSet<String>,
    scan_events: &mut ScanEventCallback<'_>,
) {
    let Some(report) = scan_events.as_mut() else {
        return;
    };
    for discovery in discoveries {
        let system_id = catalog_system_id_for_discovery(discovery);
        if is_reportable_catalog_system_id(&system_id) && scanning_systems.insert(system_id.clone())
        {
            report(LibraryScanEvent::SystemScanning { system_id });
        }
    }
}

fn report_resumed_systems(
    discoveries: &[GameDiscovery],
    discovered_systems: &mut BTreeSet<String>,
    scanning_systems: &mut BTreeSet<String>,
    scan_events: &mut ScanEventCallback<'_>,
) {
    report_new_discovered_systems(discoveries, discovered_systems, scan_events);
    report_new_scanning_systems(discoveries, scanning_systems, scan_events);
}

fn is_reportable_catalog_system_id(system_id: &str) -> bool {
    system_id != "unknown"
}

fn bootstrap_library_progress(
    cfg: &BenchConfig,
    mut progress: ProgressCallback<'_>,
) -> LibraryBootstrapSummary {
    let started = Instant::now();
    let mut launchers = 0usize;
    for target in bootstrap_launcher_targets(&cfg.roots) {
        scan_bootstrap_launcher_target(&target, &mut launchers, &mut progress);
    }
    LibraryBootstrapSummary {
        launchers,
        scan_us: started.elapsed().as_micros() as u64,
    }
}

#[cfg(test)]
mod incremental_planning_tests {
    use super::*;

    #[test]
    fn completed_target_dependencies_preserve_exact_system_ids() {
        let output = TargetOutput {
            game_dir_facts: Vec::new(),
            normal_files: Vec::new(),
            containers: Vec::new(),
            entries: Vec::new(),
            ignored_files: 0,
            discoveries: vec![crate::test_support::mra_discovery(1, "Robotron")],
        };

        assert_eq!(
            target_output_systems(&serde_json::to_string(&output).unwrap()),
            BTreeSet::from(["arcade".to_string()])
        );
    }

    #[test]
    fn corrupt_target_dependencies_force_no_false_exact_match() {
        assert!(target_output_systems("{not-json").is_empty());
    }

    #[test]
    fn oneload_helper_output_excludes_personal_c64_and_keeps_bundle_diagnostics() {
        let mut bundled = crate::test_support::payload(
            "/media/fat/games/C64/OneLoad64-Games-Collection-v5/Games/Bundled.crt",
        );
        bundled.prepared = Some(
            crate::prepared_collections::PreparedLaunchProvenance::prepared(
                crate::prepared_collections::PreparedCollectionId::OneLoad64,
            ),
        );
        let diagnostic = crate::test_support::payload(
            "/media/fat/games/C64/OneLoad64-Games-Collection-v5/Dumps/Diagnostic.crt",
        );
        let personal = crate::test_support::payload("/media/fat/games/C64/Personal/Homebrew.crt");
        let discoveries = vec![bundled, diagnostic, personal];
        let output = BorrowedTargetOutput {
            game_dir_facts: &[],
            normal_files: &[],
            containers: &[],
            entries: &[],
            ignored_files: 0,
            discoveries: &discoveries,
        };

        let (root, filtered) = oneload64_target_output(&output).unwrap().unwrap();

        assert_eq!(
            root,
            Path::new("/media/fat/games/C64/OneLoad64-Games-Collection-v5")
        );
        assert_eq!(filtered.discoveries.len(), 2);
        assert!(
            filtered
                .discoveries
                .iter()
                .all(|discovery| discovery.source_path.starts_with(root.to_str().unwrap()))
        );
    }

    #[test]
    fn contributor_closure_waits_for_unknown_targets() {
        let mut closure = ContributorClosure::new([
            (0, Some("arcade".to_string())),
            (1, None),
            (2, Some("snes".to_string())),
        ]);

        assert!(
            closure
                .complete(0, &BTreeSet::from(["arcade".to_string()]))
                .is_empty()
        );
        assert_eq!(
            closure.complete(1, &BTreeSet::new()),
            vec!["arcade".to_string()]
        );
        assert_eq!(
            closure.complete(2, &BTreeSet::from(["snes".to_string()])),
            vec!["snes".to_string()]
        );
    }

    #[test]
    fn contributor_closure_fails_closed_on_unexpected_system() {
        let mut closure = ContributorClosure::new([(0, Some("snes".to_string()))]);

        assert!(
            closure
                .complete(0, &BTreeSet::from(["nes".to_string()]))
                .is_empty()
        );
        assert!(closure.compact_detail().contains("sound=0"));
    }
}

fn bootstrap_launcher_targets(roots: &[String]) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    for root in roots {
        let path = Path::new(root);
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("_Arcade"))
        {
            targets.push(path.to_path_buf());
        } else {
            targets.push(path.join("_Arcade"));
        }
    }
    targets
}

fn scan_bootstrap_launcher_target(
    target: &Path,
    launchers: &mut usize,
    progress: &mut ProgressCallback<'_>,
) {
    let Ok(entries) = std::fs::read_dir(target) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !is_bootstrap_launcher_path(&path) {
            continue;
        }
        *launchers += 1;
        if launchers.is_multiple_of(BOOTSTRAP_PROGRESS_BATCH) {
            report_catalog_progress(progress, CatalogProgress::finding_games_found(*launchers));
        }
    }
}

fn is_bootstrap_launcher_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name.len() > 1 && name.starts_with('.') {
        return false;
    }
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("mra" | "mgl")
    )
}

#[cfg(test)]
mod timing_tests {
    use super::*;

    fn updater_index(
        row: crate::arcade_updater_index::ArcadeUpdaterRow,
    ) -> crate::arcade_updater_index::ArcadeUpdaterIndex {
        crate::arcade_updater_index::ArcadeUpdaterIndex {
            sources: [
                "alternatives",
                "arcade-offset",
                "coinop",
                "distribution",
                "jtcores",
            ]
            .into_iter()
            .map(|id| crate::arcade_updater_index::ArcadeUpdaterSource {
                id: id.to_string(),
                revision: "a".repeat(40),
                database_sha256: "b".repeat(64),
            })
            .collect(),
            rows: vec![row],
        }
    }

    #[test]
    fn scan_handoff_accounting_is_bounded_and_reports_batch_shape() {
        let attribution = ScanHandoffAttribution {
            receive_wait_us: 30,
            consumer_active_us: 55,
            events: 4,
            file_events: 2,
            facts_events: 0,
            runtime_events: 1,
            target_events: 1,
            files: 65,
            max_batch: 64,
        };

        let detail = attribution.compact_detail(80);

        assert!(detail.contains("receive_wait_us=30"));
        assert!(detail.contains("consumer_active_us=55"));
        assert!(detail.contains("unattributed_us=0"));
        assert!(detail.contains("files=65"));
        assert!(detail.contains("max_batch=64"));
    }

    #[test]
    fn catalog_progress_reports_valid_systems_but_not_the_unknown_sentinel() {
        assert!(is_reportable_catalog_system_id("gba"));
        assert!(!is_reportable_catalog_system_id("unknown"));
    }

    #[test]
    fn resumed_discoveries_republish_scanning_presentation() {
        let discoveries = vec![crate::test_support::mra_discovery(1, "Robotron")];
        let mut discovered_systems = BTreeSet::new();
        let mut scanning_systems = BTreeSet::new();
        let mut events = Vec::new();
        let mut report = |event| events.push(event);
        let mut scan_events: ScanEventCallback<'_> = Some(&mut report);

        report_resumed_systems(
            &discoveries,
            &mut discovered_systems,
            &mut scanning_systems,
            &mut scan_events,
        );

        assert!(matches!(
            events.as_slice(),
            [
                LibraryScanEvent::SystemDiscovered { system_id: discovered },
                LibraryScanEvent::SystemScanning { system_id: scanning },
            ] if discovered == "arcade" && scanning == "arcade"
        ));
    }

    #[test]
    fn file_discovery_source_classes_separate_metadata_reads_from_paths() {
        assert_eq!(file_discovery_source_class("mra"), "mra-metadata");
        assert_eq!(file_discovery_source_class("MGL"), "mgl-metadata");
        assert_eq!(file_discovery_source_class("crt"), "path-derived");
    }

    #[test]
    fn parallel_mra_reads_are_limited_to_non_resumable_arcade_bootstrap() {
        assert!(is_arcade_bootstrap_scan(
            &[crate::arcade_catalog::DEFAULT_ARCADE_ROOT.to_string()],
            false,
        ));
        assert!(!is_arcade_bootstrap_scan(
            &[crate::arcade_catalog::DEFAULT_ARCADE_ROOT.to_string()],
            true,
        ));
        assert!(!is_arcade_bootstrap_scan(
            &["/media/fat/games/SNES".to_string()],
            false,
        ));
    }

    #[test]
    fn updater_index_supplies_matching_metadata_and_size_changes_fall_back() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-arcade-updater-prefetch-{}",
            std::process::id()
        ));
        let arcade = root.join("_Arcade");
        std::fs::create_dir_all(&arcade).unwrap();
        let mra = arcade.join("Fixture.mra");
        let local = b"<misterromdescription><name>Local</name></misterromdescription>";
        std::fs::write(&mra, local).unwrap();
        let index_path = root.join("arcade-updater-index-v1.lz4b");
        updater_index(crate::arcade_updater_index::ArcadeUpdaterRow {
            path: "_Arcade/Fixture.mra".to_string(),
            source_id: "distribution".to_string(),
            size: local.len() as u64,
            md5: "c".repeat(32),
            header: media_metadata::MraMetadata {
                name: Some("Indexed".to_string()),
                ..media_metadata::MraMetadata::default()
            },
            primary_rom: media_metadata::PrimaryRomRequirement::None,
            catalog_metadata: Some(crate::arcade_updater_index::ArcadeUpdaterCatalogMetadata {
                identity_id: "fixture".to_string(),
                family_id: "fixture-parent".to_string(),
                title: "Indexed title".to_string(),
                category: "Platform".to_string(),
                ..crate::arcade_updater_index::ArcadeUpdaterCatalogMetadata::default()
            }),
        })
        .write(&index_path)
        .unwrap();
        let event = || {
            vec![DiscoveryEvent::File(crate::catalog_scan::FoundFile {
                path: mra.clone(),
                ext: "mra".to_string(),
                size: 0,
                mtime_secs: 0,
            })]
        };

        let indexed = prefetch_arcade_mra_metadata(&event(), Some(&index_path));
        assert_eq!(indexed.index_status, "loaded");
        assert_eq!(
            indexed.index_path.as_deref(),
            Some(index_path.to_str().unwrap())
        );
        assert_eq!(indexed.index_rows, 1);
        assert_eq!(indexed.index_file_sha256.as_deref().map(str::len), Some(64));
        assert!(indexed.index_error.is_none());
        assert_eq!(indexed.index_hits, 1);
        assert_eq!(indexed.fallback_reads, 0);
        assert_eq!(indexed.identity_stats, 1);
        assert_eq!(indexed.identity_stat_failures, 0);
        assert_eq!(
            indexed.inspections[&mra]
                .as_ref()
                .unwrap()
                .header
                .name
                .as_deref(),
            Some("Indexed")
        );
        assert_eq!(
            indexed.inspections[&mra]
                .as_ref()
                .unwrap()
                .catalog_metadata
                .as_ref()
                .map(|metadata| metadata.family_id.as_str()),
            Some("fixture-parent")
        );

        std::fs::write(&mra, [local.as_slice(), b" "].concat()).unwrap();
        let fallback = prefetch_arcade_mra_metadata(&event(), Some(&index_path));
        assert_eq!(fallback.index_hits, 0);
        assert_eq!(fallback.fallback_reads, 1);
        assert_eq!(fallback.identity_stats, 1);
        assert_eq!(
            fallback.inspections[&mra]
                .as_ref()
                .unwrap()
                .header
                .name
                .as_deref(),
            Some("Local")
        );
        assert!(
            fallback.inspections[&mra]
                .as_ref()
                .unwrap()
                .catalog_metadata
                .is_none()
        );

        let missing = prefetch_arcade_mra_metadata(&event(), Some(&root.join("missing.lz4b")));
        assert_eq!(missing.index_status, "missing");
        assert!(missing.index_error.is_some());
        assert_eq!(missing.fallback_reads, 1);

        let invalid_path = root.join("invalid.lz4b");
        std::fs::write(&invalid_path, b"invalid").unwrap();
        let invalid = prefetch_arcade_mra_metadata(&event(), Some(&invalid_path));
        assert_eq!(invalid.index_status, "invalid");
        assert!(invalid.index_error.is_some());
        assert_eq!(invalid.fallback_reads, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_discovery_timing_aggregates_by_profile_and_extension() {
        let mut timing = ScanTimingStats::default();
        timing.record_file_discovery("arcade", "mra", 12);
        timing.record_file_discovery("arcade", "mra", 30);
        timing.record_file_discovery("c64", "crt", 5);

        assert_eq!(timing.file_discovery_us, 47);
        assert_eq!(timing.file_discovery_count, 3);
        let arcade = &timing.file_discovery_breakdown["arcade"]["mra"];
        assert_eq!(arcade.elapsed_us, 42);
        assert_eq!(arcade.calls, 2);
        assert_eq!(arcade.max_us, 30);
        assert_eq!(timing.file_discovery_breakdown["c64"]["crt"].calls, 1);
    }

    fn assert_target_restart_rolls_back(stage: &str) {
        let offsets = TargetOffsets {
            facts: 1,
            files: 1,
            containers: 1,
            entries: 1,
            ignored: 1,
            discoveries: 1,
            candidates: 1,
            arcade_mra_eligible: 1,
            arcade_mra_missing_rom: 1,
            arcade_mra_ambiguous: 1,
            arcade_mra_malformed: 1,
            first_discovery_reported: false,
        };
        let mut facts = vec![1];
        let mut files = vec![1];
        let mut containers = vec![1];
        let mut entries = vec![1];
        let mut discoveries = vec![1];
        match stage {
            "before-first-entry" => {}
            "mid-directory" => {
                facts.push(2);
                files.push(2);
                discoveries.push(2);
            }
            "after-archive" => {
                containers.push(2);
                entries.push(2);
                discoveries.push(2);
            }
            "before-completion" => {
                facts.push(2);
                files.push(2);
                containers.push(2);
                entries.push(2);
                discoveries.push(2);
            }
            _ => unreachable!("unknown restart fixture"),
        }
        let mut ignored = 9;
        let mut candidates = 9;
        let mut arcade_mra_eligible = 9;
        let mut arcade_mra_missing_rom = 9;
        let mut arcade_mra_ambiguous = 9;
        let mut arcade_mra_malformed = 9;
        let mut first_discovery_reported = true;

        offsets.rollback(
            &mut facts,
            &mut files,
            &mut containers,
            &mut entries,
            &mut ignored,
            &mut discoveries,
            &mut candidates,
            &mut arcade_mra_eligible,
            &mut arcade_mra_missing_rom,
            &mut arcade_mra_ambiguous,
            &mut arcade_mra_malformed,
            &mut first_discovery_reported,
        );

        assert_eq!(facts, vec![1]);
        assert_eq!(files, vec![1]);
        assert_eq!(containers, vec![1]);
        assert_eq!(entries, vec![1]);
        assert_eq!(discoveries, vec![1]);
        assert_eq!(ignored, 1);
        assert_eq!(candidates, 1);
        assert_eq!(arcade_mra_eligible, 1);
        assert_eq!(arcade_mra_missing_rom, 1);
        assert_eq!(arcade_mra_ambiguous, 1);
        assert_eq!(arcade_mra_malformed, 1);
        assert!(!first_discovery_reported);
    }

    #[test]
    fn target_restart_before_first_entry_preserves_prior_output() {
        assert_target_restart_rolls_back("before-first-entry");
    }

    #[test]
    fn target_restart_mid_directory_discards_partial_output() {
        assert_target_restart_rolls_back("mid-directory");
    }

    #[test]
    fn target_restart_after_archive_discards_container_output() {
        assert_target_restart_rolls_back("after-archive");
    }

    #[test]
    fn target_restart_before_completion_resets_every_counter() {
        assert_target_restart_rolls_back("before-completion");
    }

    #[test]
    fn injected_restart_produces_byte_identical_target_output() {
        let offsets = TargetOffsets {
            facts: 1,
            files: 1,
            containers: 1,
            entries: 1,
            ignored: 1,
            discoveries: 1,
            candidates: 1,
            arcade_mra_eligible: 1,
            arcade_mra_missing_rom: 0,
            arcade_mra_ambiguous: 0,
            arcade_mra_malformed: 0,
            first_discovery_reported: true,
        };
        let baseline = serde_json::to_vec(&(
            vec![1, 2],
            vec![1, 2],
            vec![1, 2],
            vec![1, 2],
            2usize,
            vec![1, 2],
        ))
        .unwrap();
        let mut facts = vec![1, 99];
        let mut files = vec![1, 99];
        let mut containers = vec![1, 99];
        let mut entries = vec![1, 99];
        let mut ignored = 9;
        let mut discoveries = vec![1, 99];
        let mut candidates = 9;
        let mut eligible = 9;
        let mut missing = 9;
        let mut ambiguous = 9;
        let mut malformed = 9;
        let mut first_reported = false;
        offsets.rollback(
            &mut facts,
            &mut files,
            &mut containers,
            &mut entries,
            &mut ignored,
            &mut discoveries,
            &mut candidates,
            &mut eligible,
            &mut missing,
            &mut ambiguous,
            &mut malformed,
            &mut first_reported,
        );
        facts.push(2);
        files.push(2);
        containers.push(2);
        entries.push(2);
        ignored += 1;
        discoveries.push(2);
        let restarted =
            serde_json::to_vec(&(facts, files, containers, entries, ignored, discoveries)).unwrap();

        assert_eq!(restarted, baseline);
    }

    #[test]
    fn target_restart_cannot_close_contributors_or_publish_checkpoints() {
        let source = include_str!("library_indexer.rs")
            .split_whitespace()
            .collect::<String>();
        let restart = source
            .find("DiscoveryEvent::TargetRestart(restart)=>{")
            .expect("restart event arm");
        let complete = source[restart..]
            .find("DiscoveryEvent::TargetComplete(descriptor)=>{")
            .map(|offset| restart + offset)
            .expect("completion event arm");
        let restart_arm = &source[restart..complete];
        let complete_arm = &source[complete..];

        assert!(!restart_arm.contains("contributor_closure.complete"));
        assert!(!restart_arm.contains("queue_target_checkpoint"));
        assert!(!restart_arm.contains("system_finality.entry"));
        assert!(complete_arm.contains("contributor_closure.complete"));
        assert!(complete_arm.contains("queue_target_checkpoint"));
        assert!(complete_arm.contains("system_finality.entry"));
    }

    #[test]
    fn scan_attribution_reports_non_overlapping_accounting_and_resume_results() {
        let attribution = CatalogScanAttribution {
            plan_us: 10,
            resume_us: 20,
            prepared_payload_us: 30,
            execution_pipeline_us: 40,
            post_pipeline_us: 50,
            accounted_us: 150,
            unattributed_us: 5,
            total_us: 155,
            resume: ResumeValidationAttribution {
                enabled: true,
                committed_state_present: true,
                committed_state_seeded: true,
                open_us: 20,
                validation_us: 12,
                committed_targets: 4,
                validated_targets: 3,
                reused_targets: 2,
                invalidated_targets: 2,
                unavailable_targets: 1,
                error_targets: 1,
                setup_errors: 0,
                namespace: catalog_scan::NamespaceRouteAttribution::default(),
                ..ResumeValidationAttribution::default()
            },
            execution: catalog_scan::NamespaceRouteAttribution::default(),
        };

        let detail = attribution.compact_detail();
        assert_eq!(
            attribution.accounted_us + attribution.unattributed_us,
            attribution.total_us
        );
        assert!(detail.contains("scan_unattributed_us=5"));
        assert!(detail.contains("resume_committed=4"));
        assert!(detail.contains("resume_reused=2"));
        assert!(detail.contains("resume_unavailable=1"));
        assert!(detail.contains("validation_targets=0"));
        assert!(detail.contains("execution_targets=0"));
    }
}
