// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::arcade_catalog::ArcadeCatalog;
use crate::builder_protocol::{
    BuilderSummary, CATALOG_BUILDER_PROTOCOL_VERSION, CatalogBuilderEvent, CatalogChangeReason,
    CatalogFailureCode, CatalogFailureDiagnostic, CatalogPlanReason, CatalogPlannedAction,
    CatalogPlannedSystem,
};
use crate::catalog_build_record;
use crate::catalog_navigation::write_catalog_navigation_snapshot_with_timing_and_fault_control;
use crate::device_layout::CatalogPaths;
use crate::library_db;
use crate::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuilderOperation {
    Check,
    Build,
    Rebuild,
    RebuildAll,
    FreshBuild,
}

/// Scheduling contract for an embedded catalog build.
///
/// The launcher selects `BackgroundContinuous` while animation or interactive
/// UI owns CPU1. That policy applies before first-visible bootstrap and is
/// inherited by every scan/prepare helper, rather than only by the outer
/// catalog-worker thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BuilderExecutionPolicy {
    #[default]
    ForegroundUntilFirstVisible,
    BackgroundContinuous,
}

pub use crate::cooperative_work::{CatalogWorkGateSnapshot, CatalogWorkMode};

/// Controls cooperative catalog checkpoints. Production launcher policy keeps
/// this enabled continuously; tests may close it to exercise checkpoint safety.
pub fn set_background_heavy_work_allowed(allowed: bool) {
    crate::cooperative_work::set_background_allowed(allowed);
}

pub fn set_catalog_work_mode(mode: CatalogWorkMode) -> u64 {
    crate::cooperative_work::set_work_mode(mode)
}

pub fn catalog_work_gate_snapshot() -> CatalogWorkGateSnapshot {
    crate::cooperative_work::work_gate_snapshot()
}

fn wait_for_background_heavy_work_enabled(enabled: bool) {
    if !enabled {
        return;
    }
    let _scope = crate::cooperative_work::BackgroundScope::enter();
    crate::cooperative_work::checkpoint();
}

impl BuilderOperation {
    fn label(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Build => "build",
            Self::Rebuild => "rebuild",
            Self::RebuildAll => "rebuild-all",
            Self::FreshBuild => "fresh-build",
        }
    }
}

pub fn run(
    operation: BuilderOperation,
    emit: impl FnMut(CatalogBuilderEvent),
) -> Result<(), String> {
    let paths = CatalogPaths::capture_process();
    let archive_cache = crate::catalog_config::ArchiveCacheConfig::capture_process(&paths);
    run_with_paths(operation, &paths, &archive_cache, emit)
}

pub fn run_with_paths(
    operation: BuilderOperation,
    paths: &CatalogPaths,
    archive_cache: &crate::catalog_config::ArchiveCacheConfig,
    emit: impl FnMut(CatalogBuilderEvent),
) -> Result<(), String> {
    run_with_execution_policy_and_paths(
        operation,
        BuilderExecutionPolicy::ForegroundUntilFirstVisible,
        paths,
        archive_cache,
        emit,
    )
}

pub fn run_with_execution_policy_and_paths(
    operation: BuilderOperation,
    execution_policy: BuilderExecutionPolicy,
    paths: &CatalogPaths,
    archive_cache: &crate::catalog_config::ArchiveCacheConfig,
    emit: impl FnMut(CatalogBuilderEvent),
) -> Result<(), String> {
    run_with_execution_policy_and_fault_control_and_paths(
        operation,
        execution_policy,
        Box::new(crate::fs_fault::NoopDirectResetFaultControl),
        paths,
        archive_cache,
        emit,
    )
}

pub fn run_with_execution_policy(
    operation: BuilderOperation,
    execution_policy: BuilderExecutionPolicy,
    emit: impl FnMut(CatalogBuilderEvent),
) -> Result<(), String> {
    let paths = CatalogPaths::capture_process();
    let archive_cache = crate::catalog_config::ArchiveCacheConfig::capture_process(&paths);
    run_with_execution_policy_and_paths(operation, execution_policy, &paths, &archive_cache, emit)
}

pub fn run_with_execution_policy_and_fault_control(
    operation: BuilderOperation,
    execution_policy: BuilderExecutionPolicy,
    fault_control: Box<dyn crate::fs_fault::DirectResetFaultControl + Send>,
    emit: impl FnMut(CatalogBuilderEvent),
) -> Result<(), String> {
    let paths = CatalogPaths::capture_process();
    let archive_cache = crate::catalog_config::ArchiveCacheConfig::capture_process(&paths);
    run_with_execution_policy_and_fault_control_and_paths(
        operation,
        execution_policy,
        fault_control,
        &paths,
        &archive_cache,
        emit,
    )
}

pub fn run_with_execution_policy_and_fault_control_and_paths(
    operation: BuilderOperation,
    execution_policy: BuilderExecutionPolicy,
    fault_control: Box<dyn crate::fs_fault::DirectResetFaultControl + Send>,
    paths: &CatalogPaths,
    archive_cache: &crate::catalog_config::ArchiveCacheConfig,
    mut emit: impl FnMut(CatalogBuilderEvent),
) -> Result<(), String> {
    let mut backend = SystemBuilderBackend {
        bootstrap_first_visible: matches!(
            operation,
            BuilderOperation::Build | BuilderOperation::FreshBuild
        ),
        durable_resume: operation_uses_durable_resume(operation),
        post_reveal_background: false,
        force_all_systems: operation == BuilderOperation::RebuildAll,
        allow_post_scan_unchanged: operation_allows_post_scan_unchanged(operation),
        arcade_bootstrap_scan: None,
        fault_control,
        paths: paths.clone(),
        archive_cache: archive_cache.clone(),
    };
    run_with_backend_policy(
        operation,
        execution_policy,
        BuilderConfig::production(paths),
        &mut backend,
        &mut emit,
    )
}

#[derive(Clone, Debug)]
struct BuilderConfig {
    lock_path: PathBuf,
    snapshot_path: PathBuf,
    sqlite_path: PathBuf,
    sharded_catalog_dir: PathBuf,
    run_id: String,
}

impl BuilderConfig {
    fn production(paths: &CatalogPaths) -> Self {
        Self {
            lock_path: std::env::var_os("MISTER_CATALOG_BUILDER_LOCK")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(crate::builder_protocol::DEFAULT_CATALOG_BUILDER_LOCK_PATH)
                }),
            snapshot_path: snapshot_path(),
            sqlite_path: crate::catalog_state::path_for_root(paths.sharded_catalog_dir()),
            sharded_catalog_dir: paths.sharded_catalog_dir().to_path_buf(),
            run_id: format!(
                "{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
            ),
        }
    }
}

#[derive(Clone, Debug)]
struct StageOutput<T> {
    value: T,
    timings: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
enum PostScanDecision<T> {
    Continue(T),
    Unchanged(BuilderSummary),
}

#[derive(Clone, Debug)]
struct PostScanOutput<T> {
    decision: PostScanDecision<T>,
    timings: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
enum CheckDecision {
    Unchanged(BuilderSummary),
    Changed {
        detail: String,
        reason: CatalogChangeReason,
    },
}

#[derive(Clone, Debug)]
struct CheckOutput {
    timing_detail: String,
    decision: CheckDecision,
}

#[derive(Clone, Debug)]
struct StageFailure {
    stage: &'static str,
    error: String,
}

impl StageFailure {
    fn new(stage: &'static str, error: String) -> Self {
        Self { stage, error }
    }
}

trait BuilderBackend {
    type Scan;
    type PreparedScan;
    type Prepared;

    fn fresh_cleanup(&mut self) -> Result<usize, String>;
    fn check(&mut self) -> Result<CheckOutput, StageFailure>;
    fn bootstrap_first_visible(
        &mut self,
        _progress: &mut dyn FnMut(&str, &str),
        _scan_event: &mut dyn FnMut(crate::library_db::LibraryScanEvent),
    ) -> Result<Option<StageOutput<Self::Prepared>>, String> {
        Ok(None)
    }
    fn set_post_reveal_background(&mut self, _background: bool) {}
    fn scan(
        &mut self,
        progress: &mut dyn FnMut(&str, &str),
        scan_event: &mut dyn FnMut(crate::library_db::LibraryScanEvent),
    ) -> Result<StageOutput<Self::Scan>, String>;
    fn decide_after_scan(
        &mut self,
        scan: Self::Scan,
    ) -> Result<PostScanOutput<Self::PreparedScan>, StageFailure>;
    fn prepare(
        &mut self,
        scan: Self::PreparedScan,
        progress: &mut dyn FnMut(&str, &str),
    ) -> Result<StageOutput<Self::Prepared>, String>;
    fn games(&self, prepared: &Self::Prepared) -> usize;
    fn load_us(&self, prepared: &Self::Prepared) -> u64;
    fn write_snapshot(
        &mut self,
        path: &Path,
        prepared: &Self::Prepared,
        progress: &mut dyn FnMut(&str, &str),
    ) -> Result<Vec<(String, String)>, String>;
    fn retain_first_visible_snapshot(
        &mut self,
        _path: &Path,
        _prepared: &Self::Prepared,
    ) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn persist(
        &mut self,
        prepared: Self::Prepared,
        progress: &mut dyn FnMut(&str, &str),
        lifecycle: &mut dyn FnMut(crate::reconciliation_executor::ReconciliationEvent),
    ) -> Result<BuilderSummary, String>;
    fn write_build_duration(
        &mut self,
        sqlite_path: &Path,
        elapsed: Duration,
    ) -> Result<u64, String>;
}

#[cfg(test)]
fn run_with_backend<B: BuilderBackend>(
    operation: BuilderOperation,
    config: BuilderConfig,
    backend: &mut B,
    emit: &mut impl FnMut(CatalogBuilderEvent),
) -> Result<(), String> {
    run_with_backend_policy(
        operation,
        BuilderExecutionPolicy::ForegroundUntilFirstVisible,
        config,
        backend,
        emit,
    )
}

fn run_with_backend_policy<B: BuilderBackend>(
    operation: BuilderOperation,
    execution_policy: BuilderExecutionPolicy,
    config: BuilderConfig,
    backend: &mut B,
    emit: &mut impl FnMut(CatalogBuilderEvent),
) -> Result<(), String> {
    let run_started = Instant::now();
    let sharded_catalog_dir = config.sharded_catalog_dir.clone();
    let protocol = CATALOG_BUILDER_PROTOCOL_VERSION;
    emit(CatalogBuilderEvent::Handshake {
        protocol,
        operation: operation.label().into(),
        run_id: config.run_id.clone(),
    });
    let _lock = BuilderLock::acquire(&config.lock_path)
        .map_err(|error| fail(protocol, "lock", error, emit))?;

    if operation == BuilderOperation::Check {
        let output = backend
            .check()
            .map_err(|failure| fail(protocol, failure.stage, failure.error, emit))?;
        emit(CatalogBuilderEvent::Timing {
            protocol,
            name: "catalog_stamp_check".into(),
            detail: output.timing_detail,
        });
        match output.decision {
            CheckDecision::Unchanged(summary) => {
                emit(CatalogBuilderEvent::Unchanged { protocol, summary });
            }
            CheckDecision::Changed { detail, reason } => {
                emit(CatalogBuilderEvent::Changed {
                    protocol,
                    detail,
                    reason: Some(reason),
                });
            }
        }
        emit(CatalogBuilderEvent::Done { protocol });
        return Ok(());
    }

    if operation == BuilderOperation::FreshBuild {
        emit(CatalogBuilderEvent::FreshCleanupStarted { protocol });
        let removed = backend
            .fresh_cleanup()
            .map_err(|error| fail(protocol, "fresh-cleanup", error, emit))?;
        emit(CatalogBuilderEvent::FreshCleanupCompleted { protocol, removed });
    }

    let snapshot_path = config.snapshot_path;
    let mut snapshot_cleanup = SnapshotCleanup::new(snapshot_path.clone());
    if let Some(parent) = snapshot_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| fail(protocol, "snapshot", error.to_string(), emit))?;
    }
    snapshot_cleanup.arm();

    let background_from_start = execution_policy == BuilderExecutionPolicy::BackgroundContinuous;
    if background_from_start {
        backend.set_post_reveal_background(true);
        apply_runtime_thread_policy(RuntimeThreadRole::CatalogWorker);
        emit(CatalogBuilderEvent::Timing {
            protocol,
            name: "builder_execution_mode".into(),
            detail: "mode=background_continuous affinity=cpu0 nice=5 boundary=builder-start".into(),
        });
    }
    let _startup_background_scope =
        background_from_start.then(crate::cooperative_work::BackgroundScope::enter);

    // Standalone builds stay foreground through first-visible bootstrap. The
    // embedded launcher may instead reserve CPU1 from the builder's first
    // instruction so animation cadence cannot be disturbed.
    let build_role = initial_build_role(operation);
    if !background_from_start {
        apply_runtime_thread_policy(build_role);
    }
    let bootstrap_pmu = mister_magik_perf_events::sampled_span("catalog.bootstrap");
    let bootstrap = {
        let protocol_output = RefCell::new(&mut *emit);
        let mut progress = |title: &str, detail: &str| {
            (protocol_output.borrow_mut())(CatalogBuilderEvent::Progress {
                protocol,
                title: title.into(),
                detail: detail.into(),
                metadata: None,
            });
        };
        backend.bootstrap_first_visible(&mut progress, &mut |event| {
            emit_scan_event(
                protocol,
                operation,
                event,
                &sharded_catalog_dir,
                &mut *protocol_output.borrow_mut(),
            );
        })
    }
    .map_err(|error| fail(protocol, "bootstrap", error, emit))?;
    drop(bootstrap_pmu);
    let first_visible_published = bootstrap.is_some();
    let background_build = background_from_start
        || full_build_runs_in_background(operation)
        || first_visible_published;
    if let Some(bootstrap) = bootstrap {
        emit_timings(protocol, bootstrap.timings, emit);
        let games = backend.games(&bootstrap.value);
        let load_us = backend.load_us(&bootstrap.value);
        let snapshot_started = Instant::now();
        let snapshot_timings = backend
            .write_snapshot(&snapshot_path, &bootstrap.value, &mut |title, detail| {
                emit(CatalogBuilderEvent::Progress {
                    protocol,
                    title: title.into(),
                    detail: detail.into(),
                    metadata: None,
                });
            })
            .map_err(|error| fail(protocol, "bootstrap-snapshot", error, emit))?;
        emit_timings(protocol, snapshot_timings, emit);
        emit(CatalogBuilderEvent::Timing {
            protocol,
            name: "builder_first_visible_ready".into(),
            detail: format!(
                "elapsed_us={} snapshot_us={} games={games}",
                run_started.elapsed().as_micros(),
                snapshot_started.elapsed().as_micros()
            ),
        });
        emit(CatalogBuilderEvent::CatalogReady {
            protocol,
            snapshot_path: snapshot_path.display().to_string(),
            games,
            load_us,
        });
        match backend.retain_first_visible_snapshot(&snapshot_path, &bootstrap.value) {
            Ok(Some(detail)) => emit(CatalogBuilderEvent::Timing {
                protocol,
                name: "builder_arcade_bootstrap_index_publish".into(),
                detail,
            }),
            Ok(None) => {}
            Err(error) => emit(CatalogBuilderEvent::Timing {
                protocol,
                name: "builder_arcade_bootstrap_index_publish".into(),
                detail: format!("status=error error={}", error.replace('\t', " ")),
            }),
        }
    }
    if !background_from_start {
        backend.set_post_reveal_background(background_build);
    }
    if background_build && !background_from_start {
        emit(CatalogBuilderEvent::Timing {
            protocol,
            name: "builder_execution_mode".into(),
            detail: "mode=background_continuous affinity=cpu0 nice=5 boundary=post-first-visible"
                .into(),
        });
    }
    if let Some(role) =
        post_bootstrap_thread_role(background_build, background_from_start, build_role)
    {
        apply_runtime_thread_policy(role);
    }
    // First-visible serialization and publication are foreground. Only after
    // CatalogReady has been emitted and the retained index has been published
    // does the complete pipeline enter the CPU0 background scope.
    let _background_scope = background_build.then(crate::cooperative_work::BackgroundScope::enter);
    let scan_pmu = mister_magik_perf_events::sampled_span("catalog.scan");
    let scanned = {
        let protocol_output = RefCell::new(&mut *emit);
        let mut scan_progress = |title: &str, detail: &str| {
            (protocol_output.borrow_mut())(CatalogBuilderEvent::Progress {
                protocol,
                title: title.into(),
                detail: detail.into(),
                metadata: None,
            });
        };
        backend.scan(&mut scan_progress, &mut |event| {
            emit_scan_event(
                protocol,
                operation,
                event,
                &sharded_catalog_dir,
                &mut *protocol_output.borrow_mut(),
            );
        })
    }
    .map_err(|error| fail(protocol, "scan", error, emit))?;
    drop(scan_pmu);
    emit_timings(protocol, scanned.timings, emit);
    let post_scan = backend
        .decide_after_scan(scanned.value)
        .map_err(|failure| fail(protocol, failure.stage, failure.error, emit))?;
    emit_timings(protocol, post_scan.timings, emit);
    let scanned = match post_scan.decision {
        PostScanDecision::Continue(scanned) => scanned,
        PostScanDecision::Unchanged(summary) => {
            emit(CatalogBuilderEvent::Unchanged { protocol, summary });
            snapshot_cleanup.remove_now();
            crate::catalog_logln!("catalog_builder_event_tsv\tevent=Done");
            emit(CatalogBuilderEvent::Done { protocol });
            return Ok(());
        }
    };
    wait_for_background_heavy_work_enabled(background_build);
    let prepare_pmu = mister_magik_perf_events::sampled_span("catalog.prepare");
    let prepared = {
        let protocol_output = RefCell::new(&mut *emit);
        let mut prepare_progress = |title: &str, detail: &str| {
            wait_for_background_heavy_work_enabled(background_build);
            (protocol_output.borrow_mut())(CatalogBuilderEvent::Progress {
                protocol,
                title: title.into(),
                detail: detail.into(),
                metadata: None,
            });
        };
        backend
            .prepare(scanned, &mut prepare_progress)
            .map_err(|error| fail(protocol, "prepare-catalog", error, emit))?
    };
    drop(prepare_pmu);
    emit_timings(protocol, prepared.timings, emit);
    let games = backend.games(&prepared.value);
    wait_for_background_heavy_work_enabled(background_build);
    if first_visible_published {
        // The launcher already owns the usable Arcade projection. Publishing
        // another all-system snapshot here makes it deserialize the complete
        // RAM catalog only to discard it once the V3 registry is durable.
        // Persisted is the authoritative-generation transition; non-Arcade
        // navigation opens lazily from its system shard after that event.
        emit(CatalogBuilderEvent::Timing {
            protocol,
            name: "builder_authoritative_catalog_prepared".into(),
            detail: format!(
                "elapsed_us={} games={games} resident=arcade-bootstrap",
                run_started.elapsed().as_micros()
            ),
        });
    } else {
        let load_us = backend.load_us(&prepared.value);
        let snapshot_started = Instant::now();
        let snapshot_timings = {
            let protocol_output = RefCell::new(&mut *emit);
            let mut snapshot_progress = |title: &str, detail: &str| {
                wait_for_background_heavy_work_enabled(background_build);
                (protocol_output.borrow_mut())(CatalogBuilderEvent::Progress {
                    protocol,
                    title: title.into(),
                    detail: detail.into(),
                    metadata: None,
                });
            };
            backend
                .write_snapshot(&snapshot_path, &prepared.value, &mut snapshot_progress)
                .map_err(|error| fail(protocol, "snapshot", error, emit))?
        };
        emit_timings(protocol, snapshot_timings, emit);
        emit(CatalogBuilderEvent::Progress {
            protocol,
            title: "Indexing library".into(),
            detail: format!("Opening library — {games} games"),
            metadata: None,
        });
        emit(CatalogBuilderEvent::Timing {
            protocol,
            name: "builder_catalog_ready".into(),
            detail: format!(
                "elapsed_us={} snapshot_us={}",
                run_started.elapsed().as_micros(),
                snapshot_started.elapsed().as_micros()
            ),
        });
        emit(CatalogBuilderEvent::CatalogReady {
            protocol,
            snapshot_path: snapshot_path.display().to_string(),
            games,
            load_us,
        });
        match backend.retain_first_visible_snapshot(&snapshot_path, &prepared.value) {
            Ok(Some(detail)) => emit(CatalogBuilderEvent::Timing {
                protocol,
                name: "builder_arcade_bootstrap_index_refresh".into(),
                detail,
            }),
            Ok(None) => {}
            Err(error) => emit(CatalogBuilderEvent::Timing {
                protocol,
                name: "builder_arcade_bootstrap_index_refresh".into(),
                detail: format!("status=error error={}", error.replace('\t', " ")),
            }),
        }
    }
    apply_runtime_thread_policy(if background_build {
        RuntimeThreadRole::CatalogWorker
    } else {
        build_role
    });
    wait_for_background_heavy_work_enabled(background_build);
    let persist_pmu = mister_magik_perf_events::sampled_span("catalog.persist");
    let persist_result = {
        let shared_emit = RefCell::new(&mut *emit);
        let mut progress = |title: &str, detail: &str| {
            wait_for_background_heavy_work_enabled(background_build);
            (shared_emit.borrow_mut())(CatalogBuilderEvent::Progress {
                protocol,
                title: title.into(),
                detail: detail.into(),
                metadata: None,
            });
        };
        let mut lifecycle = |event| {
            emit_reconciliation_event(protocol, event, &mut *shared_emit.borrow_mut());
        };
        backend.persist(prepared.value, &mut progress, &mut lifecycle)
    };
    drop(persist_pmu);
    let summary = persist_result.map_err(|error| fail(protocol, "persist", error, emit))?;
    let completed_build_seconds = backend
        .write_build_duration(&config.sqlite_path, run_started.elapsed())
        .map_err(|error| fail(protocol, "build-duration", error, emit))?;
    emit(CatalogBuilderEvent::Timing {
        protocol,
        name: "builder_persisted".into(),
        detail: format!(
            "elapsed_us={} completed_build_seconds={completed_build_seconds}",
            run_started.elapsed().as_micros()
        ),
    });
    let mut builder_summary = summary;
    builder_summary.completed_build_seconds = Some(completed_build_seconds);
    crate::catalog_logln!("catalog_builder_event_tsv\tevent=Persisted");
    emit(CatalogBuilderEvent::Persisted {
        protocol,
        summary: builder_summary,
    });
    snapshot_cleanup.remove_now();
    crate::catalog_logln!("catalog_builder_event_tsv\tevent=Done");
    emit(CatalogBuilderEvent::Done { protocol });
    Ok(())
}

fn operation_uses_durable_resume(operation: BuilderOperation) -> bool {
    // Checkpoint fresh construction for interruption recovery. Rebuild replay
    // is deliberately disabled: decoding the whole-card cache costs more than
    // the execution walk it replaces on exFAT.
    matches!(
        operation,
        BuilderOperation::Build | BuilderOperation::FreshBuild
    )
}

fn operation_allows_post_scan_unchanged(operation: BuilderOperation) -> bool {
    operation == BuilderOperation::Rebuild
}

fn initial_build_role(operation: BuilderOperation) -> RuntimeThreadRole {
    if matches!(
        operation,
        BuilderOperation::Rebuild | BuilderOperation::RebuildAll
    ) {
        RuntimeThreadRole::CatalogWorker
    } else {
        RuntimeThreadRole::CatalogForeground
    }
}

fn post_bootstrap_thread_role(
    background_build: bool,
    background_from_start: bool,
    build_role: RuntimeThreadRole,
) -> Option<RuntimeThreadRole> {
    if background_from_start {
        // The continuous policy was installed before bootstrap. Reapplying the
        // initial build role here would silently release CPU0 and compete with
        // the 60 Hz launcher renderer on CPU1.
        None
    } else if background_build {
        Some(RuntimeThreadRole::CatalogWorker)
    } else {
        Some(build_role)
    }
}

fn full_build_runs_in_background(operation: BuilderOperation) -> bool {
    matches!(
        operation,
        BuilderOperation::Rebuild | BuilderOperation::RebuildAll
    )
}

fn emit_timings(
    protocol: u32,
    timings: Vec<(String, String)>,
    emit: &mut impl FnMut(CatalogBuilderEvent),
) {
    for (name, detail) in timings {
        emit(CatalogBuilderEvent::Timing {
            protocol,
            name,
            detail,
        });
    }
}

fn emit_scan_event(
    protocol: u32,
    operation: BuilderOperation,
    event: crate::library_db::LibraryScanEvent,
    sharded_catalog_dir: &Path,
    emit: &mut dyn FnMut(CatalogBuilderEvent),
) {
    match event {
        crate::library_db::LibraryScanEvent::ReconciliationPlanReady {
            system_ids,
            all_published_systems,
        } => {
            let explicit_all = operation == BuilderOperation::RebuildAll;
            let all_published_systems = all_published_systems || explicit_all;
            let mut system_ids = system_ids;
            if all_published_systems
                && let Ok(manifest) = crate::shard_registry::read_latest_manifest_lazy(
                    sharded_catalog_dir,
                    crate::shard_registry::production_registry_limits(),
                )
            {
                system_ids = manifest
                    .systems
                    .into_iter()
                    .map(|system| system.system_id.as_str().to_string())
                    .collect();
            }
            let reason = if explicit_all {
                CatalogPlanReason::ExplicitRebuild
            } else if all_published_systems {
                CatalogPlanReason::ConservativeFallback
            } else {
                CatalogPlanReason::ChangedInput
            };
            let systems = system_ids
                .iter()
                .map(|system_id| CatalogPlannedSystem {
                    system_id: system_id.clone(),
                    action: CatalogPlannedAction::Rebuild,
                    reasons: vec![reason],
                })
                .collect();
            emit(CatalogBuilderEvent::PlanReady {
                protocol,
                system_ids,
                all_published_systems,
                systems,
            });
        }
        crate::library_db::LibraryScanEvent::SystemDiscovered { system_id } => {
            emit(CatalogBuilderEvent::SystemDiscovered {
                protocol,
                system_id,
            });
        }
        crate::library_db::LibraryScanEvent::SystemScanning { system_id } => {
            emit(CatalogBuilderEvent::SystemScanning {
                protocol,
                system_id,
            });
        }
        crate::library_db::LibraryScanEvent::TargetProgress {
            ordinal,
            total,
            path,
            target_kind,
            state,
            completed_targets,
            discoveries,
            execution_mode,
            cooperative_policy,
        } => {
            let detail = format!(
                "target={} of {} state={} path={path}",
                ordinal.saturating_add(1),
                total,
                state
            );
            emit(CatalogBuilderEvent::Progress {
                protocol,
                title: "Scanning library".into(),
                detail,
                metadata: Some(crate::builder_protocol::CatalogProgressMetadata {
                    scan_target: Some(crate::builder_protocol::CatalogScanTargetProgress {
                        ordinal,
                        total,
                        path,
                        target_kind,
                        state,
                        completed_targets,
                        discoveries,
                        execution_mode,
                        cooperative_policy,
                    }),
                }),
            });
        }
    }
}

fn emit_reconciliation_event(
    protocol: u32,
    event: crate::reconciliation_executor::ReconciliationEvent,
    emit: &mut dyn FnMut(CatalogBuilderEvent),
) {
    use crate::reconciliation_executor::ReconciliationEvent;
    match event {
        ReconciliationEvent::SystemScanning { system_id } => {
            emit(CatalogBuilderEvent::SystemScanning {
                protocol,
                system_id: system_id.as_str().to_string(),
            });
        }
        ReconciliationEvent::SystemPrepared {
            system_id,
            generation,
        } => emit(CatalogBuilderEvent::SystemPrepared {
            protocol,
            system_id: system_id.as_str().to_string(),
            generation,
        }),
        ReconciliationEvent::SystemFailed {
            system_id,
            stage,
            error,
        } => emit(CatalogBuilderEvent::SystemFailed {
            protocol,
            system_id: system_id.as_str().to_string(),
            stage,
            error,
        }),
        ReconciliationEvent::ManifestPublished {
            generation,
            rebuilt,
            removed,
        } => emit(CatalogBuilderEvent::ManifestPublished {
            protocol,
            generation,
            rebuilt: rebuilt
                .into_iter()
                .map(|system_id| system_id.as_str().to_string())
                .collect(),
            removed: removed
                .into_iter()
                .map(|system_id| system_id.as_str().to_string())
                .collect(),
        }),
    }
}

struct PreparedBuild {
    persistence: Option<PreparedPersistence>,
    stamp: crate::catalog_stamp::CatalogStamp,
    catalog: ArcadeCatalog,
    scanner_cache: crate::scanner_cache::ScannerCacheState,
    load_us: u64,
    bootstrap_source: BootstrapSource,
}

struct PreparedPersistence {
    catalog_state: crate::catalog_state::CatalogState,
    scan_stats: library_db::LibraryScanStats,
}

impl PreparedPersistence {
    fn from_prepared(state: library_db::LibraryPreparedState) -> Self {
        let (catalog_state, scan_stats) = state.into_parts();
        Self {
            catalog_state,
            scan_stats,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BootstrapSource {
    LiveScan,
    RetainedIndex,
    FullBuild,
}

enum ProductionPostScan {
    Raw(library_db::LibraryRamScanArtifact),
    Audited(library_db::LibraryAuditedScanArtifact),
}

struct SystemBuilderBackend {
    bootstrap_first_visible: bool,
    durable_resume: bool,
    post_reveal_background: bool,
    force_all_systems: bool,
    allow_post_scan_unchanged: bool,
    arcade_bootstrap_scan: Option<library_db::LibraryRamScanArtifact>,
    fault_control: Box<dyn crate::fs_fault::DirectResetFaultControl + Send>,
    paths: CatalogPaths,
    archive_cache: crate::catalog_config::ArchiveCacheConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ProductionRepairStatus {
    Current,
    UpgradeRequired { installed: String, required: String },
    RepairRequired,
}

impl ProductionRepairStatus {
    fn label(&self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::UpgradeRequired { .. } => "upgrade-required",
            Self::RepairRequired => "rebuild-required",
        }
    }
}

fn inspect_v3_before_source_check(storage: &Path) -> Result<ProductionRepairStatus, String> {
    let started = Instant::now();
    let limits = crate::production_sharded_projection::production_registry_limits();
    let generation = crate::shard_registry::read_latest_manifest_lazy(storage, limits)
        .ok()
        .map(|manifest| manifest.generation);
    if let Some(generation) = generation {
        match crate::production_sharded_projection::inspect_production_binding(storage, generation)
        {
            Ok(crate::production_sharded_projection::ProductionBindingStatus::Current {
                ..
            }) => return Ok(ProductionRepairStatus::Current),
            Ok(
                crate::production_sharded_projection::ProductionBindingStatus::UpgradeRequired {
                    installed,
                    required,
                    ..
                },
            ) => {
                return Ok(ProductionRepairStatus::UpgradeRequired {
                    installed,
                    required,
                });
            }
            Err(error)
                if error
                    .to_string()
                    .contains("unsupported future catalog format")
                    || error.to_string().contains("incoherent catalog format") =>
            {
                return Err(error.to_string());
            }
            Err(_) => {}
        }
    }
    crate::catalog_errln!(
        "catalog_v3_repair_tsv\tstatus=rebuild-required\telapsed_us={}",
        started.elapsed().as_micros()
    );
    Ok(ProductionRepairStatus::RepairRequired)
}

/// Remove the complete production catalog so the next launch performs a clean
/// V3 build. Legacy files are removed as hygiene, but are never read or
/// republished.
pub fn remove_default_production_catalog_artifacts() -> Result<usize, String> {
    let paths = CatalogPaths::capture_process();
    let archive_cache = crate::catalog_config::ArchiveCacheConfig::capture_process(&paths);
    remove_production_catalog_artifacts_with_config(&paths, &archive_cache)
}

pub fn remove_production_catalog_artifacts_with_config(
    paths: &CatalogPaths,
    archive_cache: &crate::catalog_config::ArchiveCacheConfig,
) -> Result<usize, String> {
    let storage = paths.sharded_catalog_dir();
    let bootstrap_index = crate::arcade_bootstrap_index::default_path();
    validate_v3_catalog_storage_path(storage)?;
    let mut removed = library_db::remove_catalog_artifacts_with_config(paths, archive_cache)?;
    removed = removed.saturating_add(remove_v3_and_bootstrap_artifacts_at(
        storage,
        &bootstrap_index,
    )?);
    Ok(removed)
}

fn remove_v3_and_bootstrap_artifacts_at(
    storage: &Path,
    bootstrap_index: &Path,
) -> Result<usize, String> {
    validate_v3_catalog_storage_path(storage)?;
    let mut removed = 0usize;
    if storage.exists() {
        let entries = walkdir::WalkDir::new(storage)
            .into_iter()
            .filter_map(Result::ok)
            .count();
        std::fs::remove_dir_all(storage)
            .map_err(|error| format!("remove V3 catalog {}: {error}", storage.display()))?;
        removed = removed.saturating_add(entries);
    }
    match std::fs::remove_file(bootstrap_index) {
        Ok(()) => removed = removed.saturating_add(1),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "remove Arcade bootstrap index {}: {error}",
                bootstrap_index.display()
            ));
        }
    }
    Ok(removed)
}

fn validate_v3_catalog_storage_path(storage: &Path) -> Result<(), String> {
    if storage.file_name().and_then(|name| name.to_str()) != Some("catalog-v3") {
        return Err(format!(
            "refusing to remove unexpected V3 catalog path {}",
            storage.display()
        ));
    }
    Ok(())
}

impl BuilderBackend for SystemBuilderBackend {
    type Scan = library_db::LibraryRamScanArtifact;
    type PreparedScan = ProductionPostScan;
    type Prepared = PreparedBuild;

    fn fresh_cleanup(&mut self) -> Result<usize, String> {
        remove_production_catalog_artifacts_with_config(&self.paths, &self.archive_cache)
    }

    fn set_post_reveal_background(&mut self, background: bool) {
        self.post_reveal_background = background;
    }

    fn check(&mut self) -> Result<CheckOutput, StageFailure> {
        let v3_repair = inspect_v3_before_source_check(self.paths.sharded_catalog_dir())
            .map_err(|error| StageFailure::new("check-format", error))?;
        let check = library_db::sqlite_catalog_stamp_check_with_paths(&self.paths)
            .map_err(|error| StageFailure::new("check", error))?;
        let timing_detail = format!(
            "unchanged={} check_us={} compute_us={} open_us={} read_us={} checkpoint_read_us={} compare_us={} checkpoint_compare_us={} stored={} current={} stored_checkpoint={} current_checkpoint={} stored_lines={} current_lines={} stored_checkpoint_lines={} current_checkpoint_lines={} drift_detail={} v3_repair={}",
            check.unchanged,
            check.check_us,
            check.compute_us,
            check.open_us,
            check.read_us,
            check.checkpoint_read_us,
            check.compare_us,
            check.checkpoint_compare_us,
            check.stored_fingerprint.as_deref().unwrap_or("missing"),
            check.current_fingerprint,
            check
                .stored_checkpoint_fingerprint
                .as_deref()
                .unwrap_or("missing"),
            check.current_checkpoint_fingerprint,
            check.stored_lines,
            check.current_lines,
            check.stored_checkpoint_lines,
            check.current_checkpoint_lines,
            check.drift.detail,
            v3_repair.label(),
        );
        let decision = match v3_repair {
            ProductionRepairStatus::UpgradeRequired {
                installed,
                required,
            } => CheckDecision::Changed {
                detail: format!(
                    "Catalog format update required: installed {installed}, required {required}."
                ),
                reason: CatalogChangeReason::ProjectionUpgrade {
                    installed,
                    required,
                },
            },
            ProductionRepairStatus::Current if check.unchanged => {
                CheckDecision::Unchanged(BuilderSummary::from(
                    library_db::sharded_cached_summary(
                        self.paths.sharded_catalog_dir(),
                        check.check_us,
                    )
                        .map_err(|error| StageFailure::new("summary", error))?,
                ))
            }
            ProductionRepairStatus::RepairRequired if check.unchanged => CheckDecision::Changed {
                detail:
                    "Catalog sources are unchanged, but the V3 generation is incomplete; rebuild required."
                        .to_string(),
                reason: CatalogChangeReason::RepairRequired,
            },
            ProductionRepairStatus::Current | ProductionRepairStatus::RepairRequired => {
                CheckDecision::Changed {
                detail: format!(
                    "Catalog inputs changed; rebuild required. {}",
                    check.drift.detail
                ),
                reason: CatalogChangeReason::InputsChanged,
                }
            }
        };
        Ok(CheckOutput {
            timing_detail,
            decision,
        })
    }

    fn bootstrap_first_visible(
        &mut self,
        progress: &mut dyn FnMut(&str, &str),
        scan_event: &mut dyn FnMut(library_db::LibraryScanEvent),
    ) -> Result<Option<StageOutput<Self::Prepared>>, String> {
        if !self.bootstrap_first_visible {
            return Ok(None);
        }
        let root = crate::arcade_catalog::DEFAULT_ARCADE_ROOT;
        match crate::arcade_bootstrap_index::probe(Path::new(root)) {
            crate::arcade_bootstrap_index::ProbeResult::Hit(loaded) => {
                scan_event(library_db::LibraryScanEvent::SystemDiscovered {
                    system_id: "arcade".to_string(),
                });
                let games = loaded.catalog.len();
                return Ok(Some(StageOutput {
                    value: PreparedBuild {
                        persistence: None,
                        stamp: loaded.stamp,
                        catalog: loaded.catalog,
                        scanner_cache: crate::scanner_cache::ScannerCacheState::default(),
                        load_us: loaded.decode_us,
                        bootstrap_source: BootstrapSource::RetainedIndex,
                    },
                    timings: vec![
                        (
                            "builder_arcade_bootstrap_index_probe".into(),
                            format!(
                                "status=hit elapsed_us={} decode_us={} bytes={} games={games}",
                                loaded.probe_us, loaded.decode_us, loaded.bytes
                            ),
                        ),
                        (
                            "builder_first_visible_scan".into(),
                            format!(
                                "source=retained-index discover_us=0 classify_us=0 discoveries={games} normal_files=0 containers=0 entries=0"
                            ),
                        ),
                        (
                            "builder_first_visible_prepare".into(),
                            format!(
                                "source=retained-index wall_us={} audit_us=0 stamp_us=0 catalog_us={} games={games}",
                                loaded.probe_us, loaded.decode_us
                            ),
                        ),
                    ],
                }));
            }
            crate::arcade_bootstrap_index::ProbeResult::Miss { reason, probe_us } => {
                crate::catalog_logln!(
                    "arcade_bootstrap_index_tsv\tstatus=miss\treason={}\tprobe_us={}",
                    reason,
                    probe_us
                );
            }
        }
        progress("Indexing library", "Scanning Arcade first…");
        let mut scan_events = |event: library_db::LibraryScanEvent| scan_event(event);
        let scanned = if self.post_reveal_background {
            library_db::scan_arcade_bootstrap_ram_background_with_paths(
                &self.paths,
                &self.archive_cache,
                Some(progress),
                Some(&mut scan_events),
            )?
        } else {
            library_db::scan_arcade_bootstrap_ram_foreground_with_paths(
                &self.paths,
                &self.archive_cache,
                Some(progress),
                Some(&mut scan_events),
            )?
        };
        let stats = scanned.stats().clone();
        self.arcade_bootstrap_scan = Some(scanned.clone());
        let (prepared_state, catalog, timing, scanner_cache) = if self.post_reveal_background {
            scanned.complete_coverage_audit_and_catalog_background_with_progress(root, progress)?
        } else {
            scanned.complete_coverage_audit_and_catalog_foreground_with_progress(root, progress)?
        };
        let games = catalog.len();
        let stamp = prepared_state.stamp().clone();
        let persistence = PreparedPersistence::from_prepared(prepared_state);
        trim_catalog_allocator("bootstrap-prepare-complete");
        report_catalog_memory("bootstrap-prepare-complete");
        Ok(Some(StageOutput {
            value: PreparedBuild {
                persistence: Some(persistence),
                stamp,
                load_us: timing.catalog_us,
                catalog,
                scanner_cache,
                bootstrap_source: BootstrapSource::LiveScan,
            },
            timings: vec![
                (
                    "builder_first_visible_scan".into(),
                    format!(
                        "discover_us={} classify_us={} discoveries={} normal_files={} containers={} entries={}",
                        stats.discover_us,
                        stats.classify_us,
                        stats.discoveries,
                        stats.normal_files,
                        stats.containers,
                        stats.entries,
                    ),
                ),
                (
                    "builder_first_visible_prepare".into(),
                    format!(
                        "wall_us={} audit_us={} stamp_us={} catalog_us={} games={}",
                        timing.wall_us, timing.audit_us, timing.stamp_us, timing.catalog_us, games,
                    ),
                ),
            ],
        }))
    }

    fn scan(
        &mut self,
        progress: &mut dyn FnMut(&str, &str),
        scan_event: &mut dyn FnMut(library_db::LibraryScanEvent),
    ) -> Result<StageOutput<Self::Scan>, String> {
        let mut scan_events = |event: library_db::LibraryScanEvent| scan_event(event);
        let arcade_bootstrap_scan = self.arcade_bootstrap_scan.take();
        let arcade_bootstrap_reused = arcade_bootstrap_scan.is_some();
        let background_full_build = self.post_reveal_background;
        let scanned = match (background_full_build, arcade_bootstrap_scan) {
            (true, Some(arcade)) => {
                library_db::scan_library_ram_background_with_paths_reusing_arcade(
                    &self.paths,
                    &self.archive_cache,
                    arcade,
                    Some(progress),
                    Some(&mut scan_events),
                    self.durable_resume,
                )?
            }
            (false, Some(arcade)) => {
                library_db::scan_library_ram_foreground_with_paths_reusing_arcade(
                    &self.paths,
                    &self.archive_cache,
                    arcade,
                    Some(progress),
                    Some(&mut scan_events),
                    self.durable_resume,
                )?
            }
            (true, None) => library_db::scan_library_ram_background_with_paths(
                &self.paths,
                &self.archive_cache,
                Some(progress),
                Some(&mut scan_events),
                self.durable_resume,
            )?,
            (false, None) => library_db::scan_library_ram_foreground_with_paths(
                &self.paths,
                &self.archive_cache,
                Some(progress),
                Some(&mut scan_events),
                self.durable_resume,
            )?,
        };
        let scan_attribution = scanned.scan_attribution_detail();
        let stats = scanned.stats();
        let detail = format!(
            "scan_us={} discover_us={} classify_us={} discoveries={} normal_files={} containers={} entries={} arcade_bootstrap_reused={arcade_bootstrap_reused} {}",
            stats.scan_us,
            stats.discover_us,
            stats.classify_us,
            stats.discoveries,
            stats.normal_files,
            stats.containers,
            stats.entries,
            scan_attribution,
        );
        report_catalog_memory("scan-complete");
        Ok(StageOutput {
            value: scanned,
            timings: vec![("library_scan_complete".into(), detail)],
        })
    }

    fn decide_after_scan(
        &mut self,
        scan: Self::Scan,
    ) -> Result<PostScanOutput<Self::PreparedScan>, StageFailure> {
        if !self.allow_post_scan_unchanged {
            return Ok(PostScanOutput {
                decision: PostScanDecision::Continue(ProductionPostScan::Raw(scan)),
                timings: vec![(
                    "builder_post_scan_unchanged".into(),
                    "status=disabled reason=operation".into(),
                )],
            });
        }

        let started = Instant::now();
        let audited = scan.complete_coverage_audit_for_decision();
        let state_path = crate::catalog_state::path_for_root(self.paths.sharded_catalog_dir());
        let repair = inspect_v3_before_source_check(self.paths.sharded_catalog_dir());
        let stored = crate::catalog_state::read(&state_path);
        let state_matches = stored
            .as_ref()
            .is_ok_and(|stored| stored == audited.catalog_state());
        let current_projection = matches!(&repair, Ok(ProductionRepairStatus::Current));
        let elapsed_us = started.elapsed().as_micros();

        if current_projection && state_matches {
            match crate::library_db::sharded_cached_summary(
                self.paths.sharded_catalog_dir(),
                audited.stats().scan_us,
            ) {
                Ok(mut summary) => {
                    summary.skipped = true;
                    summary.discover_us = audited.stats().discover_us;
                    summary.classify_us = audited.stats().classify_us;
                    return Ok(PostScanOutput {
                        decision: PostScanDecision::Unchanged(BuilderSummary::from(summary)),
                        timings: vec![(
                            "builder_post_scan_unchanged".into(),
                            format!(
                                "status=unchanged elapsed_us={elapsed_us} scan_us={} stamp={}",
                                audited.stats().scan_us,
                                audited.catalog_state().stamp.fingerprint_hex(),
                            ),
                        )],
                    });
                }
                Err(error) => {
                    return Ok(PostScanOutput {
                        decision: PostScanDecision::Continue(ProductionPostScan::Audited(audited)),
                        timings: vec![(
                            "builder_post_scan_unchanged".into(),
                            format!(
                                "status=continue elapsed_us={elapsed_us} reason=summary-unavailable error={}",
                                error.replace('\t', " ").replace('\n', " ")
                            ),
                        )],
                    });
                }
            }
        }

        let reason = if !current_projection {
            match repair {
                Ok(status) => format!("projection-{}", status.label()),
                Err(error) => format!(
                    "projection-error-{}",
                    error.replace('\t', " ").replace('\n', " ")
                ),
            }
        } else if let Err(error) = stored {
            format!(
                "state-unavailable-{}",
                error.replace('\t', " ").replace('\n', " ")
            )
        } else {
            "state-changed".to_string()
        };
        Ok(PostScanOutput {
            decision: PostScanDecision::Continue(ProductionPostScan::Audited(audited)),
            timings: vec![(
                "builder_post_scan_unchanged".into(),
                format!("status=continue elapsed_us={elapsed_us} reason={reason}"),
            )],
        })
    }

    fn prepare(
        &mut self,
        scan: Self::PreparedScan,
        progress: &mut dyn FnMut(&str, &str),
    ) -> Result<StageOutput<Self::Prepared>, String> {
        let root = crate::arcade_catalog::DEFAULT_ARCADE_ROOT;
        let background_full_build = self.post_reveal_background;
        let (prepared_state, catalog, timing, scanner_cache) =
            match scan {
                ProductionPostScan::Audited(scan) => {
                    scan.complete_catalog_background_with_progress(root, progress)?
                }
                ProductionPostScan::Raw(scan) if background_full_build => scan
                    .complete_coverage_audit_and_catalog_background_with_progress(root, progress)?,
                ProductionPostScan::Raw(scan) => scan
                    .complete_coverage_audit_and_catalog_foreground_with_progress(root, progress)?,
            };
        let load_us = timing.catalog_us;
        let persistence = PreparedPersistence::from_prepared(prepared_state);
        trim_catalog_allocator("prepare-complete");
        report_catalog_memory("prepare-complete");
        let timings = vec![
            (
                "builder_deferred_audit_stamp".into(),
                format!(
                    "elapsed_us={} audit_us={} stamp_us={} audit_rows={}",
                    timing.audit_stamp_worker_us,
                    timing.audit_us,
                    timing.stamp_us,
                    persistence.scan_stats.audit_rows
                ),
            ),
            (
                "builder_catalog_projection".into(),
                format!("elapsed_us={} games={}", load_us, catalog.len()),
            ),
            (
                "builder_catalog_metadata".into(),
                format!("elapsed_us={}", timing.metadata_us),
            ),
            (
                "builder_catalog_projection_rows".into(),
                format!("elapsed_us={}", timing.projection_rows_us),
            ),
            (
                "builder_catalog_indexes".into(),
                format!("elapsed_us={}", timing.indexes_us),
            ),
            (
                "builder_catalog_prepare_overlap".into(),
                format!(
                    "wall_us={} audit_stamp_worker_us={} audit_us={} stamp_us={} catalog_us={} overlapped_us={} mode={} worker_role={} worker_affinity={}",
                    timing.wall_us,
                    timing.audit_stamp_worker_us,
                    timing.audit_us,
                    timing.stamp_us,
                    timing.catalog_us,
                    timing.overlapped_us,
                    if background_full_build {
                        "sequential-background"
                    } else {
                        "sequential-foreground"
                    },
                    if background_full_build {
                        RuntimeThreadRole::CatalogWorker.label()
                    } else {
                        RuntimeThreadRole::CatalogForeground.label()
                    },
                    if background_full_build {
                        RuntimeThreadRole::CatalogWorker
                            .default_policy()
                            .affinity
                            .label()
                    } else {
                        RuntimeThreadRole::CatalogForeground
                            .default_policy()
                            .affinity
                            .label()
                    },
                ),
            ),
        ];
        Ok(StageOutput {
            value: PreparedBuild {
                stamp: persistence.catalog_state.stamp.clone(),
                persistence: Some(persistence),
                catalog,
                scanner_cache,
                load_us,
                bootstrap_source: BootstrapSource::FullBuild,
            },
            timings,
        })
    }

    fn games(&self, prepared: &Self::Prepared) -> usize {
        prepared.catalog.len()
    }

    fn load_us(&self, prepared: &Self::Prepared) -> u64 {
        prepared.load_us
    }

    fn write_snapshot(
        &mut self,
        path: &Path,
        prepared: &Self::Prepared,
        progress: &mut dyn FnMut(&str, &str),
    ) -> Result<Vec<(String, String)>, String> {
        let timing = with_builder_progress_heartbeat(
            progress,
            "Creating compressed navigation catalog…",
            || {
                write_catalog_navigation_snapshot_with_timing_and_fault_control(
                    path,
                    &prepared.catalog,
                    &prepared.stamp,
                    &mut *self.fault_control,
                )
            },
        )?;
        Ok(vec![
            (
                "builder_navigation_snapshot".into(),
                format!(
                    "conversion_us={} encode_us={} compress_us={} write_us={} total_us={} encoded_bytes={} compressed_bytes={}",
                    timing.conversion_us,
                    timing.encode_us,
                    timing.compress_us,
                    timing.write_us,
                    timing.total_us,
                    timing.encoded_bytes,
                    timing.compressed_bytes,
                ),
            ),
            (
                "builder_navigation_snapshot_conversion".into(),
                format!("elapsed_us={}", timing.conversion_us),
            ),
            (
                "builder_navigation_snapshot_encode".into(),
                format!("elapsed_us={}", timing.encode_us),
            ),
            (
                "builder_navigation_snapshot_compress".into(),
                format!("elapsed_us={}", timing.compress_us),
            ),
            (
                "builder_navigation_snapshot_write".into(),
                format!("elapsed_us={}", timing.write_us),
            ),
        ])
    }

    fn retain_first_visible_snapshot(
        &mut self,
        path: &Path,
        prepared: &Self::Prepared,
    ) -> Result<Option<String>, String> {
        let (bytes, elapsed_us) = match prepared.bootstrap_source {
            BootstrapSource::LiveScan => {
                crate::arcade_bootstrap_index::publish_from_snapshot(path)?
            }
            BootstrapSource::FullBuild => {
                crate::arcade_bootstrap_index::publish_from_full_catalog(&prepared.catalog)?
            }
            BootstrapSource::RetainedIndex => return Ok(None),
        };
        Ok(Some(format!(
            "status=published elapsed_us={elapsed_us} bytes={bytes} path={}",
            crate::arcade_bootstrap_index::default_path().display()
        )))
    }

    fn persist(
        &mut self,
        prepared: Self::Prepared,
        progress: &mut dyn FnMut(&str, &str),
        lifecycle: &mut dyn FnMut(crate::reconciliation_executor::ReconciliationEvent),
    ) -> Result<BuilderSummary, String> {
        let PreparedBuild {
            persistence,
            stamp: _,
            catalog,
            scanner_cache,
            load_us: _,
            bootstrap_source: _,
        } = prepared;
        let persistence = persistence
            .ok_or_else(|| "cannot persist full catalog from retained Arcade index".to_string())?;
        let catalog_fingerprint = persistence.catalog_state.stamp.fingerprint_hex();
        let catalog_state = persistence.catalog_state;
        let scan_stats = persistence.scan_stats;
        progress("Indexing library", "Publishing system catalogs…");
        let v3_started = Instant::now();
        let projection_started = Instant::now();
        let outcome =
            crate::production_sharded_projection::publish_bound_production_projection_with_events(
                self.paths.sharded_catalog_dir(),
                &catalog,
                &catalog_fingerprint,
                crate::production_sharded_projection::production_registry_limits(),
                self.force_all_systems,
                lifecycle,
            )
            .map_err(|error| format!("publish V3 system catalogs: {error}"))?;
        let projection_us = projection_started.elapsed().as_micros();
        drop(catalog);
        trim_catalog_allocator("shards-complete");
        report_catalog_memory("shards-complete");
        progress("Indexing library", "Saving scanner cache…");
        let scanner_cache_stage_started = Instant::now();
        let staged_scanner_cache = crate::scanner_cache::stage(
            &crate::scanner_cache::path_for_root(self.paths.sharded_catalog_dir()),
            &scanner_cache,
        )?;
        let scanner_cache_stage_us = scanner_cache_stage_started.elapsed().as_micros();
        drop(scanner_cache);
        let scanner_cache_publish_started = Instant::now();
        staged_scanner_cache.publish()?;
        let scanner_cache_publish_us = scanner_cache_publish_started.elapsed().as_micros();
        let scanner_cache_us = scanner_cache_stage_us + scanner_cache_publish_us;
        report_catalog_memory("scanner-cache-complete");
        // Catalog state is the acceptance marker. Publishing it last ensures
        // an interrupted shard/cache write is detected and rebuilt.
        let catalog_state_started = Instant::now();
        crate::catalog_state::write(
            &crate::catalog_state::path_for_root(self.paths.sharded_catalog_dir()),
            &catalog_state,
        )?;
        let catalog_state_us = catalog_state_started.elapsed().as_micros();
        report_catalog_memory("catalog-state-complete");
        let build_progress_path =
            crate::build_progress::path_for_root(self.paths.sharded_catalog_dir());
        crate::build_progress::commit_successful_state(
            &build_progress_path,
            &crate::build_progress::committed_path_for_root(self.paths.sharded_catalog_dir()),
            outcome.generation,
        )?;
        crate::build_progress::remove(&build_progress_path)?;
        let import_us = v3_started.elapsed().as_micros() as u64;
        crate::catalog_logln!(
            "catalog_v3_projection_tsv\tstatus=published\tgeneration={}\tsystems={}\tgames={}\trebuilt_systems={}\tremoved_systems={}\telapsed_us={}",
            outcome.generation,
            outcome.systems,
            outcome.games,
            outcome.rebuilt_systems,
            outcome.removed_systems,
            import_us
        );
        let summary_started = Instant::now();
        let mut summary = crate::library_db::sharded_cached_summary(
            self.paths.sharded_catalog_dir(),
            scan_stats.scan_us,
        )?;
        crate::catalog_logln!(
            "catalog_v3_persist_phases_tsv\tprojection_us={}\tscanner_cache_us={}\tscanner_cache_stage_us={}\tscanner_cache_publish_us={}\tcatalog_state_us={}\tsummary_us={}",
            projection_us,
            scanner_cache_us,
            scanner_cache_stage_us,
            scanner_cache_publish_us,
            catalog_state_us,
            summary_started.elapsed().as_micros(),
        );
        summary.skipped = false;
        summary.discover_us = scan_stats.discover_us;
        summary.classify_us = scan_stats.classify_us;
        summary.import_us = import_us;
        Ok(BuilderSummary::from(summary))
    }

    fn write_build_duration(
        &mut self,
        sqlite_path: &Path,
        elapsed: Duration,
    ) -> Result<u64, String> {
        catalog_build_record::write_completed_build_duration(sqlite_path, elapsed)
    }
}

fn with_builder_progress_heartbeat<T: Send>(
    progress: &mut dyn FnMut(&str, &str),
    detail: &str,
    work: impl FnOnce() -> T + Send,
) -> T {
    let started = Instant::now();
    let background = crate::cooperative_work::in_background_scope();
    progress("Indexing library", detail);
    std::thread::scope(|scope| {
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        scope.spawn(move || {
            if background {
                apply_runtime_thread_policy(RuntimeThreadRole::CatalogWorker);
            }
            let _background_scope =
                background.then(crate::cooperative_work::BackgroundScope::enter);
            crate::cooperative_work::checkpoint();
            let _ = result_tx.send(work());
        });
        loop {
            match result_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(result) => return result,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    progress(
                        "Indexing library",
                        &format!("{detail} — Still working… {}s", started.elapsed().as_secs()),
                    );
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("catalog snapshot worker disconnected")
                }
            }
        }
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProcessMemory {
    rss_kb: u64,
    hwm_kb: u64,
}

fn report_catalog_memory(stage: &str) {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return;
    };
    let Some(memory) = parse_process_memory(&status) else {
        return;
    };
    crate::catalog_logln!(
        "catalog_memory_tsv\tstage={}\trss_kb={}\thwm_kb={}",
        stage,
        memory.rss_kb,
        memory.hwm_kb,
    );
}

#[cfg(target_os = "linux")]
fn trim_catalog_allocator(stage: &str) {
    // Cold-build phases release large, disjoint object graphs. glibc can keep
    // those free arenas resident while the next phase faults in a new working
    // set, so return wholly free pages at the explicit lifetime boundaries.
    let released = unsafe { libc::malloc_trim(0) };
    crate::catalog_logln!("catalog_allocator_trim_tsv\tstage={stage}\treleased={released}");
}

#[cfg(not(target_os = "linux"))]
fn trim_catalog_allocator(_stage: &str) {}

fn parse_process_memory(status: &str) -> Option<ProcessMemory> {
    let mut rss_kb = None;
    let mut hwm_kb = None;
    for line in status.lines() {
        let mut fields = line.split_ascii_whitespace();
        match fields.next() {
            Some("VmRSS:") => rss_kb = fields.next()?.parse().ok(),
            Some("VmHWM:") => hwm_kb = fields.next()?.parse().ok(),
            _ => {}
        }
    }
    Some(ProcessMemory {
        rss_kb: rss_kb?,
        hwm_kb: hwm_kb?,
    })
}

struct SnapshotCleanup {
    path: PathBuf,
    armed: bool,
}

impl SnapshotCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: false }
    }

    fn arm(&mut self) {
        self.armed = true;
    }

    fn remove_now(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
            self.armed = false;
        }
    }
}

impl Drop for SnapshotCleanup {
    fn drop(&mut self) {
        self.remove_now();
    }
}

struct BuilderLock(File);

impl BuilderLock {
    fn acquire(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create catalog builder lock directory: {e}"))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| format!("open catalog builder lock {}: {e}", path.display()))?;
        let status = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if status != 0 {
            return Err(format!(
                "catalog builder already active ({})",
                path.display()
            ));
        }
        Ok(Self(file))
    }
}

impl Drop for BuilderLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn snapshot_path() -> PathBuf {
    std::env::var_os("MISTER_CATALOG_READY_SNAPSHOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(format!(
                "/tmp/mister-magik/catalog-ready-{}.nav.lz4b",
                std::process::id()
            ))
        })
}

fn fail(
    protocol: u32,
    stage: &str,
    error: String,
    emit: &mut impl FnMut(CatalogBuilderEvent),
) -> String {
    emit(CatalogBuilderEvent::Failure {
        protocol,
        stage: stage.into(),
        error: error.clone(),
        diagnostic: Some(CatalogFailureDiagnostic {
            code: CatalogFailureCode::Unknown,
            ..CatalogFailureDiagnostic::default()
        }),
    });
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn reset_removes_v3_catalog_and_retained_arcade_bootstrap_index() {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "mister-magik-reset-artifacts-{}-{id}",
            std::process::id()
        ));
        let storage = root.join("catalog-v3");
        let shard = storage.join("systems/arcade/1.sqlite3");
        let bootstrap_index = root.join("arcade-bootstrap.nav.lz4b");
        std::fs::create_dir_all(shard.parent().unwrap()).unwrap();
        std::fs::write(&shard, b"catalog").unwrap();
        std::fs::write(&bootstrap_index, b"bootstrap").unwrap();

        let removed = remove_v3_and_bootstrap_artifacts_at(&storage, &bootstrap_index).unwrap();

        assert!(removed >= 2);
        assert!(!storage.exists());
        assert!(!bootstrap_index.exists());
        let _ = std::fs::remove_dir(root);
    }

    #[test]
    fn proc_status_memory_parser_requires_current_and_peak_rss() {
        let status =
            "Name:\tmister-magik-fb\nVmPeak:\t250000 kB\nVmHWM:\t127999 kB\nVmRSS:\t96000 kB\n";
        assert_eq!(
            parse_process_memory(status),
            Some(ProcessMemory {
                rss_kb: 96_000,
                hwm_kb: 127_999,
            })
        );
        assert_eq!(parse_process_memory("VmRSS:\t10 kB\n"), None);
    }

    #[test]
    fn prepared_persistence_is_lossless_without_retaining_the_scan() {
        let scan = crate::test_support::sqlite_scan_with_normal_files(&["/games/One.rom"]);
        let stats = library_db::LibraryScanStats {
            scan_us: 10,
            discover_us: 4,
            classify_us: 6,
            normal_files: 1,
            containers: 0,
            entries: 0,
            audit_rows: 0,
            discoveries: 0,
        };
        let artifact = library_db::LibraryScanArtifact {
            scan,
            stats: stats.clone(),
            stamp: crate::catalog_stamp::CatalogStamp::from_lines(vec!["fixture".to_string()]),
        };
        let expected_state = artifact.catalog_state();
        let prepared = library_db::LibraryPreparedState {
            catalog_state: expected_state.clone(),
            stats: stats.clone(),
        };

        let compact = PreparedPersistence::from_prepared(prepared);

        assert_eq!(compact.catalog_state, expected_state);
        assert_eq!(compact.scan_stats.scan_us, stats.scan_us);
        assert_eq!(compact.scan_stats.normal_files, stats.normal_files);
    }

    #[test]
    fn initial_bootstrap_is_foreground_and_replacement_build_is_background() {
        assert_eq!(
            initial_build_role(BuilderOperation::Rebuild),
            RuntimeThreadRole::CatalogWorker
        );
        assert_eq!(
            initial_build_role(BuilderOperation::RebuildAll),
            RuntimeThreadRole::CatalogWorker
        );
        assert_eq!(
            initial_build_role(BuilderOperation::Build),
            RuntimeThreadRole::CatalogForeground
        );
        assert_eq!(
            initial_build_role(BuilderOperation::FreshBuild),
            RuntimeThreadRole::CatalogForeground
        );
        assert!(full_build_runs_in_background(BuilderOperation::Rebuild));
        assert!(full_build_runs_in_background(BuilderOperation::RebuildAll));
        assert!(!full_build_runs_in_background(BuilderOperation::Build));
        assert!(!full_build_runs_in_background(BuilderOperation::FreshBuild));
        assert!(operation_uses_durable_resume(BuilderOperation::Build));
        assert!(!operation_uses_durable_resume(BuilderOperation::Rebuild));
        assert!(!operation_uses_durable_resume(BuilderOperation::RebuildAll));
        assert!(operation_uses_durable_resume(BuilderOperation::FreshBuild));
        assert!(operation_allows_post_scan_unchanged(
            BuilderOperation::Rebuild
        ));
        for operation in [
            BuilderOperation::Check,
            BuilderOperation::Build,
            BuilderOperation::RebuildAll,
            BuilderOperation::FreshBuild,
        ] {
            assert!(!operation_allows_post_scan_unchanged(operation));
        }
    }

    #[derive(Default)]
    struct FakeBackend {
        fail_stage: Option<&'static str>,
        cleanup_removed: usize,
        check_unchanged: bool,
        post_scan_unchanged: bool,
        bootstrap_first_visible: bool,
        calls: Vec<&'static str>,
        bootstrap_background_scopes: Vec<bool>,
        snapshot_background_scopes: Vec<bool>,
        scan_background_scopes: Vec<bool>,
        prepare_background_scopes: Vec<bool>,
        persist_background_scopes: Vec<bool>,
        post_reveal_background: Vec<bool>,
    }

    impl FakeBackend {
        fn fail(&self, stage: &str) -> Result<(), String> {
            if self.fail_stage == Some(stage) {
                Err(format!("fixture {stage} failure"))
            } else {
                Ok(())
            }
        }
    }

    impl BuilderBackend for FakeBackend {
        type Scan = ();
        type PreparedScan = ();
        type Prepared = ();

        fn fresh_cleanup(&mut self) -> Result<usize, String> {
            self.calls.push("fresh-cleanup");
            self.fail("fresh-cleanup")?;
            Ok(self.cleanup_removed)
        }

        fn set_post_reveal_background(&mut self, background: bool) {
            self.post_reveal_background.push(background);
        }

        fn check(&mut self) -> Result<CheckOutput, StageFailure> {
            self.calls.push("check");
            self.fail("check")
                .map_err(|error| StageFailure::new("check", error))?;
            let decision = if self.check_unchanged {
                CheckDecision::Unchanged(BuilderSummary::default())
            } else {
                CheckDecision::Changed {
                    detail: "fixture changed".into(),
                    reason: CatalogChangeReason::InputsChanged,
                }
            };
            Ok(CheckOutput {
                timing_detail: "fixture timing".into(),
                decision,
            })
        }

        fn bootstrap_first_visible(
            &mut self,
            progress: &mut dyn FnMut(&str, &str),
            scan_event: &mut dyn FnMut(library_db::LibraryScanEvent),
        ) -> Result<Option<StageOutput<Self::Prepared>>, String> {
            if !self.bootstrap_first_visible {
                return Ok(None);
            }
            self.calls.push("bootstrap");
            self.bootstrap_background_scopes
                .push(crate::cooperative_work::in_background_scope());
            progress("Indexing library", "Scanning Arcade first…");
            scan_event(library_db::LibraryScanEvent::SystemDiscovered {
                system_id: "arcade".into(),
            });
            Ok(Some(StageOutput {
                value: (),
                timings: vec![("builder_first_visible_scan".into(), "fixture".into())],
            }))
        }

        fn scan(
            &mut self,
            progress: &mut dyn FnMut(&str, &str),
            scan_event: &mut dyn FnMut(library_db::LibraryScanEvent),
        ) -> Result<StageOutput<Self::Scan>, String> {
            self.calls.push("scan");
            self.scan_background_scopes
                .push(crate::cooperative_work::in_background_scope());
            self.fail("scan")?;
            progress("Scanning", "fixture");
            scan_event(library_db::LibraryScanEvent::SystemDiscovered {
                system_id: "arcade".into(),
            });
            Ok(StageOutput {
                value: (),
                timings: vec![("library_scan_complete".into(), "fixture".into())],
            })
        }

        fn decide_after_scan(
            &mut self,
            scan: Self::Scan,
        ) -> Result<PostScanOutput<Self::Scan>, StageFailure> {
            self.calls.push("post-scan-decision");
            self.fail("post-scan-decision")
                .map_err(|error| StageFailure::new("post-scan-decision", error))?;
            Ok(PostScanOutput {
                decision: if self.post_scan_unchanged {
                    PostScanDecision::Unchanged(BuilderSummary {
                        skipped: true,
                        discoveries: 2,
                        ..BuilderSummary::default()
                    })
                } else {
                    PostScanDecision::Continue(scan)
                },
                timings: vec![("builder_post_scan_decision".into(), "fixture".into())],
            })
        }

        fn prepare(
            &mut self,
            _scan: Self::Scan,
            progress: &mut dyn FnMut(&str, &str),
        ) -> Result<StageOutput<Self::Prepared>, String> {
            self.calls.push("prepare-catalog");
            self.prepare_background_scopes
                .push(crate::cooperative_work::in_background_scope());
            self.fail("prepare-catalog")?;
            progress("Indexing library", "Preparing library — 2 discoveries");
            progress("Indexing library", "Resolving playable games — 2 of 2");
            progress("Indexing library", "Building launcher indexes — 2 of 2");
            Ok(StageOutput {
                value: (),
                timings: vec![("builder_catalog_projection".into(), "fixture".into())],
            })
        }

        fn games(&self, _prepared: &Self::Prepared) -> usize {
            2
        }

        fn load_us(&self, _prepared: &Self::Prepared) -> u64 {
            17
        }

        fn write_snapshot(
            &mut self,
            path: &Path,
            _prepared: &Self::Prepared,
            progress: &mut dyn FnMut(&str, &str),
        ) -> Result<Vec<(String, String)>, String> {
            self.calls.push("snapshot");
            self.snapshot_background_scopes
                .push(crate::cooperative_work::in_background_scope());
            progress(
                "Indexing library",
                "Creating compressed navigation catalog…",
            );
            std::fs::write(path, b"fixture-navigation").map_err(|error| error.to_string())?;
            self.fail("snapshot")?;
            Ok(vec![(
                "builder_navigation_snapshot".into(),
                "fixture".into(),
            )])
        }

        fn retain_first_visible_snapshot(
            &mut self,
            path: &Path,
            _prepared: &Self::Prepared,
        ) -> Result<Option<String>, String> {
            self.calls.push("retain-first-visible");
            assert!(
                path.exists(),
                "first-visible snapshot must exist before retention"
            );
            Ok(Some("status=published elapsed_us=1 bytes=18".to_string()))
        }

        fn persist(
            &mut self,
            _prepared: Self::Prepared,
            progress: &mut dyn FnMut(&str, &str),
            _lifecycle: &mut dyn FnMut(crate::reconciliation_executor::ReconciliationEvent),
        ) -> Result<BuilderSummary, String> {
            self.calls.push("persist");
            self.persist_background_scopes
                .push(crate::cooperative_work::in_background_scope());
            self.fail("persist")?;
            progress("Persisting", "fixture");
            Ok(BuilderSummary {
                entries: 2,
                ..BuilderSummary::default()
            })
        }

        fn write_build_duration(
            &mut self,
            _sqlite_path: &Path,
            _elapsed: Duration,
        ) -> Result<u64, String> {
            self.calls.push("build-duration");
            self.fail("build-duration")?;
            Ok(3)
        }
    }

    fn fixture_config(name: &str) -> BuilderConfig {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "mister-magik-builder-service-{}-{name}-{id}",
            std::process::id()
        ));
        BuilderConfig {
            lock_path: root.join("builder.lock"),
            snapshot_path: root.join("ready.nav.lz4b"),
            sqlite_path: root.join("library.sqlite3"),
            sharded_catalog_dir: root.join("catalog-v3"),
            run_id: format!("fixture-{id}"),
        }
    }

    fn event_name(event: &CatalogBuilderEvent) -> &'static str {
        match event {
            CatalogBuilderEvent::Handshake { .. } => "handshake",
            CatalogBuilderEvent::Progress { .. } => "progress",
            CatalogBuilderEvent::PlanReady { .. } => "plan-ready",
            CatalogBuilderEvent::SystemDiscovered { .. } => "system-discovered",
            CatalogBuilderEvent::SystemScanning { .. } => "system-scanning",
            CatalogBuilderEvent::SystemPrepared { .. } => "system-prepared",
            CatalogBuilderEvent::SystemFailed { .. } => "system-failed",
            CatalogBuilderEvent::ManifestPublished { .. } => "manifest-published",
            CatalogBuilderEvent::Timing { .. } => "timing",
            CatalogBuilderEvent::FreshCleanupStarted { .. } => "fresh-cleanup-started",
            CatalogBuilderEvent::FreshCleanupCompleted { .. } => "fresh-cleanup-completed",
            CatalogBuilderEvent::CatalogReady { .. } => "catalog-ready",
            CatalogBuilderEvent::Persisted { .. } => "persisted",
            CatalogBuilderEvent::Unchanged { .. } => "unchanged",
            CatalogBuilderEvent::Changed { .. } => "changed",
            CatalogBuilderEvent::Failure { .. } => "failure",
            CatalogBuilderEvent::Done { .. } => "done",
        }
    }

    #[test]
    fn build_emits_ordered_events_and_snapshot_is_live_only_until_done() {
        let config = fixture_config("success");
        let snapshot = config.snapshot_path.clone();
        let mut backend = FakeBackend::default();
        let mut events = Vec::new();
        run_with_backend(
            BuilderOperation::Build,
            config,
            &mut backend,
            &mut |event| {
                if matches!(event, CatalogBuilderEvent::CatalogReady { .. }) {
                    assert_eq!(std::fs::read(&snapshot).unwrap(), b"fixture-navigation");
                }
                events.push(event);
            },
        )
        .unwrap();

        assert!(!snapshot.exists());
        assert!(matches!(
            events.first(),
            Some(CatalogBuilderEvent::Handshake { operation, .. }) if operation == "build"
        ));
        assert!(matches!(
            events.last(),
            Some(CatalogBuilderEvent::Done { .. })
        ));
        let names = events.iter().map(event_name).collect::<Vec<_>>();
        for pair in [
            ("progress", "system-discovered"),
            ("system-discovered", "catalog-ready"),
            ("catalog-ready", "persisted"),
            ("persisted", "done"),
        ] {
            assert!(
                names.iter().position(|name| *name == pair.0)
                    < names.iter().position(|name| *name == pair.1),
                "event order {pair:?}: {names:?}"
            );
        }
        let opening_library = events
            .iter()
            .position(|event| {
                matches!(event, CatalogBuilderEvent::Progress { detail, .. } if detail == "Opening library — 2 games")
            })
            .expect("opening-library progress event");
        let ready = events
            .iter()
            .position(|event| matches!(event, CatalogBuilderEvent::CatalogReady { .. }))
            .expect("catalog-ready event");
        assert!(opening_library < ready);
        for detail in [
            "Preparing library — 2 discoveries",
            "Resolving playable games — 2 of 2",
            "Building launcher indexes — 2 of 2",
            "Creating compressed navigation catalog…",
            "Opening library — 2 games",
        ] {
            assert!(
                events.iter().any(
                    |event| matches!(event, CatalogBuilderEvent::Progress { detail: actual, .. } if actual == detail)
                ),
                "missing final-phase progress {detail}"
            );
        }
        assert_eq!(
            backend.calls,
            [
                "scan",
                "post-scan-decision",
                "prepare-catalog",
                "snapshot",
                "retain-first-visible",
                "persist",
                "build-duration"
            ]
        );
        assert_eq!(backend.post_reveal_background, [false]);
        assert_eq!(backend.scan_background_scopes, [false]);
        assert_eq!(backend.prepare_background_scopes, [false]);
        assert_eq!(backend.persist_background_scopes, [false]);
    }

    #[test]
    fn fresh_build_reports_cleanup_before_scanning() {
        let config = fixture_config("fresh");
        let mut backend = FakeBackend {
            cleanup_removed: 4,
            ..FakeBackend::default()
        };
        let mut events = Vec::new();
        run_with_backend(
            BuilderOperation::FreshBuild,
            config,
            &mut backend,
            &mut |event| events.push(event),
        )
        .unwrap();
        assert!(matches!(
            events.get(1),
            Some(CatalogBuilderEvent::FreshCleanupStarted { .. })
        ));
        assert!(matches!(
            events.get(2),
            Some(CatalogBuilderEvent::FreshCleanupCompleted { removed: 4, .. })
        ));
        assert_eq!(backend.calls.first(), Some(&"fresh-cleanup"));
    }

    #[test]
    fn cold_bootstrap_publishes_before_the_authoritative_full_scan() {
        set_background_heavy_work_allowed(true);
        let config = fixture_config("bootstrap");
        let mut backend = FakeBackend {
            bootstrap_first_visible: true,
            ..FakeBackend::default()
        };
        let mut events = Vec::new();
        run_with_backend(
            BuilderOperation::Build,
            config,
            &mut backend,
            &mut |event| events.push(event),
        )
        .unwrap();

        assert_eq!(
            backend.calls,
            [
                "bootstrap",
                "snapshot",
                "retain-first-visible",
                "scan",
                "post-scan-decision",
                "prepare-catalog",
                "persist",
                "build-duration"
            ]
        );
        let ready = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| {
                matches!(event, CatalogBuilderEvent::CatalogReady { .. }).then_some(index)
            })
            .collect::<Vec<_>>();
        assert_eq!(ready.len(), 1);
        let retained = events
            .iter()
            .position(|event| {
                matches!(event, CatalogBuilderEvent::Timing { name, .. } if name == "builder_arcade_bootstrap_index_publish")
            })
            .expect("retained first-visible timing");
        let background_transition = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    CatalogBuilderEvent::Timing { name, detail, .. }
                        if name == "builder_execution_mode"
                            && detail.contains("affinity=cpu0")
                )
            })
            .expect("post-reveal background transition");
        assert_eq!(
            backend.snapshot_background_scopes,
            [false],
            "fresh builds publish only the foreground first-visible snapshot"
        );
        assert_eq!(backend.post_reveal_background, [true]);
        assert_eq!(backend.scan_background_scopes, [true]);
        assert_eq!(backend.prepare_background_scopes, [true]);
        assert_eq!(backend.persist_background_scopes, [true]);
        let full_scan_timing = events
            .iter()
            .position(|event| {
                matches!(event, CatalogBuilderEvent::Timing { name, .. } if name == "library_scan_complete")
            })
            .unwrap();
        assert!(ready[0] < full_scan_timing);
        assert!(ready[0] < retained);
        assert!(retained < background_transition);
        assert!(background_transition < full_scan_timing);
        assert!(retained < full_scan_timing);
        let authoritative_prepared = events
            .iter()
            .position(|event| {
                matches!(event, CatalogBuilderEvent::Timing { name, .. } if name == "builder_authoritative_catalog_prepared")
            })
            .expect("authoritative prepared timing");
        let persisted = events
            .iter()
            .position(|event| matches!(event, CatalogBuilderEvent::Persisted { .. }))
            .expect("persisted event");
        assert!(full_scan_timing < authoritative_prepared);
        assert!(authoritative_prepared < persisted);
    }

    #[test]
    fn continuous_background_policy_covers_bootstrap_snapshot_and_full_build() {
        set_background_heavy_work_allowed(true);
        let config = fixture_config("continuous-background");
        let mut backend = FakeBackend {
            bootstrap_first_visible: true,
            ..FakeBackend::default()
        };
        let mut events = Vec::new();

        run_with_backend_policy(
            BuilderOperation::Build,
            BuilderExecutionPolicy::BackgroundContinuous,
            config,
            &mut backend,
            &mut |event| events.push(event),
        )
        .unwrap();

        assert_eq!(backend.post_reveal_background, [true]);
        assert_eq!(backend.bootstrap_background_scopes, [true]);
        assert_eq!(backend.snapshot_background_scopes, [true]);
        assert_eq!(backend.scan_background_scopes, [true]);
        assert_eq!(backend.prepare_background_scopes, [true]);
        assert_eq!(backend.persist_background_scopes, [true]);
        assert!(events.iter().any(|event| {
            matches!(
                event,
                CatalogBuilderEvent::Timing { name, detail, .. }
                    if name == "builder_execution_mode"
                        && detail.contains("boundary=builder-start")
            )
        }));
    }

    #[test]
    fn continuous_background_policy_does_not_release_cpu0_after_bootstrap() {
        assert_eq!(
            post_bootstrap_thread_role(true, true, RuntimeThreadRole::CatalogForeground,),
            None
        );
        assert_eq!(
            post_bootstrap_thread_role(true, false, RuntimeThreadRole::CatalogForeground,),
            Some(RuntimeThreadRole::CatalogWorker)
        );
        assert_eq!(
            post_bootstrap_thread_role(false, false, RuntimeThreadRole::CatalogForeground,),
            Some(RuntimeThreadRole::CatalogForeground)
        );
    }

    #[test]
    fn check_emits_unchanged_or_changed_without_building() {
        for unchanged in [false, true] {
            let config = fixture_config(if unchanged { "unchanged" } else { "changed" });
            let mut backend = FakeBackend {
                check_unchanged: unchanged,
                ..FakeBackend::default()
            };
            let mut events = Vec::new();
            run_with_backend(
                BuilderOperation::Check,
                config,
                &mut backend,
                &mut |event| events.push(event),
            )
            .unwrap();
            assert_eq!(backend.calls, ["check"]);
            assert!(events.iter().any(|event| matches!(
                (unchanged, event),
                (true, CatalogBuilderEvent::Unchanged { .. })
                    | (false, CatalogBuilderEvent::Changed { .. })
            )));
            assert!(matches!(
                events.last(),
                Some(CatalogBuilderEvent::Done { .. })
            ));
        }
    }

    #[test]
    fn unchanged_post_scan_decision_skips_prepare_snapshot_and_persist() {
        let config = fixture_config("post-scan-unchanged");
        let snapshot = config.snapshot_path.clone();
        let mut backend = FakeBackend {
            post_scan_unchanged: true,
            ..FakeBackend::default()
        };
        let mut events = Vec::new();

        run_with_backend(
            BuilderOperation::Rebuild,
            config,
            &mut backend,
            &mut |event| events.push(event),
        )
        .unwrap();

        assert_eq!(backend.calls, ["scan", "post-scan-decision"]);
        assert!(!snapshot.exists());
        assert!(events.iter().any(|event| {
            matches!(
                event,
                CatalogBuilderEvent::Timing { name, .. }
                    if name == "builder_post_scan_decision"
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                CatalogBuilderEvent::Unchanged { summary, .. }
                    if summary.skipped && summary.discoveries == 2
            )
        }));
        assert!(matches!(
            events.last(),
            Some(CatalogBuilderEvent::Done { .. })
        ));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, CatalogBuilderEvent::Persisted { .. }))
        );
    }

    #[test]
    fn every_backend_failure_emits_one_staged_failure_and_cleans_snapshot() {
        for stage in [
            "fresh-cleanup",
            "scan",
            "post-scan-decision",
            "prepare-catalog",
            "snapshot",
            "persist",
            "build-duration",
        ] {
            let config = fixture_config(stage);
            let snapshot = config.snapshot_path.clone();
            let operation = if stage == "fresh-cleanup" {
                BuilderOperation::FreshBuild
            } else {
                BuilderOperation::Build
            };
            let mut backend = FakeBackend {
                fail_stage: Some(stage),
                ..FakeBackend::default()
            };
            let mut events = Vec::new();
            assert!(
                run_with_backend(operation, config, &mut backend, &mut |event| {
                    events.push(event);
                })
                .is_err()
            );
            let failures = events
                .iter()
                .filter_map(|event| match event {
                    CatalogBuilderEvent::Failure { stage, .. } => Some(stage.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(failures, [stage], "events={events:?}");
            assert!(!snapshot.exists(), "snapshot leaked after {stage}");
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, CatalogBuilderEvent::Done { .. }))
            );
        }
    }

    #[test]
    fn real_lock_rejects_overlap_and_is_reusable_after_drop() {
        let config = fixture_config("lock");
        let held = BuilderLock::acquire(&config.lock_path).unwrap();
        let mut backend = FakeBackend::default();
        let mut events = Vec::new();
        assert!(
            run_with_backend(
                BuilderOperation::Build,
                config.clone(),
                &mut backend,
                &mut |event| events.push(event),
            )
            .is_err()
        );
        assert!(matches!(
            events.last(),
            Some(CatalogBuilderEvent::Failure { stage, .. }) if stage == "lock"
        ));
        assert!(backend.calls.is_empty());

        drop(held);
        events.clear();
        run_with_backend(
            BuilderOperation::Build,
            config,
            &mut backend,
            &mut |event| events.push(event),
        )
        .unwrap();
        assert!(matches!(
            events.last(),
            Some(CatalogBuilderEvent::Done { .. })
        ));
    }
}
