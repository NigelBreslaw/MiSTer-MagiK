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
    discovery_from_profile_file_with_prepared_index,
};
use crate::launch_profiles::{self, PayloadDisposition, PayloadRule, ProfilePathClass};
use crate::library_db::{
    self, ArchiveFormat, BenchConfig, LibraryBootstrapSummary, LibraryPayloadFile, LibraryScan,
    LibraryScanEvent, ProgressCallback, ScanEventCallback,
};
use crate::media_metadata;
use crate::prepared_collections::PreparedPayloadIndex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

const SCAN_PROGRESS_CANDIDATE_BATCH: usize = 50;
const BOOTSTRAP_PROGRESS_BATCH: usize = 50;
pub(crate) struct LibraryIndexer<'a> {
    cfg: &'a BenchConfig,
    archive_reader: crate::catalog_config::ArchiveReaderConfig,
    priority: LibraryScanPriority,
    durable_resume: bool,
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
        }
    }

    pub(crate) fn with_durable_resume(mut self, durable_resume: bool) -> Self {
        self.durable_resume = durable_resume;
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

#[derive(Serialize, Deserialize)]
struct TargetOutput {
    game_dir_facts: Vec<crate::catalog_discovery::GameDirFact>,
    normal_files: Vec<LibraryPayloadFile>,
    containers: Vec<crate::library_db::LibraryContainer>,
    entries: Vec<crate::library_db::LibraryContainerEntry>,
    ignored_files: usize,
    discoveries: Vec<GameDiscovery>,
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
        Ok(()) => {
            state.committed += count;
            report_resume(
                state,
                "targets-committed",
                ordinal,
                &format!("durable-batch:{count}"),
            );
        }
        Err(error) => report_resume(state, "checkpoint-failed", ordinal, &error),
    }
}

fn queue_target_checkpoint(
    state: &mut ResumeScan,
    completed: crate::build_progress::CompletedTarget,
) {
    state.pending_bytes = state
        .pending_bytes
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
) -> Option<ResumeScan> {
    if !durable_resume {
        return None;
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
    };
    let path = crate::catalog_config::default_build_progress_path();
    let committed_path = crate::catalog_config::default_builder_state_path();
    let had_committed_state = committed_path.exists();
    if let Err(error) = crate::build_progress::seed_from_committed(&committed_path, &path) {
        crate::catalog_logln!(
            "catalog_resume_tsv\tphase=committed-state-disabled\treason={}",
            error.replace(['\t', '\n'], " ")
        );
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
            return None;
        }
    };
    let completed: HashMap<_, _> = journal
        .completed_targets()
        .unwrap_or_default()
        .into_iter()
        .map(|target| (target.target.ordinal, target))
        .collect();
    let fingerprints = if completed.is_empty() {
        HashMap::new()
    } else {
        validate_target_fingerprints(cfg, plan, excluded_targets, priority, &completed)
    };
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
    };
    report_resume(&state, "journal-open", 0, &format!("{status:?}"));
    Some(state)
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
) -> HashMap<u32, String> {
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
    let rx = match priority {
        LibraryScanPriority::Background => catalog_scan::discover_files_pipelined_with_plan(
            cfg.roots.clone(),
            plan.clone(),
            validation_exclusions,
            crate::runtime_thread::RuntimeThreadRole::LibraryWalker,
        ),
        LibraryScanPriority::Foreground => {
            catalog_scan::discover_files_pipelined_foreground_with_plan(
                cfg.roots.clone(),
                plan.clone(),
                validation_exclusions,
            )
        }
    };
    let mut current: Option<(u32, Fingerprint)> = None;
    let mut fingerprints = HashMap::new();
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
            DiscoveryEvent::Done { .. } => break,
        }
    }
    fingerprints
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
    } = execution;
    crate::cooperative_work::checkpoint();
    let discover_t = Instant::now();
    let plan_t = Instant::now();
    let plan = launch_profiles::CatalogScanPlan::for_roots(&cfg.roots);
    apply_configured_target_allowlist(&cfg.roots, &plan, &mut excluded_targets);
    crate::cooperative_work::checkpoint();
    library_db::report_library_scan_timing(
        "catalog_scan_plan",
        plan_t.elapsed().as_micros() as u64,
        format!(
            "base_profiles={} installed_cores={} runtime_dirs={}",
            plan.base_profiles().len(),
            plan.installed_cores().len(),
            plan.game_dir_headers().len(),
        ),
    );
    let mut resume = prepare_resume_scan(cfg, &plan, &excluded_targets, priority, durable_resume);
    if let (Some(state), Some(report)) = (resume.as_ref(), scan_events.as_mut()) {
        report(LibraryScanEvent::ReconciliationPlanReady {
            system_ids: state.affected_systems.clone(),
            all_published_systems: state.all_published_systems,
        });
    }
    let target_count =
        catalog_scan::planned_scan_target_descriptors(&cfg.roots, &plan, &excluded_targets).len();
    let prepared_payload_t = Instant::now();
    let prepared_payload_index = PreparedPayloadIndex::from_library_roots(&cfg.roots);
    crate::cooperative_work::checkpoint();
    library_db::report_library_scan_timing(
        "prepared_payload_index",
        prepared_payload_t.elapsed().as_micros() as u64,
        format!(
            "files={} complete_roots={}",
            prepared_payload_index.file_count(),
            prepared_payload_index.complete_root_count(),
        ),
    );
    let rx = match priority {
        LibraryScanPriority::Background => catalog_scan::discover_files_pipelined_with_plan(
            cfg.roots.clone(),
            plan.clone(),
            excluded_targets,
            crate::runtime_thread::RuntimeThreadRole::LibraryWalker,
        ),
        LibraryScanPriority::Foreground => {
            catalog_scan::discover_files_pipelined_foreground_with_plan(
                cfg.roots.clone(),
                plan.clone(),
                excluded_targets,
            )
        }
    };
    let mut game_dir_facts = Vec::with_capacity(plan.game_dir_headers().len());
    let mut profiles = plan.base_profiles().to_vec();
    let mut discover_us = 0;

    let mut normal_files = Vec::new();
    let mut containers = Vec::new();
    let mut entries = Vec::new();
    let mut ignored_files = 0usize;
    let mut discoveries = Vec::new();
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
    let mut last_target_heartbeat = Instant::now();
    while let Ok(event) = rx.recv() {
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
        let files = match event {
            DiscoveryEvent::TargetStart(descriptor) => {
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
                if !skip_target
                    && target_checkpointable
                    && let (Some(offsets), Some(state)) = (target_offsets, resume.as_mut())
                {
                    let output = TargetOutput {
                        game_dir_facts: game_dir_facts[offsets.facts..].to_vec(),
                        normal_files: normal_files[offsets.files..].to_vec(),
                        containers: containers[offsets.containers..].to_vec(),
                        entries: entries[offsets.entries..].to_vec(),
                        ignored_files: ignored_files.saturating_sub(offsets.ignored),
                        discoveries: discoveries[offsets.discoveries..].to_vec(),
                    };
                    match serde_json::to_string(&output) {
                        Ok(output_json) => {
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
                        Err(error) => report_resume(
                            state,
                            "checkpoint-failed",
                            descriptor.ordinal,
                            &format!("encode-error:{error}"),
                        ),
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
                if skip_target {
                    Vec::new()
                } else {
                    target_fingerprint.file(&file);
                    vec![file]
                }
            }
            DiscoveryEvent::GameDirFacts(facts) => {
                if !skip_target {
                    target_fingerprint.facts(&facts);
                    game_dir_facts.push(facts);
                }
                Vec::new()
            }
            DiscoveryEvent::RuntimeDirectory(runtime) => {
                if skip_target {
                    continue;
                }
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
            DiscoveryEvent::Done {
                discover_us: us, ..
            } => {
                discover_us = us;
                break;
            }
        };
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
                    discoveries.push(discovery_from_profile_file_with_prepared_index(
                        &f,
                        profile,
                        &payload_rule,
                        &profiles,
                        Some(&prepared_payload_index),
                    ));
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
    }
    if let Some(state) = resume.as_mut() {
        flush_target_checkpoints(state);
    }
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
}
