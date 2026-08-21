// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Library filesystem indexing and classification.
//!
//! This module owns the full-scan product contract: walk configured roots,
//! classify launcher/payload/archive/listing candidates, emit progress/events,
//! and return a complete `LibraryScan`.

use crate::catalog_config::SCHEMA_VERSION;
use crate::catalog_progress::{CatalogProgress, report_catalog_progress};
use crate::catalog_scan::{self, DiscoveryEvent};
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
        }
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
    pub(crate) validation_us: u64,
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
            "scan_total_us={} scan_accounted_us={} scan_unattributed_us={} scan_plan_us={} scan_resume_us={} scan_prepared_payload_us={} scan_execution_pipeline_us={} scan_post_pipeline_us={} resume_enabled={} resume_state_present={} resume_state_seeded={} resume_open_us={} resume_validation_us={} resume_committed={} resume_validated={} resume_reused={} resume_invalidated={} resume_unavailable={} resume_errors={} resume_setup_errors={} {} {}",
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
            self.resume.validation_us,
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

struct Fingerprint(u64);

impl Fingerprint {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }

    fn for_descriptor(descriptor: &catalog_scan::ScanTargetDescriptor) -> Self {
        let mut value = Self::new();
        value.bytes(descriptor.path.to_string_lossy().as_bytes());
        value.bytes(format!("{:?}", descriptor.kind).as_bytes());
        value
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(0x100000001b3);
    }

    fn file(&mut self, file: &catalog_scan::FoundFile) {
        self.bytes(file.path.to_string_lossy().as_bytes());
        self.bytes(file.ext.as_bytes());
        self.bytes(&file.size.to_le_bytes());
        self.bytes(&file.mtime_secs.to_le_bytes());
    }

    fn facts(&mut self, facts: &crate::catalog_discovery::GameDirFact) {
        if let Ok(encoded) = serde_json::to_vec(facts) {
            self.bytes(&encoded);
        }
    }

    fn finish(&self) -> String {
        format!("{:016x}", self.0)
    }
}

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
    let decode_started = Instant::now();
    let decoded = journal.completed_targets();
    let decode_us = decode_started.elapsed().as_micros() as u64;
    let completed: HashMap<_, _> = decoded
        .unwrap_or_default()
        .into_iter()
        .map(|target| (target.target.ordinal, target))
        .collect();
    attribution.committed_targets = completed.len();
    let validation_started = Instant::now();
    let (fingerprints, validation_namespace) = if completed.is_empty() {
        (
            HashMap::new(),
            catalog_scan::NamespaceRouteAttribution::default(),
        )
    } else {
        validate_target_fingerprints(cfg, plan, excluded_targets, priority, &completed)
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
    attribution.open_us = open_started.elapsed().as_micros() as u64;
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

fn validate_target_fingerprints(
    cfg: &BenchConfig,
    plan: &launch_profiles::CatalogScanPlan,
    excluded_targets: &[PathBuf],
    priority: LibraryScanPriority,
    completed: &HashMap<u32, crate::build_progress::CompletedTarget>,
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
    let rx = catalog_scan::discover_files_pipelined_for_resume_validation(
        cfg.roots.clone(),
        plan.clone(),
        validation_exclusions,
        role,
    );
    let mut current: Option<(u32, Fingerprint)> = None;
    let mut fingerprints = HashMap::new();
    let mut attribution = catalog_scan::NamespaceRouteAttribution::default();
    while let Ok(event) = rx.recv() {
        match event {
            DiscoveryEvent::TargetStart(descriptor) => {
                let original = completed.values().find(|saved| {
                    saved.target.path == descriptor.path.to_string_lossy()
                        && saved
                            .target
                            .key
                            .starts_with(&format!("{:?}:", descriptor.kind))
                });
                current = original.map(|saved| {
                    (
                        saved.target.ordinal,
                        Fingerprint::for_descriptor(&descriptor),
                    )
                });
            }
            DiscoveryEvent::File(file) => {
                if let Some((_, fingerprint)) = current.as_mut() {
                    fingerprint.file(&file);
                }
            }
            DiscoveryEvent::GameDirFacts(facts) => {
                if let Some((_, fingerprint)) = current.as_mut() {
                    fingerprint.facts(&facts);
                }
            }
            DiscoveryEvent::RuntimeDirectory(runtime) => {
                if let Some((_, fingerprint)) = current.as_mut() {
                    fingerprint.facts(&runtime.facts);
                    for file in &runtime.files {
                        fingerprint.file(file);
                    }
                }
            }
            DiscoveryEvent::TargetComplete(_) => {
                if let Some((ordinal, fingerprint)) = current.take() {
                    fingerprints.insert(ordinal, fingerprint.finish());
                }
            }
            DiscoveryEvent::Done {
                attribution: route_attribution,
                ..
            } => {
                attribution = route_attribution;
                break;
            }
        }
    }
    (fingerprints, attribution)
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
}

fn is_arcade_bootstrap_scan(roots: &[String], durable_resume: bool) -> bool {
    !durable_resume
        && roots.len() == 1
        && roots[0].eq_ignore_ascii_case(crate::arcade_catalog::DEFAULT_ARCADE_ROOT)
}

struct ArcadeMraPrefetch {
    inspections: HashMap<PathBuf, Option<media_metadata::MraInspection>>,
    index_status: &'static str,
    index_hits: usize,
    index_misses: usize,
    fallback_reads: usize,
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
            index_hits: 0,
            index_misses: 0,
            fallback_reads: 0,
            index_load_us: 0,
        };
    }

    let load_started = Instant::now();
    let loaded_index = updater_index_path
        .and_then(|path| crate::arcade_updater_index::ArcadeUpdaterIndex::read(path).ok());
    let index_load_us = load_started.elapsed().as_micros() as u64;
    let index_status = if loaded_index.is_some() {
        "loaded"
    } else if updater_index_path.is_some() {
        "invalid-or-missing"
    } else {
        "not-configured"
    };
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
    for (path, size) in paths {
        let indexed = arcade_updater_key(&path)
            .and_then(|key| indexed_rows.get(&key))
            .filter(|row| {
                size == row.size
                    && row.primary_rom != media_metadata::PrimaryRomRequirement::Ambiguous
            });
        if let Some(row) = indexed {
            inspections.insert(
                path,
                Some(media_metadata::MraInspection {
                    header: row.header.clone(),
                    primary_rom: row.primary_rom.clone(),
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
        index_hits,
        index_misses: fallback_paths.len(),
        fallback_reads: fallback_paths.len(),
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
    let (mut resume, resume_attribution) =
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
            "files={} complete_roots={}",
            prepared_payload_index.file_count(),
            prepared_payload_index.complete_root_count(),
        ),
    );
    let prevalidated_targets = resume
        .as_ref()
        .map(|state| {
            state
                .reusable
                .values()
                .map(|saved| PathBuf::from(&saved.target.path))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let pipeline_started = Instant::now();
    let rx = match priority {
        LibraryScanPriority::Background => catalog_scan::discover_files_pipelined_with_plan(
            cfg.roots.clone(),
            plan.clone(),
            excluded_targets,
            prevalidated_targets,
            crate::runtime_thread::RuntimeThreadRole::LibraryWalker,
        ),
        LibraryScanPriority::Foreground => {
            catalog_scan::discover_files_pipelined_foreground_with_plan(
                cfg.roots.clone(),
                plan.clone(),
                excluded_targets,
                prevalidated_targets,
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
                "files={} successes={} failures={} workers={} index_status={} index_hits={} index_misses={} fallback_reads={} index_load_us={}",
                prefetched_arcade_mra.len(),
                successes,
                prefetched_arcade_mra.len().saturating_sub(successes),
                ARCADE_MRA_READ_WORKERS.min(prefetch.fallback_reads),
                prefetch.index_status,
                prefetch.index_hits,
                prefetch.index_misses,
                prefetch.fallback_reads,
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
    let mut target_descriptor = None;
    let mut target_offsets = None;
    let mut target_fingerprint = Fingerprint::new();
    let mut skip_target = false;
    let mut target_checkpointable = true;
    let mut checkpoint_attribution = CheckpointAttribution {
        enabled: durable_resume,
        ..CheckpointAttribution::default()
    };
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
            break;
        };
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
                target_offsets = Some(TargetOffsets::capture(
                    &game_dir_facts,
                    &normal_files,
                    &containers,
                    &entries,
                    ignored_files,
                    &discoveries,
                ));
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
                if let Some(saved) = resume
                    .as_mut()
                    .and_then(|state| state.reusable.remove(&(descriptor.ordinal as u32)))
                {
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
                }
                target_descriptor = Some(descriptor);
                Vec::new()
            }
            DiscoveryEvent::TargetComplete(descriptor) => {
                handoff_attribution.target_events =
                    handoff_attribution.target_events.saturating_add(1);
                let ready_us = pipeline_started.elapsed().as_micros() as u64;
                let offsets = target_offsets.unwrap_or_else(|| {
                    TargetOffsets::capture(
                        &game_dir_facts,
                        &normal_files,
                        &containers,
                        &entries,
                        ignored_files,
                        &discoveries,
                    )
                });
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
                    "catalog_target_handoff_tsv\tordinal={}\tkind={:?}\tready_us={}\tdiscoveries={}\tsystems={}\tpath={}",
                    descriptor.ordinal,
                    descriptor.kind,
                    ready_us,
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
                    && let (Some(offsets), Some(state)) = (target_offsets, resume.as_mut())
                {
                    let snapshot_started = Instant::now();
                    let output = BorrowedTargetOutput {
                        game_dir_facts: &game_dir_facts[offsets.facts..],
                        normal_files: &normal_files[offsets.files..],
                        containers: &containers[offsets.containers..],
                        entries: &entries[offsets.entries..],
                        ignored_files: ignored_files.saturating_sub(offsets.ignored),
                        discoveries: &discoveries[offsets.discoveries..],
                    };
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
                                Some(Some(inspection.header))
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
    let _ = target_descriptor;
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
        })
        .write(&index_path)
        .unwrap();
        let event = || {
            vec![DiscoveryEvent::File(crate::catalog_scan::FoundFile {
                path: mra.clone(),
                ext: "mra".to_string(),
                size: std::fs::metadata(&mra).unwrap().len(),
                mtime_secs: 0,
            })]
        };

        let indexed = prefetch_arcade_mra_metadata(&event(), Some(&index_path));
        assert_eq!(indexed.index_hits, 1);
        assert_eq!(indexed.fallback_reads, 0);
        assert_eq!(
            indexed.inspections[&mra]
                .as_ref()
                .unwrap()
                .header
                .name
                .as_deref(),
            Some("Indexed")
        );

        std::fs::write(&mra, [local.as_slice(), b" "].concat()).unwrap();
        let fallback = prefetch_arcade_mra_metadata(&event(), Some(&index_path));
        assert_eq!(fallback.index_hits, 0);
        assert_eq!(fallback.fallback_reads, 1);
        assert_eq!(
            fallback.inspections[&mra]
                .as_ref()
                .unwrap()
                .header
                .name
                .as_deref(),
            Some("Local")
        );
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
