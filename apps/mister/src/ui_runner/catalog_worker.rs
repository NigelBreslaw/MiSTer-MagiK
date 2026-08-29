// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::cpu_profile::CatalogBuildProfiler;
use crate::preview_state::SystemEntryPreviewPrelude;
use mister_magik_catalog::arcade_catalog::ArcadeCatalog;
use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use std::ffi::CString;
use std::path::Path;

fn filesystem_available_bytes(path: &str) -> Option<u64> {
    let path = CString::new(path).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    let result = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    let stats = unsafe { stats.assume_init() };
    (stats.f_bavail as u64).checked_mul(stats.f_frsize as u64)
}

fn report_catalog_filesystem_headroom(tx: &mpsc::Sender<CatalogWorkerMessage>, phase: &str) {
    let tmp_available_bytes = filesystem_available_bytes("/tmp/mister-magik");
    let media_available_bytes = filesystem_available_bytes("/media/fat");
    let format_bytes = |value: Option<u64>| {
        value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    };
    let _ = tx.send(CatalogWorkerMessage::Timing {
        name: "catalog_filesystem_headroom".to_string(),
        detail: format!(
            "phase={phase} tmp_available_bytes={} media_available_bytes={}",
            format_bytes(tmp_available_bytes),
            format_bytes(media_available_bytes),
        ),
    });
}

fn send_ready_catalog(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    catalog: ArcadeCatalog,
    load_us: u64,
    source: CatalogSource,
    durable_save_pending: bool,
    generation_fingerprint: Option<String>,
) {
    let publication_started = Instant::now();
    let publication_source = format!("{source:?}");
    let _ = tx.send(CatalogWorkerMessage::Ready {
        catalog,
        load_us,
        source,
        durable_save_pending,
        generation_fingerprint,
        publication_ack: None,
    });
    crate::ui_logln!(
        "catalog_publication_dispatched_tsv\tsource={}\telapsed_us={}\tdurable_save_pending={}",
        publication_source,
        publication_started.elapsed().as_micros(),
        durable_save_pending as u8,
    );
}

fn publish_strict_registry_seed_at(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    root: &str,
    storage: &Path,
) -> Result<(), String> {
    let load_started = Instant::now();
    match load_sharded_registry_seed_at(root, storage) {
        Ok(seed) => {
            let load_us = load_started.elapsed().as_micros() as u64;
            let fingerprint = seed.catalog_fingerprint.clone();
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_strict_registry_load".to_string(),
                detail: format!(
                    "status=ready load_us={load_us} generation={} systems={} resident_games={}",
                    seed.generation,
                    seed.catalog.systems.len(),
                    seed.catalog.games.len()
                ),
            });
            send_ready_catalog(
                tx,
                seed.catalog,
                load_us,
                CatalogSource::ShardedRegistry,
                false,
                Some(fingerprint),
            );
            Ok(())
        }
        Err(error) => {
            let detail = format!(
                "status={} load_us={} error={}",
                error.status,
                load_started.elapsed().as_micros(),
                error.to_string().replace('\t', " ")
            );
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_strict_registry_load".to_string(),
                detail,
            });
            Err(error.to_string())
        }
    }
}

pub(super) fn catalog_refresh_available() -> bool {
    true
}

pub(super) fn start_library_catalog_worker(
    root: String,
    request: CatalogWorkerRequest,
    initial_cache: CatalogWorkerInitialCache,
    execution_mode: CatalogExecutionMode,
    catalog_paths: mister_magik_catalog::device_layout::CatalogPaths,
    _archive_cache: mister_magik_catalog::catalog_config::ArchiveCacheConfig,
) -> mpsc::Receiver<CatalogWorkerMessage> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("catalog-refresh".to_string())
        .spawn(move || {
            apply_runtime_thread_policy(execution_mode.thread_role());
            if request == CatalogWorkerRequest::StrictLoad {
                match publish_strict_registry_seed_at(
                    &tx,
                    &root,
                    catalog_paths.sharded_catalog_dir(),
                ) {
                    Ok(()) => {
                        let _ = tx.send(CatalogWorkerMessage::Done);
                    }
                    Err(error) => {
                        let _ = tx.send(CatalogWorkerMessage::LoadFailed { error });
                    }
                }
                return;
            }
            let _mutation_lease =
                match mister_magik_catalog::catalog_lease::CatalogMutationLease::acquire_default() {
                    Ok(lease) => lease,
                    Err(error) => {
                        let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
                            error: error.to_string(),
                        });
                        return;
                    }
                };
            if let Err(error) =
                mister_magik_catalog::fast_catalog_refresh::cleanup_refresh_temporary_files(
                    catalog_paths.sharded_catalog_dir(),
                )
            {
                let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
                    error: format!("catalog temporary recovery failed: {error}"),
                });
                return;
            }
            let cache_state = match initial_cache {
                CatalogWorkerInitialCache::AlreadyLoadedReady => CatalogCacheState::Ready,
                _ => CatalogCacheState::Missing,
            };
            let plan = catalog_worker_plan(cache_state, request);
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_refresh_decision".to_string(),
                detail: format!(
                    "cache_state={} request={} plan={} execution_mode={}",
                    cache_state.label(),
                    request.label(),
                    plan.label(),
                    execution_mode.label()
                ),
            });
            if plan == CatalogWorkerPlan::LoadOnly {
                let _ = tx.send(CatalogWorkerMessage::Done);
                return;
            }
            run_fast_catalog_refresh_in_process(
                &root,
                plan,
                catalog_paths.sharded_catalog_dir(),
                &tx,
            );
        })
        .expect("spawn catalog-refresh");
    rx
}
fn run_fast_catalog_refresh_in_process(
    root: &str,
    plan: CatalogWorkerPlan,
    catalog_root: &Path,
    tx: &mpsc::Sender<CatalogWorkerMessage>,
) {
    use mister_magik_catalog::fast_catalog_refresh::{
        FastCatalogRefreshRequest, FastCatalogSystemOutcome, FastSourceCheckStatus,
    };

    let mut catalog_profile = CatalogBuildProfiler::capture_process();
    let profile_operation = match plan {
        CatalogWorkerPlan::InitialBuild | CatalogWorkerPlan::FreshBuild => "fresh",
        CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS => "rebuild-all",
        _ => "refresh",
    };
    catalog_profile.arm(profile_operation);
    if matches!(
        plan,
        CatalogWorkerPlan::InitialBuild | CatalogWorkerPlan::FreshBuild
    ) {
        run_fast_catalog_fresh_build(root, catalog_root, tx, catalog_profile);
        return;
    }
    let request = if plan == CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS {
        FastCatalogRefreshRequest::RebuildAll
    } else {
        FastCatalogRefreshRequest::Update
    };
    let storage_root = PathBuf::from("/media/fat");
    report_catalog_filesystem_headroom(tx, "begin");
    let planned = match mister_magik_catalog::fast_catalog_refresh::plan_fast_refresh(
        &storage_root,
        catalog_root,
        request,
    ) {
        Ok(planned) => planned,
        Err(error) => {
            report_catalog_filesystem_headroom(tx, "planning-error");
            catalog_profile.fail("planning-failed");
            let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
                error: format!("fast catalog refresh planning failed: {error}"),
            });
            return;
        }
    };
    let system_ids = planned
        .checks
        .iter()
        .map(|check| check.system_id.clone())
        .collect::<Vec<_>>();
    let _ = tx.send(CatalogWorkerMessage::ReconciliationPlanReady {
        system_ids,
        all_published_systems: false,
    });
    for check in &planned.checks {
        if check.status != FastSourceCheckStatus::Unchanged {
            let _ = tx.send(CatalogWorkerMessage::SystemScanning {
                system_id: check.system_id.clone(),
            });
        }
    }
    let report = match mister_magik_catalog::fast_catalog_refresh::execute_planned_fast_refresh(
        &storage_root,
        catalog_root,
        request,
        planned,
    ) {
        Ok(report) => report,
        Err(error) => {
            report_catalog_filesystem_headroom(tx, "refresh-error");
            catalog_profile.fail("refresh-failed");
            let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
                error: format!("fast catalog refresh failed: {error}"),
            });
            return;
        }
    };
    let mut rebuilt = Vec::new();
    let mut removed = Vec::new();
    for system in &report.system_reports {
        match system.outcome {
            FastCatalogSystemOutcome::Unchanged => {
                let _ = tx.send(CatalogWorkerMessage::SystemPrepared {
                    system_id: system.system_id.clone(),
                    generation: report.catalog_generation,
                });
            }
            FastCatalogSystemOutcome::Updated => {
                rebuilt.push(system.system_id.clone());
                let _ = tx.send(CatalogWorkerMessage::SystemPrepared {
                    system_id: system.system_id.clone(),
                    generation: report.catalog_generation,
                });
            }
            FastCatalogSystemOutcome::Removed => {
                removed.push(system.system_id.clone());
                let _ = tx.send(CatalogWorkerMessage::SystemRemoved {
                    system_id: system.system_id.clone(),
                });
            }
            FastCatalogSystemOutcome::FailedRetained => {
                let _ = tx.send(CatalogWorkerMessage::SystemUpdateFailed {
                    system_id: system.system_id.clone(),
                    error: system.detail.clone(),
                });
            }
        }
    }
    let _ = tx.send(CatalogWorkerMessage::Timing {
        name: "fast_catalog_refresh".to_string(),
        detail: format!(
            "elapsed_us={} planning_us={} manifest_read_us={} active_read_us={} system_discovery_us={} checks_us={} watch_read_us={} metadata_probe_us={} metadata_parents={} metadata_paths={} source_rebuild_us={} artifact_publish_us={} snapshot_publish_us={} systems={} unchanged={} updated={} failed_retained={} artifact_systems_written={}",
            report.elapsed_us,
            report.planning_us,
            report.plan.manifest_read_us,
            report.plan.active_read_us,
            report.plan.system_discovery_us,
            report.plan.checks_us,
            report.plan.watch_read_us,
            report.plan.metadata_probe_us,
            report.plan.metadata_parents,
            report.plan.metadata_paths,
            report.source_rebuild_us,
            report.artifact_publish_us,
            report.snapshot_publish_us,
            report.systems,
            report.unchanged,
            report.updated,
            report.failed_retained,
            report.artifact_systems_written,
        ),
    });
    if !rebuilt.is_empty() || !removed.is_empty() {
        let _ = tx.send(CatalogWorkerMessage::ManifestPublished {
            generation: report.catalog_generation,
            rebuilt,
            removed,
        });
        if let Err(error) = publish_strict_registry_seed_at(tx, root, catalog_root) {
            catalog_profile.fail("registry-reload-failed");
            let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
                error: format!("fast catalog registry reload failed: {error}"),
            });
            return;
        }
    }
    report_catalog_filesystem_headroom(tx, "complete");
    if report.artifact_systems_written == 0 {
        catalog_profile.unchanged();
    } else {
        catalog_profile.persisted();
    }
    let _ = tx.send(CatalogWorkerMessage::Timing {
        name: "catalog_100_percent_complete".to_string(),
        detail: format!(
            "generation={} systems={} unchanged={} updated={} artifacts_written={}",
            report.catalog_generation,
            report.systems,
            report.unchanged,
            report.updated,
            report.artifact_systems_written,
        ),
    });
    let _ = tx.send(CatalogWorkerMessage::Done);
}

fn run_fast_catalog_fresh_build(
    root: &str,
    catalog_root: &Path,
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    mut catalog_profile: CatalogBuildProfiler,
) {
    let storage_root = PathBuf::from("/media/fat");
    report_catalog_filesystem_headroom(tx, "fresh-begin");
    let mut planned_system_ids = Vec::new();
    let mut completed_system_ids = std::collections::BTreeSet::new();
    let report = match mister_magik_catalog::fast_catalog_refresh::build_fresh_catalog_with_progress(
        &storage_root,
        catalog_root,
        |system_ids| {
            planned_system_ids = system_ids.to_vec();
            let _ = tx.send(CatalogWorkerMessage::ReconciliationPlanReady {
                system_ids: system_ids.to_vec(),
                all_published_systems: true,
            });
            for system_id in system_ids {
                if system_id == "arcade" {
                    continue;
                }
                let _ = tx.send(CatalogWorkerMessage::SystemScanning {
                    system_id: system_id.clone(),
                });
            }
            if system_ids.iter().any(|system_id| system_id == "arcade") {
                let _ = tx.send(CatalogWorkerMessage::SystemPrepared {
                    system_id: "arcade".to_string(),
                    generation: 0,
                });
            }
        },
        |system| {
            if completed_system_ids.insert(system.system_id.clone()) {
                let _ = tx.send(CatalogWorkerMessage::SystemPrepared {
                    system_id: system.system_id.clone(),
                    generation: 0,
                });
            }
            if system.system_id == "arcade" && !system.games.is_empty() {
                let started = Instant::now();
                let catalog =
                    mister_magik_catalog::fast_catalog_sources::launcher_catalog_for_fast_system(
                        Path::new(root),
                        system,
                    );
                let games = catalog
                    .system_game_count(mister_magik_catalog::arcade_catalog::MENU_ARCADE_SYSTEM_ID);
                let load_us = started.elapsed().as_micros() as u64;
                let _ = tx.send(CatalogWorkerMessage::Timing {
                    name: "catalog_arcade_bootstrap_ready".to_string(),
                    detail: format!("games={games} load_us={load_us}"),
                });
                send_ready_catalog(
                    tx,
                    catalog,
                    load_us,
                    CatalogSource::NavigationProjection,
                    true,
                    None,
                );
            }
        },
    ) {
        Ok(report) => report,
        Err(error) => {
            report_catalog_filesystem_headroom(tx, "fresh-error");
            catalog_profile.fail("fresh-build-failed");
            let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
                error: format!("catalog build failed: {error}"),
            });
            return;
        }
    };
    for system_id in &planned_system_ids {
        if completed_system_ids.insert(system_id.clone()) {
            let _ = tx.send(CatalogWorkerMessage::SystemPrepared {
                system_id: system_id.clone(),
                generation: report.publication.generation,
            });
        }
    }
    let _ = tx.send(CatalogWorkerMessage::ManifestPublished {
        generation: report.publication.generation,
        rebuilt: report.system_ids.clone(),
        removed: Vec::new(),
    });
    report_catalog_filesystem_headroom(tx, "fresh-complete");
    let _ = tx.send(CatalogWorkerMessage::Timing {
        name: "catalog_fresh_build".to_string(),
        detail: format!(
            "elapsed_us={} source_us={} publish_us={} capture_us={} refresh_state_publish_us={} systems={} games={} copied_bytes={}",
            report.elapsed_us,
            report.source.elapsed_us,
            report.publication.elapsed_us,
            report.capture.elapsed_us,
            report.refresh_state_publish.elapsed_us,
            report.publication.systems,
            report.publication.games,
            report.publication.copied_bytes,
        ),
    });
    let _ = tx.send(CatalogWorkerMessage::Timing {
        name: "catalog_100_percent_complete".to_string(),
        detail: format!(
            "generation={} systems={} games={} artifacts_written={}",
            report.publication.generation,
            report.publication.systems,
            report.publication.games,
            report.publication.systems,
        ),
    });
    let _ = tx.send(CatalogWorkerMessage::BuildCompleted {
        elapsed_us: report.elapsed_us,
    });
    if let Err(error) = publish_strict_registry_seed_at(tx, root, catalog_root) {
        catalog_profile.fail("registry-reload-failed");
        let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
            error: format!("catalog registry load failed: {error}"),
        });
        return;
    }
    catalog_profile.persisted();
    let _ = tx.send(CatalogWorkerMessage::Done);
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogReconcileScope {
    ChangedInputs,
    AllSystems,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogWorkerRequest {
    LoadOnly,
    StrictLoad,
    CheckStamp,
    Reconcile { scope: CatalogReconcileScope },
    FreshBuild,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogExecutionMode {
    ForegroundExclusive,
    BackgroundInteractive,
}

impl CatalogExecutionMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ForegroundExclusive => "foreground_exclusive",
            Self::BackgroundInteractive => "background_interactive",
        }
    }

    fn thread_role(self) -> RuntimeThreadRole {
        match self {
            Self::ForegroundExclusive => RuntimeThreadRole::CatalogForeground,
            Self::BackgroundInteractive => RuntimeThreadRole::CatalogWorker,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogWorkerInitialCache {
    AlreadyLoadedReady,
    AlreadyProbedMissing,
}

impl CatalogWorkerRequest {
    pub(super) const RECONCILE_CHANGED_INPUTS: Self = Self::Reconcile {
        scope: CatalogReconcileScope::ChangedInputs,
    };
    pub(super) const RECONCILE_ALL_SYSTEMS: Self = Self::Reconcile {
        scope: CatalogReconcileScope::AllSystems,
    };

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::LoadOnly => "load_only",
            Self::StrictLoad => "strict_load",
            Self::CheckStamp => "check_stamp",
            Self::Reconcile {
                scope: CatalogReconcileScope::ChangedInputs,
            } => "reconcile_changed_inputs",
            Self::Reconcile {
                scope: CatalogReconcileScope::AllSystems,
            } => "reconcile_all_systems",
            Self::FreshBuild => "fresh_build",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogCacheState {
    Ready,
    Missing,
}

impl CatalogCacheState {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Missing => "missing",
        }
    }

    fn has_usable_catalog(self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogWorkerPlan {
    LoadOnly,
    CheckStamp,
    InitialBuild,
    Reconcile { scope: CatalogReconcileScope },
    FreshBuild,
}

impl CatalogWorkerPlan {
    const RECONCILE_CHANGED_INPUTS: Self = Self::Reconcile {
        scope: CatalogReconcileScope::ChangedInputs,
    };
    const RECONCILE_ALL_SYSTEMS: Self = Self::Reconcile {
        scope: CatalogReconcileScope::AllSystems,
    };

    fn label(self) -> &'static str {
        match self {
            Self::LoadOnly => "load_only",
            Self::CheckStamp => "check_stamp",
            Self::InitialBuild => "initial_build",
            Self::Reconcile {
                scope: CatalogReconcileScope::ChangedInputs,
            } => "reconcile_changed_inputs",
            Self::Reconcile {
                scope: CatalogReconcileScope::AllSystems,
            } => "reconcile_all_systems",
            Self::FreshBuild => "fresh_build",
        }
    }
}

fn catalog_worker_plan(
    cache_state: CatalogCacheState,
    request: CatalogWorkerRequest,
) -> CatalogWorkerPlan {
    match request {
        CatalogWorkerRequest::StrictLoad => return CatalogWorkerPlan::LoadOnly,
        CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS => {
            return CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS;
        }
        CatalogWorkerRequest::FreshBuild => return CatalogWorkerPlan::FreshBuild,
        _ => {}
    }
    match cache_state {
        CatalogCacheState::Ready => match request {
            CatalogWorkerRequest::LoadOnly => CatalogWorkerPlan::LoadOnly,
            CatalogWorkerRequest::StrictLoad => CatalogWorkerPlan::LoadOnly,
            CatalogWorkerRequest::CheckStamp => CatalogWorkerPlan::CheckStamp,
            CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS => {
                CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS
            }
            CatalogWorkerRequest::RECONCILE_ALL_SYSTEMS => CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS,
            CatalogWorkerRequest::FreshBuild => CatalogWorkerPlan::FreshBuild,
        },
        CatalogCacheState::Missing => CatalogWorkerPlan::InitialBuild,
    }
}

pub(super) enum CatalogWorkerMessage {
    Timing {
        name: String,
        detail: String,
    },
    LoadFailed {
        error: String,
    },
    ReconciliationPlanReady {
        system_ids: Vec<String>,
        all_published_systems: bool,
    },
    SystemScanning {
        system_id: String,
    },
    SystemPrepared {
        system_id: String,
        generation: u64,
    },
    SystemRemoved {
        system_id: String,
    },
    SystemUpdateFailed {
        system_id: String,
        error: String,
    },
    ManifestPublished {
        generation: u64,
        rebuilt: Vec<String>,
        removed: Vec<String>,
    },
    BuildCompleted {
        elapsed_us: u64,
    },
    SystemShardReady {
        system_id: String,
        catalog: ArcadeCatalog,
        base_catalog_version: usize,
        game_count: usize,
        prepare_us: u64,
        profile: SystemEntryCatalogProfile,
        preview_prelude: Option<SystemEntryPreviewPrelude>,
    },
    SystemShardFailed {
        system_id: String,
        error: String,
    },
    SearchQueryReady {
        request: launcher::ArcadeSearchRequest,
        result: mister_magik_catalog::persisted_search::PersistedCollectionSearchResult,
    },
    SearchQueryFailed {
        request: launcher::ArcadeSearchRequest,
        error: String,
    },
    HydrationDoneNeedsValidation {
        root: String,
    },
    Ready {
        catalog: ArcadeCatalog,
        load_us: u64,
        source: CatalogSource,
        durable_save_pending: bool,
        generation_fingerprint: Option<String>,
        publication_ack: Option<mpsc::Sender<()>>,
    },
    PersistenceFailed {
        error: String,
    },
    Done,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(super) struct SystemEntryCatalogProfile {
    pub(super) open: mister_magik_catalog::lazy_sharded_reader::LazySystemOpenTiming,
    pub(super) catalog_replacement_us: u64,
    pub(super) total_wall_us: u64,
    pub(super) thread_cpu_us: u64,
    pub(super) cpu_start: i32,
    pub(super) cpu_end: i32,
    pub(super) minor_page_faults: u64,
    pub(super) major_page_faults: u64,
    pub(super) allocations: u64,
    pub(super) allocated_bytes: u64,
}

pub(super) fn print_startup_event(start: Instant, name: &str, detail: impl std::fmt::Display) {
    let elapsed_us = start.elapsed().as_micros();
    let elapsed_ms = elapsed_us / 1_000;
    let detail = detail.to_string();
    boot_analytics::event(
        name,
        format!("since_run_ui_us={elapsed_us} since_run_ui_ms={elapsed_ms} {detail}"),
    );
    crate::ui_logln!("startup_timing\t{name}\t{elapsed_us}us\t{detail}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_builds_missing_catalogs_and_updates_ready_catalogs() {
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Missing, CatalogWorkerRequest::LoadOnly),
            CatalogWorkerPlan::InitialBuild
        );
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Ready, CatalogWorkerRequest::LoadOnly),
            CatalogWorkerPlan::LoadOnly
        );
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Ready, CatalogWorkerRequest::CheckStamp),
            CatalogWorkerPlan::CheckStamp
        );
        assert_eq!(
            catalog_worker_plan(
                CatalogCacheState::Ready,
                CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
            ),
            CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS
        );
        assert_eq!(
            catalog_worker_plan(
                CatalogCacheState::Ready,
                CatalogWorkerRequest::RECONCILE_ALL_SYSTEMS,
            ),
            CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS
        );
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Ready, CatalogWorkerRequest::FreshBuild),
            CatalogWorkerPlan::FreshBuild
        );
    }

    #[test]
    fn refresh_has_no_external_builder_lock() {
        assert!(catalog_refresh_available());
    }
}
