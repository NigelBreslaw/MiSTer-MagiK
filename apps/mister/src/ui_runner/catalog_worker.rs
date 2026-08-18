// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::preview_state::SystemEntryPreviewPrelude;
use mister_magik_catalog::builder_protocol::{
    BuilderSummary, CatalogBuilderEvent, CatalogChangeReason,
};
use mister_magik_catalog::builder_service::{self, BuilderOperation};
use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use mister_magik_catalog::{
    arcade_catalog::{self, ArcadeCatalog},
    catalog_stamp, catalog_summary,
};
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::path::Path;

fn send_ready_catalog(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    catalog: ArcadeCatalog,
    summary: Option<library_db::LibraryRefreshSummary>,
    load_us: u64,
    source: CatalogSource,
    durable_save_pending: bool,
    generation_fingerprint: Option<String>,
) {
    let publication_started = Instant::now();
    let publication_source = format!("{source:?}");
    let (publication_tx, publication_rx) = mpsc::channel();
    let _ = tx.send(CatalogWorkerMessage::Ready {
        catalog,
        summary,
        load_us,
        source,
        durable_save_pending,
        generation_fingerprint,
        publication_ack: Some(publication_tx),
    });
    // The launcher session owns the next transition. For a fresh catalog it
    // schedules indexing only after the durable Persisted event; the worker
    // must therefore return immediately to its SQLite/sidecar save path.
    let _ = publication_rx.recv();
    crate::ui_logln!(
        "catalog_publication_ack_tsv\tsource={}\telapsed_us={}\tdurable_save_pending={}",
        publication_source,
        publication_started.elapsed().as_micros(),
        durable_save_pending as u8,
    );
}

fn publish_persisted_registry_seed_at(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    root: &str,
    storage: &Path,
) -> Option<String> {
    let load_started = Instant::now();
    match load_sharded_registry_seed_at(root, storage) {
        Ok(seed) => {
            let load_us = load_started.elapsed().as_micros() as u64;
            let fingerprint = seed.catalog_fingerprint.clone();
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_post_persist_registry_load".to_string(),
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
                None,
                load_us,
                CatalogSource::ShardedRegistry,
                true,
                Some(fingerprint.clone()),
            );
            Some(fingerprint)
        }
        Err(error) => {
            let detail = format!(
                "status={} load_us={} error={}",
                error.status,
                load_started.elapsed().as_micros(),
                error.to_string().replace('\t', " ")
            );
            crate::ui_errln!("catalog post-persist registry hydration failed: {detail}");
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_post_persist_registry_load".to_string(),
                detail,
            });
            None
        }
    }
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
                None,
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

fn send_persisted_catalog(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    root: &str,
    summary: BuilderSummary,
    catalog_paths: &mister_magik_catalog::device_layout::CatalogPaths,
) {
    let storage = catalog_paths.sharded_catalog_dir();
    send_persisted_catalog_at(
        tx,
        root,
        summary,
        storage,
        &mister_magik_catalog::catalog_state::path_for_root(storage),
    );
}

fn send_persisted_catalog_at(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    root: &str,
    summary: BuilderSummary,
    storage: &Path,
    state_path: &Path,
) {
    let completed_build_seconds = summary.completed_build_seconds;
    let generation_fingerprint =
        publish_persisted_registry_seed_at(tx, root, storage).or_else(|| {
            mister_magik_catalog::catalog_state::read(state_path)
                .ok()
                .map(|state| state.stamp.fingerprint_hex())
        });
    let _ = tx.send(CatalogWorkerMessage::Persisted {
        summary: refresh_summary(summary),
        completed_build_seconds,
        generation_fingerprint,
    });
}

pub(super) fn catalog_builder_lock_available() -> bool {
    let path = std::env::var_os("MISTER_CATALOG_BUILDER_LOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(mister_magik_catalog::builder_protocol::DEFAULT_CATALOG_BUILDER_LOCK_PATH)
        });
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
        Err(_) => return false,
    };
    // SAFETY: flock only acts on this owned file descriptor.
    let acquired = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if acquired {
        // SAFETY: releases the lock acquired immediately above.
        let _ = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    }
    acquired
}

pub(super) fn start_library_catalog_worker(
    root: String,
    request: CatalogWorkerRequest,
    initial_cache: CatalogWorkerInitialCache,
    execution_mode: CatalogExecutionMode,
    catalog_paths: mister_magik_catalog::device_layout::CatalogPaths,
    archive_cache: mister_magik_catalog::catalog_config::ArchiveCacheConfig,
) -> mpsc::Receiver<CatalogWorkerMessage> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("library-catalog".to_string())
        .spawn(move || {
            apply_runtime_thread_policy(execution_mode.thread_role());
            let mut progress_coalescer = CatalogProgressCoalescer::default();
            let mut cache_state = CatalogCacheState::Missing;
            #[cfg(test)]
            let mut cached_catalog_published = false;
            #[cfg(not(test))]
            let cached_catalog_published = false;
            #[cfg(test)]
            let mut projection_repair_allowed = true;
            #[cfg(not(test))]
            let projection_repair_allowed = true;
            #[cfg(test)]
            let mut projection_repair_catalog: Option<(
                ArcadeCatalog,
                catalog_stamp::CatalogStamp,
            )> = None;
            #[cfg(not(test))]
            let projection_repair_catalog: Option<(
                ArcadeCatalog,
                catalog_stamp::CatalogStamp,
            )> = None;
            #[cfg(test)]
            let mut startup_projection_catalog: Option<ArcadeCatalog> = None;
            match initial_cache {
                #[cfg(test)]
                CatalogWorkerInitialCache::ProbeNavigationThenSqlite => {
                    match load_navigation_projection_cache(
                        &root,
                        catalog_paths.library_sqlite(),
                    ) {
                        Ok(Some(loaded)) => {
                            send_catalog_load_timing(
                                &tx,
                                "catalog_worker_navigation_load",
                                &loaded,
                            );
                            let _ = tx.send(CatalogWorkerMessage::Timing {
                                name: "catalog_projection_ready".to_string(),
                                detail: format!(
                                    "status=ready source=navigation_projection games={} load_us={}",
                                    loaded.catalog.games.len(),
                                    loaded.us
                                ),
                            });
                            cache_state = CatalogCacheState::Ready;
                            startup_projection_catalog = Some(loaded.catalog.clone());
                            let generation_fingerprint =
                                loaded.stamp.as_ref().map(|stamp| stamp.fingerprint_hex());
                            send_ready_catalog(
                                &tx,
                                loaded.catalog,
                                None,
                                loaded.us,
                                CatalogSource::NavigationProjection,
                                false,
                                generation_fingerprint,
                            );
                            cached_catalog_published = true;
                        }
                        Ok(None) => {
                            let _ = tx.send(CatalogWorkerMessage::Timing {
                                name: "catalog_worker_navigation_load".to_string(),
                                detail: format!(
                                    "status=missing_or_stale {}",
                                    library_db::catalog_load_counter_detail()
                                ),
                            });
                            let _ = tx.send(CatalogWorkerMessage::Timing {
                                name: "catalog_projection_fallback".to_string(),
                                detail: "status=missing_or_stale".to_string(),
                            });
                        }
                        Err(e) => {
                            crate::ui_errln!("library navigation projection load failed: {e}");
                            let _ = tx.send(CatalogWorkerMessage::Timing {
                                name: "catalog_worker_navigation_load_failed".to_string(),
                                detail: format!("{e} {}", library_db::catalog_load_counter_detail()),
                            });
                            let _ = tx.send(CatalogWorkerMessage::Timing {
                                name: "catalog_projection_fallback".to_string(),
                                detail: format!("status=load_failed error={e}"),
                            });
                        }
                    }
                    if !cache_state.has_usable_catalog() {
                        library_db::record_catalog_worker_cache_load();
                        match library_db::load_arcade_catalog_from_materialized_sqlite(&root) {
                            Ok(loaded) => {
                                send_catalog_load_timing(&tx, "catalog_worker_cache_load", &loaded);
                                if loaded.catalog.games.is_empty() {
                                    cache_state = CatalogCacheState::Empty;
                                } else {
                                    cache_state = CatalogCacheState::Ready;
                                    let catalog_for_repair = loaded.catalog.clone();
                                    let generation_fingerprint =
                                        loaded.stamp.as_ref().map(|stamp| stamp.fingerprint_hex());
                                    send_ready_catalog(
                                        &tx,
                                        loaded.catalog,
                                        None,
                                        loaded.us,
                                        CatalogSource::FullSqlite,
                                        false,
                                        generation_fingerprint,
                                    );
                                    cached_catalog_published = true;
                                    if loaded.projection_repair_safe {
                                        if let Some(stamp) = loaded.stamp {
                                            projection_repair_catalog =
                                                Some((catalog_for_repair, stamp));
                                        }
                                    } else {
                                        projection_repair_allowed = false;
                                        let _ = tx.send(CatalogWorkerMessage::Timing {
                                            name: "catalog_navigation_repair_tsv".to_string(),
                                            detail: "status=skipped_degraded_joined_fallback"
                                                .to_string(),
                                        });
                                    }
                                }
                            }
                            Err(e) => {
                                crate::ui_errln!("library catalog cache load failed: {e}");
                                let _ = tx.send(CatalogWorkerMessage::Timing {
                                    name: "catalog_worker_cache_load_failed".to_string(),
                                    detail: e.clone(),
                                });
                                if library_db::is_catalog_schema_mismatch_error(&e) {
                                    let _ = tx.send(CatalogWorkerMessage::Timing {
                                        name: "catalog_schema_mismatch_rebuild".to_string(),
                                        detail: e,
                                    });
                                    cache_state = CatalogCacheState::Missing;
                                } else {
                                    let _ =
                                        tx.send(CatalogWorkerMessage::LoadFailed { error: e });
                                    return;
                                }
                            }
                        }
                    } else if let Some(projected) = startup_projection_catalog.as_ref() {
                        library_db::record_catalog_worker_cache_load();
                        let materialized_parity_started = std::time::Instant::now();
                        let _ = tx.send(CatalogWorkerMessage::Timing {
                            name: "catalog_materialized_parity_started".to_string(),
                            detail: format!("projection_games={}", projected.games.len()),
                        });
                        match library_db::load_arcade_catalog_from_materialized_sqlite(&root) {
                            Ok(loaded) if !loaded.catalog.games.is_empty() => {
                                let parity_elapsed_us =
                                    materialized_parity_started.elapsed().as_micros();
                                send_catalog_load_timing(
                                    &tx,
                                    "catalog_worker_materialized_parity_load",
                                    &loaded,
                                );
                                let mismatches = loaded.catalog.filter_option_mismatches(projected);
                                let parity_status = if mismatches.is_empty() {
                                    "match"
                                } else {
                                    "mismatch"
                                };
                                let _ = tx.send(CatalogWorkerMessage::Timing {
                                    name: "catalog_materialized_parity_finished".to_string(),
                                    detail: format!(
                                        "status={parity_status} elapsed_us={parity_elapsed_us} load_us={} games={} mismatches={}",
                                        loaded.us,
                                        loaded.catalog.games.len(),
                                        mismatches.len()
                                    ),
                                });
                                if !mismatches.is_empty() {
                                    let _ = tx.send(CatalogWorkerMessage::Timing {
                                        name: "catalog_filter_parity_tsv".to_string(),
                                        detail: format!(
                                            "status=mismatch collections={} detail={}",
                                            mismatches.len(),
                                            mismatches.join(" | ")
                                        ),
                                    });
                                    let generation_fingerprint =
                                        loaded.stamp.as_ref().map(|stamp| stamp.fingerprint_hex());
                                    send_ready_catalog(
                                        &tx,
                                        loaded.catalog.clone(),
                                        None,
                                        loaded.us,
                                        CatalogSource::FullSqlite,
                                        false,
                                        generation_fingerprint,
                                    );
                                }
                                if loaded.projection_repair_safe {
                                    if let Some(stamp) = loaded.stamp {
                                        projection_repair_catalog =
                                            Some((loaded.catalog, stamp));
                                    }
                                } else {
                                    projection_repair_allowed = false;
                                }
                            }
                            Ok(_) => {
                                let _ = tx.send(CatalogWorkerMessage::Timing {
                                    name: "catalog_materialized_parity_finished".to_string(),
                                    detail: format!(
                                        "status=empty elapsed_us={}",
                                        materialized_parity_started.elapsed().as_micros()
                                    ),
                                });
                            }
                            Err(e) => {
                                let _ = tx.send(CatalogWorkerMessage::Timing {
                                    name: "catalog_materialized_parity_finished".to_string(),
                                    detail: format!(
                                        "status=load_failed elapsed_us={} error={e}",
                                        materialized_parity_started.elapsed().as_micros()
                                    ),
                                });
                                let _ = tx.send(CatalogWorkerMessage::Timing {
                                    name: "catalog_worker_materialized_parity_load_failed"
                                        .to_string(),
                                    detail: e.clone(),
                                });
                                if library_db::is_catalog_schema_mismatch_error(&e) {
                                    let _ = tx.send(CatalogWorkerMessage::Timing {
                                        name: "catalog_schema_mismatch_rebuild".to_string(),
                                        detail: e,
                                    });
                                    cache_state = CatalogCacheState::Missing;
                                }
                            }
                        }
                    }
                }
                #[cfg(test)]
                CatalogWorkerInitialCache::ProbeSqlite => {
                    library_db::record_catalog_worker_cache_load();
                    match library_db::load_arcade_catalog_from_materialized_sqlite(&root) {
                        Ok(loaded) => {
                            send_catalog_load_timing(&tx, "catalog_worker_cache_load", &loaded);
                            if loaded.catalog.games.is_empty() {
                                cache_state = CatalogCacheState::Empty;
                            } else {
                                cache_state = CatalogCacheState::Ready;
                                let catalog_for_repair = loaded.catalog.clone();
                                let generation_fingerprint =
                                    loaded.stamp.as_ref().map(|stamp| stamp.fingerprint_hex());
                                send_ready_catalog(
                                    &tx,
                                    loaded.catalog,
                                    None,
                                    loaded.us,
                                    CatalogSource::FullSqlite,
                                    false,
                                    generation_fingerprint,
                                );
                                cached_catalog_published = true;
                                if loaded.projection_repair_safe {
                                    if let Some(stamp) = loaded.stamp {
                                        projection_repair_catalog =
                                            Some((catalog_for_repair, stamp));
                                    }
                                } else {
                                    projection_repair_allowed = false;
                                    let _ = tx.send(CatalogWorkerMessage::Timing {
                                        name: "catalog_navigation_repair_tsv".to_string(),
                                        detail: "status=skipped_degraded_joined_fallback"
                                            .to_string(),
                                    });
                                }
                            }
                        }
                        Err(e) => {
                            crate::ui_errln!("library catalog cache load failed: {e}");
                            let _ = tx.send(CatalogWorkerMessage::Timing {
                                name: "catalog_worker_cache_load_failed".to_string(),
                                detail: e.clone(),
                            });
                            if library_db::is_catalog_schema_mismatch_error(&e) {
                                let _ = tx.send(CatalogWorkerMessage::Timing {
                                    name: "catalog_schema_mismatch_rebuild".to_string(),
                                    detail: e,
                                });
                                cache_state = CatalogCacheState::Missing;
                            } else {
                                let _ = tx.send(CatalogWorkerMessage::LoadFailed { error: e });
                                return;
                            }
                        }
                    }
                }
                CatalogWorkerInitialCache::AlreadyLoadedReady => {
                    cache_state = CatalogCacheState::Ready;
                    let _ = tx.send(CatalogWorkerMessage::Timing {
                        name: "catalog_worker_initial_cache".to_string(),
                        detail: "source=already_loaded state=ready".to_string(),
                    });
                }
                CatalogWorkerInitialCache::AlreadyProbedMissing => {
                    let _ = tx.send(CatalogWorkerMessage::Timing {
                        name: "catalog_worker_initial_cache".to_string(),
                        detail: "source=ui_probe state=missing".to_string(),
                    });
                }
                #[cfg(test)]
                CatalogWorkerInitialCache::AlreadyProbedEmpty => {
                    cache_state = CatalogCacheState::Empty;
                    let _ = tx.send(CatalogWorkerMessage::Timing {
                        name: "catalog_worker_initial_cache".to_string(),
                        detail: "source=ui_probe state=empty".to_string(),
                    });
                }
            }
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
            let plan = catalog_worker_plan(cache_state, request);
            let dispatch = catalog_worker_dispatch(
                plan,
                execution_mode,
                cached_catalog_published,
            );
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
            if dispatch.deferred_cached_validation {
                repair_navigation_projection_cache_after_ready(
                    &root,
                    projection_repair_catalog.as_ref(),
                    projection_repair_allowed,
                    catalog_paths.library_sqlite(),
                    &tx,
                );
                let _ = tx.send(CatalogWorkerMessage::Timing {
                    name: "catalog_hydration_handoff".to_string(),
                    detail: "status=validation_deferred request=check_stamp".to_string(),
                });
                let _ = tx.send(CatalogWorkerMessage::HydrationDoneNeedsValidation {
                    root,
                });
                return;
            }
            if dispatch.completes_without_builder {
                let _ = tx.send(CatalogWorkerMessage::Done);
                return;
            }
            if dispatch.sends_initial_progress {
                send_catalog_progress(
                    &tx,
                    library_db::CatalogProgress::indexing_building_catalog(),
                );
            }
            if let Some(builder_execution_mode) = dispatch.builder_execution_mode {
                run_catalog_builder_in_process(
                    &root,
                    plan,
                    builder_execution_mode,
                    &catalog_paths,
                    &archive_cache,
                    &tx,
                    &mut progress_coalescer,
                );
                return;
            }
        })
        .expect("spawn library-catalog");
    rx
}

fn run_catalog_builder_in_process(
    root: &str,
    plan: CatalogWorkerPlan,
    execution_mode: CatalogExecutionMode,
    catalog_paths: &mister_magik_catalog::device_layout::CatalogPaths,
    archive_cache: &mister_magik_catalog::catalog_config::ArchiveCacheConfig,
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    progress_coalescer: &mut CatalogProgressCoalescer,
) {
    let Some((operation, operation_label)) = catalog_builder_operation(plan, execution_mode) else {
        return;
    };
    let mut state = EmbeddedBuilderEventState::default();
    let execution_policy = match execution_mode {
        CatalogExecutionMode::ForegroundExclusive => {
            builder_service::BuilderExecutionPolicy::ForegroundUntilFirstVisible
        }
        CatalogExecutionMode::BackgroundInteractive => {
            builder_service::BuilderExecutionPolicy::BackgroundContinuous
        }
    };
    let result = builder_service::run_with_execution_policy_and_fault_control_and_paths(
        operation,
        execution_policy,
        Box::new(mister_magik_mister_runtime::direct_reset_fault::process_fault_control()),
        catalog_paths,
        archive_cache,
        |event| {
            handle_embedded_builder_event_with_paths(
                root,
                plan,
                operation_label,
                event,
                catalog_paths,
                tx,
                progress_coalescer,
                &mut state,
            );
        },
    );
    mister_magik_perf_events::submit_thread_profile("catalog-builder");
    match result {
        Ok(()) if state.handshake_seen && state.terminal_seen => {}
        Ok(()) => {
            send_builder_failure(
                tx,
                plan,
                state.catalog_ready_seen,
                format!(
                    "catalog builder returned without a complete event sequence; handshake={} terminal={}",
                    state.handshake_seen, state.terminal_seen
                ),
            );
        }
        Err(_) if state.terminal_seen => {}
        Err(error) => {
            send_builder_failure(
                tx,
                plan,
                state.catalog_ready_seen,
                format!("catalog builder failed without a terminal event: {error}"),
            );
        }
    }
}

#[derive(Default)]
struct EmbeddedBuilderEventState {
    handshake_seen: bool,
    terminal_seen: bool,
    catalog_ready_seen: bool,
}

fn handle_embedded_builder_event_with_paths(
    root: &str,
    plan: CatalogWorkerPlan,
    expected_operation: &str,
    event: CatalogBuilderEvent,
    catalog_paths: &mister_magik_catalog::device_layout::CatalogPaths,
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    progress_coalescer: &mut CatalogProgressCoalescer,
    state: &mut EmbeddedBuilderEventState,
) {
    if !state.handshake_seen && !matches!(event, CatalogBuilderEvent::Handshake { .. }) {
        send_builder_failure(
            tx,
            plan,
            state.catalog_ready_seen,
            "catalog builder emitted an event before its handshake".into(),
        );
        state.terminal_seen = true;
        return;
    }
    match event {
        CatalogBuilderEvent::Handshake { operation, .. }
            if !state.handshake_seen && operation == expected_operation =>
        {
            state.handshake_seen = true;
        }
        CatalogBuilderEvent::Handshake { .. } => {
            send_builder_failure(
                tx,
                plan,
                state.catalog_ready_seen,
                "catalog builder emitted a duplicate or mismatched handshake".into(),
            );
            state.terminal_seen = true;
        }
        CatalogBuilderEvent::Progress {
            title,
            detail,
            metadata,
            ..
        } => {
            if metadata.is_some() {
                let _ = tx.send(CatalogWorkerMessage::Progress {
                    title,
                    detail,
                    percent: -1,
                    metadata,
                });
            } else {
                send_catalog_progress_text(tx, progress_coalescer, &title, &detail);
            }
        }
        CatalogBuilderEvent::PlanReady {
            system_ids,
            all_published_systems,
            systems: _,
            ..
        } => {
            let all_published_systems =
                all_published_systems || plan == CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS;
            let _ = tx.send(CatalogWorkerMessage::ReconciliationPlanReady {
                system_ids: if all_published_systems {
                    Vec::new()
                } else {
                    system_ids
                },
                all_published_systems,
            });
        }
        CatalogBuilderEvent::SystemDiscovered { system_id, .. } => {
            let _ = tx.send(CatalogWorkerMessage::SystemDiscovered { system_id });
        }
        CatalogBuilderEvent::SystemScanning { system_id, .. } => {
            let _ = tx.send(CatalogWorkerMessage::SystemScanning { system_id });
        }
        CatalogBuilderEvent::SystemPrepared {
            system_id,
            generation,
            ..
        } => {
            let _ = tx.send(CatalogWorkerMessage::SystemPrepared {
                system_id,
                generation,
            });
        }
        CatalogBuilderEvent::SystemFailed {
            system_id,
            stage,
            error,
            ..
        } => {
            let _ = tx.send(CatalogWorkerMessage::SystemUpdateFailed {
                system_id,
                error: format!("{stage}: {error}"),
            });
        }
        CatalogBuilderEvent::ManifestPublished {
            generation,
            rebuilt,
            removed,
            ..
        } => {
            let _ = tx.send(CatalogWorkerMessage::ManifestPublished {
                generation,
                rebuilt,
                removed,
            });
        }
        CatalogBuilderEvent::Timing { name, detail, .. } => {
            let _ = tx.send(CatalogWorkerMessage::Timing { name, detail });
        }
        CatalogBuilderEvent::FreshCleanupStarted { .. } => {
            let _ = tx.send(CatalogWorkerMessage::FreshCleanupStarted);
        }
        CatalogBuilderEvent::FreshCleanupCompleted { removed, .. } => {
            let _ = tx.send(CatalogWorkerMessage::FreshCleanupCompleted { removed });
        }
        CatalogBuilderEvent::CatalogReady {
            snapshot_path,
            load_us,
            ..
        } => {
            if matches!(
                plan,
                CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS
                    | CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS
            ) {
                let _ = tx.send(CatalogWorkerMessage::Timing {
                    name: "catalog_warm_bootstrap_ignored".to_string(),
                    detail: "reason=published_catalog_remains_authoritative".to_string(),
                });
                return;
            }
            match library_db::load_arcade_catalog_from_snapshot(
                root,
                std::path::Path::new(&snapshot_path),
            ) {
                Ok(loaded) => {
                    state.catalog_ready_seen = true;
                    send_ready_catalog(
                        tx,
                        loaded.catalog,
                        None,
                        load_us,
                        CatalogSource::FreshBuild,
                        true,
                        None,
                    );
                }
                Err(error) => {
                    let _ = tx.send(CatalogWorkerMessage::LoadFailed { error });
                }
            }
        }
        CatalogBuilderEvent::Persisted { summary, .. } => {
            send_persisted_catalog(tx, root, summary, catalog_paths);
        }
        CatalogBuilderEvent::Unchanged { summary, .. } => {
            let _ = tx.send(CatalogWorkerMessage::Unchanged {
                summary: refresh_summary(summary),
            });
        }
        CatalogBuilderEvent::Changed { detail, reason, .. } => {
            let _ = tx.send(CatalogWorkerMessage::Changed {
                detail,
                reason: reason.unwrap_or(CatalogChangeReason::RepairRequired),
            });
        }
        CatalogBuilderEvent::Failure { stage, error, .. } => {
            state.terminal_seen = true;
            send_builder_failure(
                tx,
                plan,
                state.catalog_ready_seen,
                format!("catalog builder {stage} failed: {error}"),
            );
        }
        CatalogBuilderEvent::Done { .. } => {
            state.terminal_seen = true;
            let _ = tx.send(CatalogWorkerMessage::Done);
        }
    }
}

#[cfg(test)]
fn handle_embedded_builder_event(
    root: &str,
    plan: CatalogWorkerPlan,
    expected_operation: &str,
    event: CatalogBuilderEvent,
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    progress_coalescer: &mut CatalogProgressCoalescer,
    state: &mut EmbeddedBuilderEventState,
) {
    let paths = mister_magik_catalog::device_layout::CatalogPaths::capture_process();
    handle_embedded_builder_event_with_paths(
        root,
        plan,
        expected_operation,
        event,
        &paths,
        tx,
        progress_coalescer,
        state,
    );
}

fn catalog_builder_operation(
    plan: CatalogWorkerPlan,
    _execution_mode: CatalogExecutionMode,
) -> Option<(BuilderOperation, &'static str)> {
    Some(match plan {
        CatalogWorkerPlan::CheckStamp => (BuilderOperation::Check, "check"),
        CatalogWorkerPlan::InitialBuild => (BuilderOperation::Build, "build"),
        CatalogWorkerPlan::Reconcile {
            scope: CatalogReconcileScope::ChangedInputs,
        } => (BuilderOperation::Rebuild, "rebuild"),
        CatalogWorkerPlan::Reconcile {
            scope: CatalogReconcileScope::AllSystems,
        } => (BuilderOperation::RebuildAll, "rebuild-all"),
        CatalogWorkerPlan::FreshBuild => (BuilderOperation::FreshBuild, "fresh-build"),
        CatalogWorkerPlan::LoadOnly => return None,
    })
}

fn send_builder_failure(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    plan: CatalogWorkerPlan,
    catalog_ready_seen: bool,
    error: String,
) {
    let message = if plan == CatalogWorkerPlan::FreshBuild && !catalog_ready_seen {
        CatalogWorkerMessage::LoadFailed { error }
    } else {
        CatalogWorkerMessage::PersistenceFailed { error }
    };
    let _ = tx.send(message);
}

fn refresh_summary(value: BuilderSummary) -> library_db::LibraryRefreshSummary {
    library_db::LibraryRefreshSummary {
        skipped: value.skipped,
        scan_us: value.scan_us,
        discover_us: value.discover_us,
        classify_us: value.classify_us,
        import_us: value.import_us,
        bytes: value.bytes,
        normal_files: value.normal_files,
        containers: value.containers,
        entries: value.entries,
        audit_rows: value.audit_rows,
        discoveries: value.discoveries,
    }
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
    #[cfg(test)]
    ProbeNavigationThenSqlite,
    #[cfg(test)]
    ProbeSqlite,
    AlreadyLoadedReady,
    AlreadyProbedMissing,
    #[cfg(test)]
    AlreadyProbedEmpty,
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
    Empty,
    Missing,
}

impl CatalogCacheState {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Empty => "empty",
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CatalogWorkerDispatch {
    completes_without_builder: bool,
    deferred_cached_validation: bool,
    builder_execution_mode: Option<CatalogExecutionMode>,
    sends_initial_progress: bool,
}

fn catalog_worker_dispatch(
    plan: CatalogWorkerPlan,
    execution_mode: CatalogExecutionMode,
    cached_catalog_published: bool,
) -> CatalogWorkerDispatch {
    if cached_catalog_published && plan == CatalogWorkerPlan::CheckStamp {
        return CatalogWorkerDispatch {
            completes_without_builder: false,
            deferred_cached_validation: true,
            builder_execution_mode: None,
            sends_initial_progress: false,
        };
    }
    CatalogWorkerDispatch {
        completes_without_builder: plan == CatalogWorkerPlan::LoadOnly,
        deferred_cached_validation: false,
        builder_execution_mode: (plan != CatalogWorkerPlan::LoadOnly).then_some(execution_mode),
        sends_initial_progress: matches!(
            plan,
            CatalogWorkerPlan::InitialBuild
                | CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS
                | CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS
        ),
    }
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
        CatalogCacheState::Empty | CatalogCacheState::Missing => CatalogWorkerPlan::InitialBuild,
    }
}

pub(super) enum CatalogWorkerMessage {
    Timing {
        name: String,
        detail: String,
    },
    Progress {
        title: String,
        detail: String,
        percent: i32,
        metadata: Option<mister_magik_catalog::builder_protocol::CatalogProgressMetadata>,
    },
    LoadFailed {
        error: String,
    },
    FreshCleanupStarted,
    FreshCleanupCompleted {
        removed: usize,
    },
    ReconciliationPlanReady {
        system_ids: Vec<String>,
        all_published_systems: bool,
    },
    SystemDiscovered {
        system_id: String,
    },
    SystemScanning {
        system_id: String,
    },
    SystemPrepared {
        system_id: String,
        generation: u64,
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
        summary: Option<library_db::LibraryRefreshSummary>,
        load_us: u64,
        source: CatalogSource,
        durable_save_pending: bool,
        generation_fingerprint: Option<String>,
        publication_ack: Option<mpsc::Sender<()>>,
    },
    Persisted {
        summary: library_db::LibraryRefreshSummary,
        completed_build_seconds: Option<u64>,
        generation_fingerprint: Option<String>,
    },
    PersistenceFailed {
        error: String,
    },
    Unchanged {
        summary: library_db::LibraryRefreshSummary,
    },
    Changed {
        detail: String,
        reason: CatalogChangeReason,
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

#[derive(Default)]
struct CatalogProgressCoalescer {
    last_sent: Option<Instant>,
    last_phase: Option<library_db::CatalogProgressPhase>,
    last_title: String,
    last_detail: String,
    last_percent: i32,
}

impl CatalogProgressCoalescer {
    fn should_send(
        &mut self,
        phase: library_db::CatalogProgressPhase,
        title: &str,
        detail: &str,
        percent: i32,
    ) -> bool {
        let now = Instant::now();
        let immediate = self.last_sent.is_none()
            || self.last_phase != Some(phase)
            || self.last_title != title
            || self.last_percent != percent
            || percent >= 0;
        let elapsed = self
            .last_sent
            .map(|last| now.duration_since(last))
            .unwrap_or(Duration::MAX);
        if !immediate && elapsed < Duration::from_millis(250) {
            return false;
        }
        self.last_sent = Some(now);
        self.last_phase = Some(phase);
        self.last_title.clear();
        self.last_title.push_str(title);
        self.last_detail.clear();
        self.last_detail.push_str(detail);
        self.last_percent = percent;
        true
    }
}

fn send_catalog_progress_text(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    coalescer: &mut CatalogProgressCoalescer,
    title: &str,
    detail: &str,
) {
    let phase = library_db::CatalogProgressPhase::from_display_title(title);
    let percent = library_db::catalog_progress_percent_from_display(title, detail);
    if !coalescer.should_send(phase, title, detail, percent) {
        return;
    }
    let _ = tx.send(CatalogWorkerMessage::Progress {
        title: title.to_string(),
        detail: detail.to_string(),
        percent,
        metadata: None,
    });
}

fn send_catalog_progress(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    progress: library_db::CatalogProgress,
) {
    let display = progress.display();
    debug_assert_eq!(display.phase(), progress.phase());
    let _ = tx.send(CatalogWorkerMessage::Progress {
        title: display.title().to_string(),
        detail: display.detail().to_string(),
        percent: display.percent(),
        metadata: None,
    });
}

pub(super) fn catalog_load_timing_detail(loaded: &library_db::LibraryCatalogLoad) -> String {
    format!(
        "games={} rows={} total_us={} open_us={} schema_check_us={} query_us={} query_prepare_us={} query_first_row_us={} query_row_read_us={} query_row_hydrate_us={} launch_plans_us={} systems_us={} catalog_us={} navigation_file_read_us={} navigation_decompress_us={} navigation_decode_us={} {}",
        loaded.catalog.len(),
        loaded.rows,
        loaded.us,
        loaded.open_us,
        loaded.schema_check_us,
        loaded.query_us,
        loaded.query_prepare_us,
        loaded.query_first_row_us,
        loaded.query_row_read_us,
        loaded.query_row_hydrate_us,
        loaded.launch_plans_us,
        loaded.systems_us,
        loaded.catalog_us,
        loaded.navigation_file_read_us,
        loaded.navigation_decompress_us,
        loaded.navigation_decode_us,
        library_db::catalog_load_counter_detail()
    )
}

fn send_catalog_load_timing(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    name: &str,
    loaded: &library_db::LibraryCatalogLoad,
) {
    let _ = tx.send(CatalogWorkerMessage::Timing {
        name: name.to_string(),
        detail: catalog_load_timing_detail(loaded),
    });
}

fn repair_navigation_projection_cache_after_ready(
    root: &str,
    loaded_catalog: Option<&(ArcadeCatalog, catalog_stamp::CatalogStamp)>,
    fallback_repair_allowed: bool,
    sqlite_path: &Path,
    tx: &mpsc::Sender<CatalogWorkerMessage>,
) {
    if let Some((catalog, stamp)) = loaded_catalog {
        repair_navigation_projection_cache_for_catalog(catalog, stamp, sqlite_path, tx);
    } else if fallback_repair_allowed {
        repair_navigation_projection_cache(root, sqlite_path, tx);
    } else {
        let _ = tx.send(CatalogWorkerMessage::Timing {
            name: "catalog_navigation_repair_tsv".to_string(),
            detail: "status=skipped_degraded_joined_fallback".to_string(),
        });
    }
}

fn repair_navigation_projection_cache_for_catalog(
    catalog: &ArcadeCatalog,
    stamp: &catalog_stamp::CatalogStamp,
    sqlite_path: &Path,
    tx: &mpsc::Sender<CatalogWorkerMessage>,
) {
    let started = Instant::now();
    match library_db::catalog_projection_pair_current(sqlite_path, stamp) {
        Ok(true) => {
            match library_db::catalog_projection_filter_mismatches(sqlite_path, catalog, stamp) {
                Ok(mismatches) if mismatches.is_empty() => {
                    let _ = tx.send(CatalogWorkerMessage::Timing {
                        name: "catalog_navigation_repair_tsv".to_string(),
                        detail: format!(
                            "status=current elapsed_us={} {}",
                            started.elapsed().as_micros(),
                            catalog
                                .filter_option_count_detail(arcade_catalog::MENU_ARCADE_SYSTEM_ID)
                        ),
                    });
                    return;
                }
                Ok(mismatches) => {
                    let _ = tx.send(CatalogWorkerMessage::Timing {
                        name: "catalog_filter_parity_tsv".to_string(),
                        detail: format!(
                            "status=mismatch collections={} detail={}",
                            mismatches.len(),
                            mismatches.join(" | ")
                        ),
                    });
                }
                Err(e) => {
                    let _ = tx.send(CatalogWorkerMessage::Timing {
                        name: "catalog_filter_parity_tsv".to_string(),
                        detail: format!("status=check_failed error={e}"),
                    });
                }
            }
        }
        Ok(false) => {}
        Err(e) => {
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_navigation_repair_tsv".to_string(),
                detail: format!(
                    "status=projection_check_failed elapsed_us={} error={e}",
                    started.elapsed().as_micros()
                ),
            });
        }
    }

    let repair_t = Instant::now();
    if !loaded_catalog_stamp_still_current(&sqlite_path, stamp, started, tx) {
        return;
    }
    match library_db::repair_catalog_projections_for_catalog(sqlite_path, catalog, stamp) {
        Ok(()) => {
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_navigation_repair_tsv".to_string(),
                detail: format!(
                    "status=ok elapsed_us={} load_us=0 repair_us={} games={}",
                    started.elapsed().as_micros(),
                    repair_t.elapsed().as_micros(),
                    catalog.len()
                ),
            });
        }
        Err(e) => {
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_navigation_repair_tsv".to_string(),
                detail: format!(
                    "status=repair_failed elapsed_us={} load_us=0 repair_us={} error={e}",
                    started.elapsed().as_micros(),
                    repair_t.elapsed().as_micros()
                ),
            });
        }
    }
}

fn repair_navigation_projection_cache(
    root: &str,
    sqlite_path: &Path,
    tx: &mpsc::Sender<CatalogWorkerMessage>,
) {
    let started = Instant::now();
    let Some(stamp) = (match library_db::read_sqlite_catalog_stamp(sqlite_path) {
        Ok(stamp) => stamp,
        Err(e) => {
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_navigation_repair_tsv".to_string(),
                detail: format!(
                    "status=stamp_failed elapsed_us={} error={e}",
                    started.elapsed().as_micros()
                ),
            });
            return;
        }
    }) else {
        let _ = tx.send(CatalogWorkerMessage::Timing {
            name: "catalog_navigation_repair_tsv".to_string(),
            detail: format!(
                "status=missing_stamp elapsed_us={}",
                started.elapsed().as_micros()
            ),
        });
        return;
    };
    match library_db::catalog_projection_pair_current(sqlite_path, &stamp) {
        Ok(true) | Ok(false) => {}
        Err(e) => {
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_navigation_repair_tsv".to_string(),
                detail: format!(
                    "status=projection_check_failed elapsed_us={} error={e}",
                    started.elapsed().as_micros()
                ),
            });
        }
    }

    let load_t = Instant::now();
    let loaded =
        match library_db::load_arcade_catalog_from_materialized_sqlite_at(root, sqlite_path) {
            Ok(loaded) => loaded,
            Err(e) => {
                let _ = tx.send(CatalogWorkerMessage::Timing {
                    name: "catalog_navigation_repair_tsv".to_string(),
                    detail: format!(
                        "status=load_failed elapsed_us={} error={e}",
                        started.elapsed().as_micros()
                    ),
                });
                return;
            }
        };
    let load_us = load_t.elapsed().as_micros();
    if !loaded.projection_repair_safe {
        let _ = tx.send(CatalogWorkerMessage::Timing {
            name: "catalog_navigation_repair_tsv".to_string(),
            detail: format!(
                "status=skipped_degraded_joined_fallback elapsed_us={} load_us={load_us}",
                started.elapsed().as_micros()
            ),
        });
        return;
    }
    let Some(loaded_stamp) = loaded.stamp.as_ref() else {
        let _ = tx.send(CatalogWorkerMessage::Timing {
            name: "catalog_navigation_repair_tsv".to_string(),
            detail: format!(
                "status=missing_loaded_stamp elapsed_us={} load_us={}",
                started.elapsed().as_micros(),
                load_us
            ),
        });
        return;
    };
    if !loaded_catalog_stamp_still_current(sqlite_path, loaded_stamp, started, tx) {
        return;
    }
    let _ = tx.send(CatalogWorkerMessage::Timing {
        name: "catalog_materialized_filter_hydration_tsv".to_string(),
        detail: format!(
            "status=ok elapsed_us={} load_us={} {}",
            started.elapsed().as_micros(),
            load_us,
            loaded
                .catalog
                .filter_option_count_detail(arcade_catalog::MENU_ARCADE_SYSTEM_ID)
        ),
    });
    repair_navigation_projection_cache_for_catalog(&loaded.catalog, loaded_stamp, sqlite_path, tx);
}

fn loaded_catalog_stamp_still_current(
    sqlite_path: &std::path::Path,
    loaded_stamp: &catalog_stamp::CatalogStamp,
    started: Instant,
    tx: &mpsc::Sender<CatalogWorkerMessage>,
) -> bool {
    match library_db::read_sqlite_catalog_stamp(sqlite_path) {
        Ok(Some(current_stamp)) if &current_stamp == loaded_stamp => true,
        Ok(Some(current_stamp)) => {
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_navigation_repair_tsv".to_string(),
                detail: format!(
                    "status=stamp_changed elapsed_us={} loaded={} current={}",
                    started.elapsed().as_micros(),
                    loaded_stamp.fingerprint_hex(),
                    current_stamp.fingerprint_hex()
                ),
            });
            false
        }
        Ok(None) => {
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_navigation_repair_tsv".to_string(),
                detail: format!(
                    "status=missing_live_stamp elapsed_us={}",
                    started.elapsed().as_micros()
                ),
            });
            false
        }
        Err(e) => {
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_navigation_repair_tsv".to_string(),
                detail: format!(
                    "status=live_stamp_failed elapsed_us={} error={e}",
                    started.elapsed().as_micros()
                ),
            });
            false
        }
    }
}

fn load_navigation_projection_cache(
    root: &str,
    sqlite_path: &Path,
) -> Result<Option<library_db::LibraryCatalogLoad>, String> {
    let summary_path = catalog_summary::summary_path_for_sqlite(sqlite_path);
    let summary_stamp = catalog_summary::read_catalog_summary(&summary_path)?
        .map(|summary| catalog_stamp::CatalogStamp::from_lines(summary.catalog_stamp_lines));
    let stored_stamp = library_db::read_sqlite_catalog_stamp(sqlite_path)?;
    let Some(stamp) = navigation_projection_stamp(summary_stamp, stored_stamp) else {
        return Ok(None);
    };
    library_db::load_arcade_catalog_from_navigation_projection(root, sqlite_path, &stamp)
}

fn navigation_projection_stamp(
    summary_stamp: Option<catalog_stamp::CatalogStamp>,
    stored_stamp: Option<catalog_stamp::CatalogStamp>,
) -> Option<catalog_stamp::CatalogStamp> {
    let stored_stamp = stored_stamp?;
    match summary_stamp {
        Some(summary_stamp) if summary_stamp != stored_stamp => None,
        _ => Some(stored_stamp),
    }
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static POST_PERSIST_FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn post_persist_registry_fixture() -> (PathBuf, String) {
        use mister_magik_catalog::arcade_catalog::{ArcadeCatalog, GameSystemEntry};
        use mister_magik_catalog::catalog_checkpoint::CatalogDiscoveryCheckpoint;
        use mister_magik_catalog::catalog_state::{CatalogState, CatalogStateStats};
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let sequence = POST_PERSIST_FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let storage = std::env::temp_dir().join(format!(
            "mister-magik-post-persist-registry-{}-{nonce}-{sequence}",
            std::process::id(),
        ));
        let stamp = catalog_stamp::CatalogStamp::from_lines(vec!["post-persist-v1".into()]);
        let fingerprint = stamp.fingerprint_hex();
        let games = vec![
            crate::test_support::arcade_game("Arcade Game").build(),
            crate::test_support::arcade_game("Game Boy Game")
                .path("/media/fat/games/Gameboy/Game Boy Game.gb")
                .system_id("gb")
                .build(),
            crate::test_support::arcade_game("Game Boy Color Game")
                .path("/media/fat/games/Gameboy2/Game Boy Color Game.gbc")
                .system_id("gbc")
                .build(),
            crate::test_support::arcade_game("Lynx Game")
                .path("/media/fat/games/AtariLynx/Lynx Game.lnx")
                .system_id("atarilynx")
                .build(),
        ];
        let catalog = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            games,
            vec![
                GameSystemEntry {
                    id: "arcade".into(),
                    title: "Arcade".into(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "gb".into(),
                    title: "Game Boy".into(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "gbc".into(),
                    title: "Game Boy Color".into(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "atarilynx".into(),
                    title: "Atari Lynx".into(),
                    count: 1,
                },
            ],
        );
        mister_magik_catalog::production_sharded_projection::publish_bound_production_projection(
            &storage,
            &catalog,
            &fingerprint,
            mister_magik_catalog::production_sharded_projection::production_registry_limits(),
        )
        .expect("publish V3 fixture");
        mister_magik_catalog::catalog_state::write(
            &mister_magik_catalog::catalog_state::path_for_root(&storage),
            &CatalogState {
                stamp,
                checkpoint: CatalogDiscoveryCheckpoint::from_lines(vec!["fixture".into()]),
                stats: CatalogStateStats::default(),
            },
        )
        .expect("write V3 fixture state");
        (storage, fingerprint)
    }

    #[test]
    fn navigation_projection_uses_sqlite_stamp_when_summary_is_missing() {
        let stored = catalog_stamp::CatalogStamp::from_lines(vec!["catalog-v1".into()]);

        assert_eq!(
            navigation_projection_stamp(None, Some(stored.clone())),
            Some(stored)
        );
    }

    #[test]
    fn navigation_projection_rejects_summary_that_disagrees_with_sqlite() {
        let summary = catalog_stamp::CatalogStamp::from_lines(vec!["catalog-old".into()]);
        let stored = catalog_stamp::CatalogStamp::from_lines(vec!["catalog-current".into()]);

        assert_eq!(
            navigation_projection_stamp(Some(summary), Some(stored)),
            None
        );
    }

    #[test]
    fn ready_catalog_defers_search_index_to_the_launcher_session() {
        use mister_magik_catalog::arcade_catalog::{ArcadeGameEntry, GameSystemEntry};
        use std::sync::Arc;

        let catalog = ArcadeCatalog::new_with_deferred_text_indexes(
            PathBuf::from("/media/fat/_Arcade"),
            vec![ArcadeGameEntry {
                title: Arc::from("Street Fighter II"),
                mra_path: Arc::from("/media/fat/_Arcade/Street Fighter II.mra"),
                preview_archive_path: Arc::from(""),
                preview_asset_key: Arc::from(""),
                has_preview: false,
                system_id: Arc::from("arcade"),
                year: Some(1991),
                manufacturer: Arc::from("Capcom"),
                category: Arc::from("Fighter"),
                players: Some(2),
                control: Arc::from("joy"),
                is_new: false,
            }],
            vec![GameSystemEntry {
                id: "arcade".into(),
                title: "Arcade".into(),
                count: 1,
            }],
            Vec::new(),
        );
        assert!(!catalog.text_indexes_ready());
        let (tx, rx) = mpsc::channel();

        let worker = std::thread::spawn(move || {
            send_ready_catalog(
                &tx,
                catalog,
                None,
                42,
                CatalogSource::NavigationProjection,
                false,
                None,
            );
        });

        let delivered_catalog = match rx.recv().expect("ready catalog") {
            CatalogWorkerMessage::Ready {
                catalog,
                publication_ack,
                ..
            } => {
                publication_ack
                    .expect("publication acknowledgement")
                    .send(())
                    .expect("acknowledge catalog publication");
                catalog
            }
            _ => panic!("expected ready catalog"),
        };
        worker.join().expect("catalog worker");
        assert!(rx.recv().is_err(), "worker must not start search indexing");
        assert!(!delivered_catalog.text_indexes_ready());
    }

    #[test]
    fn strict_load_rehydrates_the_published_registry_without_validation() {
        let (storage, fingerprint) = post_persist_registry_fixture();
        let (tx, rx) = mpsc::channel();
        let worker_storage = storage.clone();
        let worker = std::thread::spawn(move || {
            publish_strict_registry_seed_at(&tx, "/media/fat/_Arcade", &worker_storage)
        });

        assert!(matches!(
            rx.recv().expect("strict load timing"),
            CatalogWorkerMessage::Timing { name, detail }
                if name == "catalog_strict_registry_load" && detail.contains("status=ready")
        ));
        let catalog = match rx.recv().expect("strict load catalog") {
            CatalogWorkerMessage::Ready {
                catalog,
                source,
                durable_save_pending,
                generation_fingerprint,
                publication_ack,
                ..
            } => {
                assert_eq!(source, CatalogSource::ShardedRegistry);
                assert!(!durable_save_pending);
                assert_eq!(
                    generation_fingerprint.as_deref(),
                    Some(fingerprint.as_str())
                );
                publication_ack
                    .expect("publication acknowledgement")
                    .send(())
                    .expect("acknowledge strict load catalog");
                catalog
            }
            _ => panic!("expected strict load catalog"),
        };
        worker
            .join()
            .expect("strict load worker")
            .expect("strict load registry");

        assert_eq!(catalog.systems.len(), 4);
        std::fs::remove_dir_all(storage).expect("remove registry fixture");
    }

    #[test]
    fn persisted_event_publishes_live_handheld_counts_before_completion() {
        let (storage, fingerprint) = post_persist_registry_fixture();
        let (tx, rx) = mpsc::channel();
        let worker_storage = storage.clone();
        let worker = std::thread::spawn(move || {
            let state_path = mister_magik_catalog::catalog_state::path_for_root(&worker_storage);
            send_persisted_catalog_at(
                &tx,
                "/media/fat/_Arcade",
                BuilderSummary::default(),
                &worker_storage,
                &state_path,
            );
        });

        assert!(matches!(
            rx.recv().expect("registry timing"),
            CatalogWorkerMessage::Timing { name, detail }
                if name == "catalog_post_persist_registry_load"
                    && detail.contains("status=ready")
        ));
        let catalog = match rx.recv().expect("registry catalog") {
            CatalogWorkerMessage::Ready {
                catalog,
                source,
                durable_save_pending,
                generation_fingerprint,
                publication_ack,
                ..
            } => {
                assert_eq!(source, CatalogSource::ShardedRegistry);
                assert!(durable_save_pending);
                assert_eq!(
                    generation_fingerprint.as_deref(),
                    Some(fingerprint.as_str())
                );
                publication_ack
                    .expect("publication acknowledgement")
                    .send(())
                    .expect("acknowledge registry catalog");
                catalog
            }
            _ => panic!("expected registry catalog"),
        };
        assert!(matches!(
            rx.recv().expect("persistence completion"),
            CatalogWorkerMessage::Persisted { generation_fingerprint, .. }
                if generation_fingerprint.as_deref() == Some(fingerprint.as_str())
        ));
        worker.join().expect("registry worker");

        assert_eq!(catalog.systems.len(), 4);
        assert_eq!(
            catalog
                .systems
                .iter()
                .find(|system| system.id == "gb")
                .unwrap()
                .count,
            1
        );
        assert_eq!(
            catalog
                .systems
                .iter()
                .find(|system| system.id == "gbc")
                .unwrap()
                .count,
            1
        );
        assert_eq!(
            catalog
                .systems
                .iter()
                .find(|system| system.id == "atarilynx")
                .unwrap()
                .count,
            1
        );
        assert!(
            catalog.games.is_empty(),
            "registry publication must not hydrate any system rows"
        );

        let mut nav = LauncherNav::new();
        nav.catalog_build_started();
        for system_id in ["gb", "gbc", "atarilynx"] {
            nav.catalog_system_discovered(system_id);
        }
        let bootstrap = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            catalog.games.iter().cloned().collect(),
            catalog
                .systems
                .iter()
                .filter(|system| system.id == "arcade")
                .cloned()
                .collect(),
        );
        let shell_catalog = nav.catalog_with_build_shells(bootstrap);
        nav.sync_launcher_taxonomy(&shell_catalog);
        assert!(nav.open_menu(crate::launcher_taxonomy::HANDHELDS_MENU_ID));
        assert!(nav.open_menu("menu:handhelds:nintendo"));
        assert_eq!(nav.current_menu_game_count(), 0);

        nav.sync_launcher_taxonomy(&catalog);
        assert_eq!(nav.current_menu_id(), "menu:handhelds:nintendo");
        assert_eq!(nav.current_menu_game_count(), 2);
        assert!(nav.open_menu(crate::launcher_taxonomy::HANDHELDS_MENU_ID));
        assert_eq!(nav.current_menu_game_count(), 3);
        assert!(
            nav.current_menu_items()
                .iter()
                .any(|item| item.title == "Nintendo" && item.count == 2)
        );
        assert!(
            nav.current_menu_items()
                .iter()
                .any(|item| item.title.contains("Lynx") && item.count == 1)
        );
        assert!(nav.open_system(&catalog, "gb"));
        assert!(super::super::launcher_bridge::active_system_games_loading(
            &catalog, &nav
        ));

        std::fs::remove_dir_all(storage).expect("remove registry fixture");
    }

    #[test]
    fn failed_post_persist_registry_hydration_is_nonfatal_and_publishes_no_catalog() {
        let storage = std::env::temp_dir().join(format!(
            "mister-magik-missing-post-persist-registry-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&storage);
        let (tx, rx) = mpsc::channel();

        assert_eq!(
            publish_persisted_registry_seed_at(&tx, "/media/fat/_Arcade", &storage),
            None
        );
        assert!(matches!(
            rx.recv().expect("failure timing"),
            CatalogWorkerMessage::Timing { name, detail }
                if name == "catalog_post_persist_registry_load"
                    && detail.contains("status=unavailable")
        ));
        assert!(
            rx.try_recv().is_err(),
            "failure must not publish a Ready catalog"
        );
    }

    #[test]
    fn catalog_worker_maps_plans_to_embedded_builder_operations() {
        assert_eq!(
            catalog_builder_operation(
                CatalogWorkerPlan::CheckStamp,
                CatalogExecutionMode::BackgroundInteractive
            ),
            Some((BuilderOperation::Check, "check"))
        );
        assert_eq!(
            catalog_builder_operation(
                CatalogWorkerPlan::InitialBuild,
                CatalogExecutionMode::ForegroundExclusive
            ),
            Some((BuilderOperation::Build, "build"))
        );
        assert_eq!(
            catalog_builder_operation(
                CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS,
                CatalogExecutionMode::ForegroundExclusive
            ),
            Some((BuilderOperation::Rebuild, "rebuild"))
        );
        assert_eq!(
            catalog_builder_operation(
                CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS,
                CatalogExecutionMode::BackgroundInteractive
            ),
            Some((BuilderOperation::Rebuild, "rebuild"))
        );
        assert_eq!(
            catalog_builder_operation(
                CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS,
                CatalogExecutionMode::BackgroundInteractive
            ),
            Some((BuilderOperation::RebuildAll, "rebuild-all"))
        );
        assert_eq!(
            catalog_builder_operation(
                CatalogWorkerPlan::FreshBuild,
                CatalogExecutionMode::ForegroundExclusive
            ),
            Some((BuilderOperation::FreshBuild, "fresh-build"))
        );
        assert_eq!(
            catalog_builder_operation(
                CatalogWorkerPlan::LoadOnly,
                CatalogExecutionMode::BackgroundInteractive
            ),
            None
        );
    }

    #[test]
    fn catalog_worker_dispatches_every_plan_without_changing_progress_semantics() {
        use CatalogExecutionMode::{BackgroundInteractive, ForegroundExclusive};
        let cases = [
            (
                CatalogWorkerPlan::LoadOnly,
                BackgroundInteractive,
                false,
                true,
                false,
                None,
                false,
            ),
            (
                CatalogWorkerPlan::CheckStamp,
                BackgroundInteractive,
                false,
                false,
                false,
                Some(BackgroundInteractive),
                false,
            ),
            (
                CatalogWorkerPlan::CheckStamp,
                BackgroundInteractive,
                true,
                false,
                true,
                None,
                false,
            ),
            (
                CatalogWorkerPlan::InitialBuild,
                ForegroundExclusive,
                false,
                false,
                false,
                Some(ForegroundExclusive),
                true,
            ),
            (
                CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS,
                ForegroundExclusive,
                false,
                false,
                false,
                Some(ForegroundExclusive),
                true,
            ),
            (
                CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS,
                BackgroundInteractive,
                false,
                false,
                false,
                Some(BackgroundInteractive),
                true,
            ),
            (
                CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS,
                BackgroundInteractive,
                false,
                false,
                false,
                Some(BackgroundInteractive),
                true,
            ),
            (
                CatalogWorkerPlan::FreshBuild,
                ForegroundExclusive,
                false,
                false,
                false,
                Some(ForegroundExclusive),
                false,
            ),
        ];

        for (
            plan,
            execution_mode,
            cached_catalog_published,
            completes_without_builder,
            deferred_cached_validation,
            builder_execution_mode,
            sends_initial_progress,
        ) in cases
        {
            assert_eq!(
                catalog_worker_dispatch(plan, execution_mode, cached_catalog_published),
                CatalogWorkerDispatch {
                    completes_without_builder,
                    deferred_cached_validation,
                    builder_execution_mode,
                    sends_initial_progress,
                },
                "{}",
                plan.label()
            );
        }
    }

    #[test]
    fn embedded_builder_events_translate_without_a_process_boundary() {
        let protocol = mister_magik_catalog::builder_protocol::CATALOG_BUILDER_PROTOCOL_VERSION;
        let (tx, rx) = mpsc::channel();
        let mut coalescer = CatalogProgressCoalescer::default();
        let mut state = EmbeddedBuilderEventState::default();
        let mut emit = |event| {
            handle_embedded_builder_event(
                "/tmp",
                CatalogWorkerPlan::InitialBuild,
                "build",
                event,
                &tx,
                &mut coalescer,
                &mut state,
            );
        };

        emit(CatalogBuilderEvent::Handshake {
            protocol,
            operation: "build".into(),
            run_id: "test".into(),
        });
        emit(CatalogBuilderEvent::Progress {
            protocol,
            title: "Scanning library".into(),
            detail: "Scanning 1 folder".into(),
            metadata: None,
        });
        assert!(matches!(
            rx.recv().unwrap(),
            CatalogWorkerMessage::Progress { .. }
        ));

        emit(CatalogBuilderEvent::SystemDiscovered {
            protocol,
            system_id: "arcade".into(),
        });
        assert!(matches!(
            rx.recv().unwrap(),
            CatalogWorkerMessage::SystemDiscovered { system_id } if system_id == "arcade"
        ));
        emit(CatalogBuilderEvent::Timing {
            protocol,
            name: "scan".into(),
            detail: "elapsed_us=1".into(),
        });
        assert!(matches!(
            rx.recv().unwrap(),
            CatalogWorkerMessage::Timing { name, .. } if name == "scan"
        ));
        emit(CatalogBuilderEvent::FreshCleanupStarted { protocol });
        assert!(matches!(
            rx.recv().unwrap(),
            CatalogWorkerMessage::FreshCleanupStarted
        ));
        emit(CatalogBuilderEvent::FreshCleanupCompleted {
            protocol,
            removed: 3,
        });
        assert!(matches!(
            rx.recv().unwrap(),
            CatalogWorkerMessage::FreshCleanupCompleted { removed: 3 }
        ));
        emit(CatalogBuilderEvent::CatalogReady {
            protocol,
            snapshot_path: "/tmp/missing-catalog-builder-test.nav.lz4b".into(),
            games: 0,
            load_us: 2,
        });
        assert!(matches!(
            rx.recv().unwrap(),
            CatalogWorkerMessage::LoadFailed { .. }
        ));
        emit(CatalogBuilderEvent::Persisted {
            protocol,
            summary: BuilderSummary::default(),
        });
        assert!(matches!(
            rx.recv().unwrap(),
            CatalogWorkerMessage::Timing { name, detail }
                if name == "catalog_post_persist_registry_load"
                    && detail.contains("status=unavailable")
        ));
        assert!(matches!(
            rx.recv().unwrap(),
            CatalogWorkerMessage::Persisted { .. }
        ));
        emit(CatalogBuilderEvent::Unchanged {
            protocol,
            summary: BuilderSummary::default(),
        });
        assert!(matches!(
            rx.recv().unwrap(),
            CatalogWorkerMessage::Unchanged { .. }
        ));
        emit(CatalogBuilderEvent::Changed {
            protocol,
            detail: "changed".into(),
            reason: Some(CatalogChangeReason::InputsChanged),
        });
        assert!(matches!(
            rx.recv().unwrap(),
            CatalogWorkerMessage::Changed { detail, .. } if detail == "changed"
        ));
        emit(CatalogBuilderEvent::Done { protocol });
        assert!(matches!(rx.recv().unwrap(), CatalogWorkerMessage::Done));
        assert!(state.handshake_seen && state.terminal_seen);

        let mut failure_state = EmbeddedBuilderEventState::default();
        handle_embedded_builder_event(
            "/tmp",
            CatalogWorkerPlan::FreshBuild,
            "fresh-build",
            CatalogBuilderEvent::Handshake {
                protocol,
                operation: "fresh-build".into(),
                run_id: "failure-test".into(),
            },
            &tx,
            &mut CatalogProgressCoalescer::default(),
            &mut failure_state,
        );
        handle_embedded_builder_event(
            "/tmp",
            CatalogWorkerPlan::FreshBuild,
            "fresh-build",
            CatalogBuilderEvent::Failure {
                protocol,
                stage: "lock".into(),
                error: "busy".into(),
                diagnostic: None,
            },
            &tx,
            &mut CatalogProgressCoalescer::default(),
            &mut failure_state,
        );
        assert!(matches!(
            rx.recv().unwrap(),
            CatalogWorkerMessage::LoadFailed { .. }
        ));
        assert!(failure_state.terminal_seen);
    }

    #[test]
    fn warm_rebuild_never_publishes_a_bootstrap_catalog() {
        let protocol = mister_magik_catalog::builder_protocol::CATALOG_BUILDER_PROTOCOL_VERSION;
        let (tx, rx) = mpsc::channel();
        let mut state = EmbeddedBuilderEventState {
            handshake_seen: true,
            ..EmbeddedBuilderEventState::default()
        };
        handle_embedded_builder_event(
            "/media/fat",
            CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS,
            "rebuild",
            CatalogBuilderEvent::CatalogReady {
                protocol,
                snapshot_path: "/tmp/ignored.nav.lz4b".into(),
                games: 1,
                load_us: 7,
            },
            &tx,
            &mut CatalogProgressCoalescer::default(),
            &mut state,
        );

        assert!(matches!(
            rx.recv().expect("warm bootstrap diagnostic"),
            CatalogWorkerMessage::Timing { name, .. }
                if name == "catalog_warm_bootstrap_ignored"
        ));
        assert!(!state.catalog_ready_seen);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn full_reconciliation_keeps_the_all_system_plan_authoritative() {
        let protocol = mister_magik_catalog::builder_protocol::CATALOG_BUILDER_PROTOCOL_VERSION;
        let (tx, rx) = mpsc::channel();
        let mut state = EmbeddedBuilderEventState {
            handshake_seen: true,
            ..EmbeddedBuilderEventState::default()
        };

        handle_embedded_builder_event(
            "/media/fat",
            CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS,
            "rebuild-all",
            CatalogBuilderEvent::PlanReady {
                protocol,
                system_ids: vec!["amiga".into()],
                all_published_systems: false,
                systems: Vec::new(),
            },
            &tx,
            &mut CatalogProgressCoalescer::default(),
            &mut state,
        );

        assert!(matches!(
            rx.recv().expect("full reconcile plan"),
            CatalogWorkerMessage::ReconciliationPlanReady {
                system_ids,
                all_published_systems: true,
            } if system_ids.is_empty()
        ));
    }

    #[test]
    fn embedded_catalog_ready_publishes_before_post_ready_failure() {
        let protocol = mister_magik_catalog::builder_protocol::CATALOG_BUILDER_PROTOCOL_VERSION;
        let snapshot_path = std::env::temp_dir().join(format!(
            "mister-magik-embedded-ready-{}.nav.lz4b",
            std::process::id()
        ));
        let catalog = ArcadeCatalog::new_with_deferred_text_indexes(
            PathBuf::from("/media/fat"),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        mister_magik_catalog::catalog_navigation::write_catalog_navigation_snapshot(
            &snapshot_path,
            &catalog,
            &catalog_stamp::CatalogStamp::from_lines(vec!["test".into()]),
        )
        .expect("write catalog snapshot");

        let (tx, rx) = mpsc::channel();
        let snapshot = snapshot_path.to_string_lossy().into_owned();
        let worker = std::thread::spawn(move || {
            let mut coalescer = CatalogProgressCoalescer::default();
            let mut state = EmbeddedBuilderEventState::default();
            for event in [
                CatalogBuilderEvent::Handshake {
                    protocol,
                    operation: "fresh-build".into(),
                    run_id: "ready-test".into(),
                },
                CatalogBuilderEvent::CatalogReady {
                    protocol,
                    snapshot_path: snapshot,
                    games: 0,
                    load_us: 7,
                },
                CatalogBuilderEvent::Failure {
                    protocol,
                    stage: "persist".into(),
                    error: "disk full".into(),
                    diagnostic: None,
                },
            ] {
                handle_embedded_builder_event(
                    "/media/fat",
                    CatalogWorkerPlan::FreshBuild,
                    "fresh-build",
                    event,
                    &tx,
                    &mut coalescer,
                    &mut state,
                );
            }
            state
        });

        match rx.recv().expect("ready catalog") {
            CatalogWorkerMessage::Ready {
                load_us,
                source,
                durable_save_pending,
                publication_ack,
                ..
            } => {
                assert_eq!(load_us, 7);
                assert_eq!(source, CatalogSource::FreshBuild);
                assert!(durable_save_pending);
                publication_ack
                    .expect("publication acknowledgement")
                    .send(())
                    .expect("acknowledge publication");
            }
            _ => panic!("expected ready catalog"),
        }
        assert!(matches!(
            rx.recv().expect("post-ready failure"),
            CatalogWorkerMessage::PersistenceFailed { .. }
        ));
        let state = worker.join().expect("embedded builder event worker");
        assert!(state.handshake_seen && state.catalog_ready_seen && state.terminal_seen);
        std::fs::remove_file(snapshot_path).expect("remove catalog snapshot");
    }

    #[test]
    fn catalog_worker_uses_cached_database_without_refresh() {
        let plan = catalog_worker_plan(CatalogCacheState::Ready, CatalogWorkerRequest::LoadOnly);
        assert_eq!(plan, CatalogWorkerPlan::LoadOnly);
    }

    #[test]
    fn catalog_worker_checks_ready_cache_stamp_when_requested() {
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Ready, CatalogWorkerRequest::CheckStamp,),
            CatalogWorkerPlan::CheckStamp
        );
    }

    #[test]
    fn catalog_worker_rebuilds_missing_or_empty_cache_without_refresh() {
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Missing, CatalogWorkerRequest::CheckStamp,),
            CatalogWorkerPlan::InitialBuild
        );
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Empty, CatalogWorkerRequest::CheckStamp),
            CatalogWorkerPlan::InitialBuild
        );
    }

    #[test]
    fn catalog_schema_mismatch_is_classified_for_automatic_rebuild() {
        assert!(library_db::is_catalog_schema_mismatch_error(
            "catalog schema mismatch: expected 34, found 33"
        ));
        assert!(library_db::is_catalog_schema_mismatch_error(
            "catalog schema mismatch: expected 34, found missing"
        ));
        assert!(!library_db::is_catalog_schema_mismatch_error(
            "open catalog database: input/output error"
        ));
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Missing, CatalogWorkerRequest::LoadOnly),
            CatalogWorkerPlan::InitialBuild
        );
    }

    #[test]
    fn strict_retry_never_falls_through_to_build() {
        for state in [
            CatalogCacheState::Ready,
            CatalogCacheState::Empty,
            CatalogCacheState::Missing,
        ] {
            assert_eq!(
                catalog_worker_plan(state, CatalogWorkerRequest::StrictLoad),
                CatalogWorkerPlan::LoadOnly
            );
        }
    }

    #[test]
    fn fresh_rebuild_uses_distinct_destructive_plan() {
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Ready, CatalogWorkerRequest::FreshBuild),
            CatalogWorkerPlan::FreshBuild
        );
    }

    #[test]
    fn full_warm_rebuild_requires_a_published_catalog() {
        assert_eq!(
            catalog_worker_plan(
                CatalogCacheState::Ready,
                CatalogWorkerRequest::RECONCILE_ALL_SYSTEMS,
            ),
            CatalogWorkerPlan::RECONCILE_ALL_SYSTEMS
        );
        for state in [CatalogCacheState::Empty, CatalogCacheState::Missing] {
            assert_eq!(
                catalog_worker_plan(state, CatalogWorkerRequest::RECONCILE_ALL_SYSTEMS),
                CatalogWorkerPlan::InitialBuild
            );
        }
    }

    #[test]
    fn catalog_worker_refreshes_only_when_requested() {
        assert_eq!(
            catalog_worker_plan(
                CatalogCacheState::Ready,
                CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
            ),
            CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS
        );
        for state in [CatalogCacheState::Empty, CatalogCacheState::Missing] {
            assert_eq!(
                catalog_worker_plan(state, CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS),
                CatalogWorkerPlan::RECONCILE_CHANGED_INPUTS,
                "an explicit rebuild remains a rebuild request"
            );
        }
    }

    #[test]
    fn execution_mode_selects_worker_thread_policy() {
        assert_eq!(
            CatalogExecutionMode::ForegroundExclusive.thread_role(),
            RuntimeThreadRole::CatalogForeground
        );
        assert_eq!(
            CatalogExecutionMode::BackgroundInteractive.thread_role(),
            RuntimeThreadRole::CatalogWorker
        );
    }

    #[test]
    fn catalog_progress_coalescer_throttles_repeated_scan_counts() {
        let mut coalescer = CatalogProgressCoalescer::default();
        assert!(coalescer.should_send(
            library_db::CatalogProgressPhase::ClassifyingLibrary,
            "Classifying library",
            "Classifying 1 games",
            -1
        ));
        assert!(!coalescer.should_send(
            library_db::CatalogProgressPhase::ClassifyingLibrary,
            "Classifying library",
            "Classifying 1 games",
            -1
        ));
        coalescer.last_sent = Some(Instant::now() - Duration::from_millis(300));
        assert!(coalescer.should_send(
            library_db::CatalogProgressPhase::ClassifyingLibrary,
            "Classifying library",
            "Classifying 1 games",
            -1
        ));
    }

    #[test]
    fn catalog_progress_coalescer_sends_phase_and_percent_changes() {
        let mut coalescer = CatalogProgressCoalescer::default();
        assert!(coalescer.should_send(
            library_db::CatalogProgressPhase::ClassifyingLibrary,
            "Classifying library",
            "Classifying 1 games",
            -1
        ));
        assert!(coalescer.should_send(
            library_db::CatalogProgressPhase::IndexingLibrary,
            "Indexing library",
            "Resolving playable games — 1 of 2",
            90
        ));
        assert!(coalescer.should_send(
            library_db::CatalogProgressPhase::LoadingLibrary,
            "Loading library",
            "Opening library — 2 games",
            100
        ));
    }

    #[test]
    fn catalog_progress_coalescer_throttles_changed_counter_details_but_keeps_heartbeats() {
        let mut coalescer = CatalogProgressCoalescer::default();
        assert!(coalescer.should_send(
            library_db::CatalogProgressPhase::IndexingLibrary,
            "Indexing library",
            "Resolving playable games — 250 of 500",
            -1,
        ));
        assert!(!coalescer.should_send(
            library_db::CatalogProgressPhase::IndexingLibrary,
            "Indexing library",
            "Resolving playable games — 251 of 500",
            -1,
        ));
        coalescer.last_sent = Some(Instant::now() - Duration::from_secs(1));
        assert!(coalescer.should_send(
            library_db::CatalogProgressPhase::IndexingLibrary,
            "Indexing library",
            "Resolving playable games — 250 of 500 — Still working… 1s",
            -1,
        ));
    }

    #[test]
    fn catalog_scan_percent_tracks_sqlite_import_progress() {
        assert_eq!(
            library_db::catalog_progress_percent_from_display(
                "Saving library",
                "Writing 0 of 100 games into SQLite..."
            ),
            90
        );
        assert_eq!(
            library_db::catalog_progress_percent_from_display(
                "Saving library",
                "Writing 50 of 100 games into SQLite..."
            ),
            94
        );
        assert_eq!(
            library_db::catalog_progress_percent_from_display(
                "Saving library",
                "Writing 100 of 100 games into SQLite..."
            ),
            99
        );
        assert_eq!(
            library_db::catalog_progress_percent_from_display(
                "Saving library",
                "Finalizing catalog views and search indexes..."
            ),
            99
        );
    }

    #[test]
    fn catalog_scan_percent_tracks_sqlite_save_progress() {
        assert_eq!(
            library_db::catalog_progress_percent_from_display(
                "Saving library",
                "Saving 0 of 1000 bytes to disk..."
            ),
            0
        );
        assert_eq!(
            library_db::catalog_progress_percent_from_display(
                "Saving library",
                "Saving 500 of 1000 bytes to disk..."
            ),
            50
        );
        assert_eq!(
            library_db::catalog_progress_percent_from_display(
                "Saving library",
                "Saving 1000 of 1000 bytes to disk..."
            ),
            100
        );
        assert_eq!(
            library_db::catalog_progress_percent_from_display(
                "Saving library",
                "Saving 1200 of 1000 bytes to disk..."
            ),
            100
        );
    }
}
