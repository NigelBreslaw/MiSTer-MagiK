// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use mister_magik_catalog::builder_protocol::{
    BuilderSummary, CatalogBuilderEvent, CATALOG_BUILDER_PROTOCOL_VERSION,
};
use mister_magik_catalog::runtime_thread::{apply_runtime_thread_policy, RuntimeThreadRole};
use mister_magik_catalog::{arcade_catalog::ArcadeCatalog, catalog_stamp, catalog_summary};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader};
use std::os::fd::AsRawFd;
use std::process::{Command, Stdio};

fn send_ready_catalog(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    catalog: ArcadeCatalog,
    summary: Option<library_db::LibraryRefreshSummary>,
    load_us: u64,
    source: CatalogSource,
    durable_save_pending: bool,
) {
    let (publication_tx, publication_rx) = mpsc::channel();
    let _ = tx.send(CatalogWorkerMessage::Ready {
        catalog,
        summary,
        load_us,
        source,
        durable_save_pending,
        publication_ack: Some(publication_tx),
    });
    // The launcher session owns the next transition. For a fresh catalog it
    // schedules indexing only after the durable Persisted event; the worker
    // must therefore return immediately to its SQLite/sidecar save path.
    let _ = publication_rx.recv();
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
) -> mpsc::Receiver<CatalogWorkerMessage> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("library-catalog".to_string())
        .spawn(move || {
            apply_runtime_thread_policy(execution_mode.thread_role());
            let progress_tx = tx.clone();
            let mut progress_coalescer = CatalogProgressCoalescer::default();
            let mut progress = move |title: &str, detail: &str| {
                let phase = library_db::CatalogProgressPhase::from_display_title(title);
                let percent = library_db::catalog_progress_percent_from_display(title, detail);
                if !progress_coalescer.should_send(phase, title, percent) {
                    return;
                }
                let _ = progress_tx.send(CatalogWorkerMessage::Progress {
                    title: title.to_string(),
                    detail: detail.to_string(),
                    percent,
                });
            };
            let scan_event_tx = tx.clone();
            let mut scan_events = move |event: library_db::LibraryScanEvent| match event {
                library_db::LibraryScanEvent::SystemDiscovered { system_id } => {
                    let _ = scan_event_tx.send(CatalogWorkerMessage::SystemDiscovered {
                        system_id,
                    });
                }
            };
            let mut cache_state = CatalogCacheState::Missing;
            let mut cached_catalog_published = false;
            let mut projection_repair_allowed = true;
            let mut projection_repair_catalog: Option<(ArcadeCatalog, catalog_stamp::CatalogStamp)> =
                None;
            match initial_cache {
                CatalogWorkerInitialCache::ProbeNavigationThenSqlite => {
                    match load_navigation_projection_cache(&root) {
                        Ok(Some(loaded)) => {
                            send_catalog_load_timing(
                                &tx,
                                "catalog_worker_navigation_load",
                                &loaded,
                            );
                            cache_state = CatalogCacheState::Ready;
                            send_ready_catalog(
                                &tx,
                                loaded.catalog,
                                None,
                                loaded.us,
                                CatalogSource::NavigationProjection,
                                false,
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
                        }
                        Err(e) => {
                            crate::ui_errln!("library navigation projection load failed: {e}");
                            let _ = tx.send(CatalogWorkerMessage::Timing {
                                name: "catalog_worker_navigation_load_failed".to_string(),
                                detail: format!("{e} {}", library_db::catalog_load_counter_detail()),
                            });
                        }
                    }
                    if !cache_state.has_usable_catalog() {
                        library_db::record_catalog_worker_cache_load();
                        match library_db::load_arcade_catalog_from_sqlite(&root) {
                            Ok(loaded) => {
                                send_catalog_load_timing(&tx, "catalog_worker_cache_load", &loaded);
                                if loaded.catalog.games.is_empty() {
                                    cache_state = CatalogCacheState::Empty;
                                } else {
                                    cache_state = CatalogCacheState::Ready;
                                    let catalog_for_repair = loaded.catalog.clone();
                                    send_ready_catalog(
                                        &tx,
                                        loaded.catalog,
                                        None,
                                        loaded.us,
                                        CatalogSource::FullSqlite,
                                        false,
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
                                let _ = tx.send(CatalogWorkerMessage::LoadFailed { error: e });
                                return;
                            }
                        }
                    }
                }
                CatalogWorkerInitialCache::ProbeSqlite => {
                    library_db::record_catalog_worker_cache_load();
                    match library_db::load_arcade_catalog_from_sqlite(&root) {
                        Ok(loaded) => {
                            send_catalog_load_timing(&tx, "catalog_worker_cache_load", &loaded);
                            if loaded.catalog.games.is_empty() {
                                cache_state = CatalogCacheState::Empty;
                            } else {
                                cache_state = CatalogCacheState::Ready;
                                let catalog_for_repair = loaded.catalog.clone();
                                send_ready_catalog(
                                    &tx,
                                    loaded.catalog,
                                    None,
                                    loaded.us,
                                    CatalogSource::FullSqlite,
                                    false,
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
                            let _ = tx.send(CatalogWorkerMessage::LoadFailed { error: e });
                            return;
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
                CatalogWorkerInitialCache::AlreadyProbedEmpty => {
                    cache_state = CatalogCacheState::Empty;
                    let _ = tx.send(CatalogWorkerMessage::Timing {
                        name: "catalog_worker_initial_cache".to_string(),
                        detail: "source=ui_probe state=empty".to_string(),
                    });
                }
            }
            if request == CatalogWorkerRequest::StrictLoad && !cache_state.has_usable_catalog() {
                let _ = tx.send(CatalogWorkerMessage::LoadFailed {
                    error: "catalog is empty".to_string(),
                });
                return;
            }
            let plan = catalog_worker_plan(cache_state, request);
            let foreground_exclusive =
                matches!(plan, CatalogWorkerPlan::ForceBuild | CatalogWorkerPlan::FreshBuild)
                    && execution_mode == CatalogExecutionMode::ForegroundExclusive;
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
            if cached_catalog_published && plan == CatalogWorkerPlan::CheckStamp {
                let _ = tx.send(CatalogWorkerMessage::HydrationDoneNeedsValidation {
                    root,
                });
                return;
            }
            match plan {
                CatalogWorkerPlan::LoadOnly => {
                    let _ = tx.send(CatalogWorkerMessage::Done);
                    repair_navigation_projection_cache_after_ready(
                        &root,
                        projection_repair_catalog.as_ref(),
                        projection_repair_allowed,
                        &tx,
                    );
                    return;
                }
                CatalogWorkerPlan::CheckStamp => {}
                CatalogWorkerPlan::ForceBuild => {
                    send_catalog_progress(
                        &tx,
                        library_db::CatalogProgress::indexing_building_catalog(),
                    );
                }
                CatalogWorkerPlan::FreshBuild => {}
            }
            if matches!(
                plan,
                CatalogWorkerPlan::CheckStamp
                    | CatalogWorkerPlan::ForceBuild
                    | CatalogWorkerPlan::FreshBuild
            ) {
                run_catalog_builder_subprocess(&root, plan, execution_mode, &tx);
                return;
            }
            if plan == CatalogWorkerPlan::ForceBuild {
                if foreground_exclusive {
                    let ram_artifact_result = library_db::scan_default_library_ram_foreground_with_events(
                        Some(&mut progress),
                        Some(&mut scan_events),
                    );
                    let ram_artifact = match ram_artifact_result {
                        Ok(artifact) => artifact,
                        Err(e) => {
                            crate::ui_errln!("library scan failed: {e}");
                            send_catalog_progress(
                                &tx,
                                library_db::CatalogProgress::library_scan_failed(e),
                            );
                            return;
                        }
                    };
                    let stats = ram_artifact.stats().clone();
                    let _ = tx.send(CatalogWorkerMessage::Timing {
                        name: "library_scan_complete".to_string(),
                        detail: format!(
                            "scan_us={} discover_us={} classify_us={} discoveries={} normal_files={} containers={} entries={}",
                            stats.scan_us,
                            stats.discover_us,
                            stats.classify_us,
                            stats.discoveries,
                            stats.normal_files,
                            stats.containers,
                            stats.entries
                        ),
                    });
                    let catalog_t = Instant::now();
                    let catalog = ram_artifact.catalog(&root);
                    let load_us = catalog_t.elapsed().as_micros() as u64;
                    let catalog_len = catalog.len();
                    let projection_catalog = catalog.clone();
                    send_ready_catalog(
                        &tx,
                        catalog,
                        None,
                        load_us,
                        CatalogSource::FreshBuild,
                        true,
                    );
                    apply_runtime_thread_policy(RuntimeThreadRole::CatalogWorker);
                    let _ = tx.send(CatalogWorkerMessage::Timing {
                        name: "catalog_execution_mode_transition".to_string(),
                        detail: "from=foreground_exclusive to=background_interactive reason=library_ready"
                            .to_string(),
                    });
                    let _ = tx.send(CatalogWorkerMessage::Timing {
                        name: "catalog_worker_ram_catalog".to_string(),
                        detail: format!("games={catalog_len} catalog_us={load_us}"),
                    });
                    send_catalog_progress(
                        &tx,
                        library_db::CatalogProgress::saving_before_opening_launcher(),
                    );
                    match ram_artifact.save_default_sqlite_with_catalog_projection(
                        &projection_catalog,
                        Some(&mut progress),
                    ) {
                        Ok(summary) => {
                            let _ = tx.send(CatalogWorkerMessage::Persisted {
                                summary: summary.clone(),
                                completed_build_seconds: None,
                            });
                            let _ = tx.send(CatalogWorkerMessage::Timing {
                                name: "catalog_worker_saved_catalog".to_string(),
                                detail: format!(
                                    "games={catalog_len} skipped=precomputed_projection"
                                ),
                            });
                        }
                        Err(e) => {
                            crate::ui_errln!("library persistence failed after RAM catalog ready: {e}");
                            let _ = tx.send(CatalogWorkerMessage::PersistenceFailed { error: e });
                        }
                    }
                    return;
                }
                let artifact_result = library_db::scan_default_library_with_events(
                        Some(&mut progress),
                        Some(&mut scan_events),
                    );
                let artifact = match artifact_result {
                    Ok(artifact) => artifact,
                    Err(e) => {
                        crate::ui_errln!("library scan failed: {e}");
                        send_catalog_progress(
                            &tx,
                            library_db::CatalogProgress::library_scan_failed(e),
                        );
                        return;
                    }
                };
                let stats = artifact.stats().clone();
                let _ = tx.send(CatalogWorkerMessage::Timing {
                    name: "library_scan_complete".to_string(),
                    detail: format!(
                        "scan_us={} discover_us={} classify_us={} discoveries={} normal_files={} containers={} entries={}",
                        stats.scan_us,
                        stats.discover_us,
                        stats.classify_us,
                        stats.discoveries,
                        stats.normal_files,
                        stats.containers,
                        stats.entries
                    ),
                });
                let catalog_t = Instant::now();
                let catalog = artifact.catalog(&root);
                let load_us = catalog_t.elapsed().as_micros() as u64;
                let catalog_len = catalog.len();
                send_ready_catalog(
                    &tx,
                    catalog,
                    None,
                    load_us,
                    CatalogSource::FreshBuild,
                    true,
                );
                let _ = tx.send(CatalogWorkerMessage::Timing {
                    name: "catalog_worker_ram_catalog".to_string(),
                    detail: format!(
                        "games={catalog_len} catalog_us={load_us}"
                    ),
                });
                send_catalog_progress(
                    &tx,
                    library_db::CatalogProgress::saving_before_opening_launcher(),
                );
                match artifact.save_default_sqlite_with_projections(&root, Some(&mut progress)) {
                    Ok(summary) => {
                        let _ = tx.send(CatalogWorkerMessage::Persisted {
                            summary: summary.clone(),
                            completed_build_seconds: None,
                        });
                        let _ = tx.send(CatalogWorkerMessage::Timing {
                            name: "catalog_worker_saved_catalog".to_string(),
                            detail: format!(
                                "games={catalog_len} skipped=precomputed_projection"
                            ),
                        });
                    }
                    Err(e) => {
                        crate::ui_errln!("library persistence failed after RAM catalog ready: {e}");
                        let _ = tx.send(CatalogWorkerMessage::PersistenceFailed { error: e });
                    }
                }
                return;
            }
            if plan == CatalogWorkerPlan::CheckStamp {
                match library_db::default_sqlite_catalog_stamp_check() {
                    Ok(check) => {
                        let _ = tx.send(CatalogWorkerMessage::Timing {
                            name: "catalog_stamp_check".to_string(),
                            detail: format!(
                                "unchanged={} check_us={} compute_us={} open_us={} read_us={} checkpoint_read_us={} compare_us={} checkpoint_compare_us={} stored={} current={} stored_checkpoint={} current_checkpoint={} stored_lines={} current_lines={} stored_checkpoint_lines={} current_checkpoint_lines={} drift_detail={}",
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
                                check.stored_checkpoint_fingerprint.as_deref().unwrap_or("missing"),
                                check.current_checkpoint_fingerprint,
                                check.stored_lines,
                                check.current_lines,
                                check.stored_checkpoint_lines,
                                check.current_checkpoint_lines,
                                check.drift.detail
                            ),
                        });
                        if check.unchanged {
                            match library_db::default_sqlite_cached_summary(check.check_us) {
                                Ok(summary) => {
                                    let _ = tx.send(CatalogWorkerMessage::Unchanged { summary });
                                    repair_navigation_projection_cache_after_ready(
                                        &root,
                                        projection_repair_catalog.as_ref(),
                                        projection_repair_allowed,
                                        &tx,
                                    );
                                    return;
                                }
                                Err(e) => {
                                    let _ = tx.send(CatalogWorkerMessage::Timing {
                                        name: "catalog_cached_summary_failed".to_string(),
                                        detail: e,
                                    });
                                    let _ = tx.send(CatalogWorkerMessage::Changed {
                                        detail:
                                            "Catalog summary unavailable; rebuild required."
                                                .to_string(),
                                    });
                                    return;
                                }
                            }
                        }
                        let _ = tx.send(CatalogWorkerMessage::Changed {
                            detail: format!(
                                "Catalog inputs changed; rebuild required. {}",
                                check.drift.detail
                            ),
                        });
                        return;
                    }
                    Err(e) => {
                        let _ = tx.send(CatalogWorkerMessage::Timing {
                            name: "catalog_stamp_check_failed".to_string(),
                            detail: e,
                        });
                        let _ = tx.send(CatalogWorkerMessage::Changed {
                            detail: "Catalog stamp check failed; rebuild required.".to_string(),
                        });
                        return;
                    }
                }
            }
        })
        .expect("spawn library-catalog");
    rx
}

fn run_catalog_builder_subprocess(
    root: &str,
    plan: CatalogWorkerPlan,
    execution_mode: CatalogExecutionMode,
    tx: &mpsc::Sender<CatalogWorkerMessage>,
) {
    let binary = std::env::var("MISTER_CATALOG_BUILDER_BIN")
        .unwrap_or_else(|_| "/media/fat/mister-magik/mister-magik-catalog-builder".into());
    let operation = match plan {
        CatalogWorkerPlan::CheckStamp => "check",
        CatalogWorkerPlan::ForceBuild
            if execution_mode == CatalogExecutionMode::ForegroundExclusive =>
        {
            "build"
        }
        CatalogWorkerPlan::ForceBuild => "rebuild",
        CatalogWorkerPlan::FreshBuild => "fresh-build",
        CatalogWorkerPlan::LoadOnly => return,
    };
    let mut child = match Command::new(&binary)
        .arg(operation)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            send_builder_failure(
                tx,
                plan,
                false,
                format!("start catalog builder {binary}: {error}"),
            );
            return;
        }
    };
    let Some(stdout) = child.stdout.take() else {
        send_builder_failure(
            tx,
            plan,
            false,
            "catalog builder stdout was unavailable".into(),
        );
        return;
    };
    let mut handshake_seen = false;
    let mut terminal_seen = false;
    let mut catalog_ready_seen = false;
    for line in BufReader::new(stdout).lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                send_builder_failure(
                    tx,
                    plan,
                    catalog_ready_seen,
                    format!("read catalog builder event: {error}"),
                );
                break;
            }
        };
        let event = match decode_builder_event(&line) {
            Ok(event) => event,
            Err(error) => {
                send_builder_failure(tx, plan, catalog_ready_seen, error);
                break;
            }
        };
        if !handshake_seen && !matches!(event, CatalogBuilderEvent::Handshake { .. }) {
            send_builder_failure(
                tx,
                plan,
                catalog_ready_seen,
                "catalog builder emitted an event before its handshake".into(),
            );
            break;
        }
        match event {
            CatalogBuilderEvent::Handshake {
                operation: child_operation,
                ..
            } if !handshake_seen && child_operation == operation => handshake_seen = true,
            CatalogBuilderEvent::Handshake { .. } => {
                send_builder_failure(
                    tx,
                    plan,
                    catalog_ready_seen,
                    "catalog builder emitted a duplicate or mismatched handshake".into(),
                );
                break;
            }
            CatalogBuilderEvent::Progress { title, detail, .. } => {
                let percent = library_db::catalog_progress_percent_from_display(&title, &detail);
                let _ = tx.send(CatalogWorkerMessage::Progress {
                    title,
                    detail,
                    percent,
                });
            }
            CatalogBuilderEvent::SystemDiscovered { system_id, .. } => {
                let _ = tx.send(CatalogWorkerMessage::SystemDiscovered { system_id });
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
                match library_db::load_arcade_catalog_from_snapshot(
                    root,
                    std::path::Path::new(&snapshot_path),
                ) {
                    Ok(loaded) => {
                        catalog_ready_seen = true;
                        send_ready_catalog(
                            tx,
                            loaded.catalog,
                            None,
                            load_us,
                            CatalogSource::FreshBuild,
                            true,
                        );
                    }
                    Err(error) => {
                        let _ = tx.send(CatalogWorkerMessage::LoadFailed { error });
                    }
                }
            }
            CatalogBuilderEvent::Persisted { summary, .. } => {
                let completed_build_seconds = summary.completed_build_seconds;
                let _ = tx.send(CatalogWorkerMessage::Persisted {
                    summary: refresh_summary(summary),
                    completed_build_seconds,
                });
            }
            CatalogBuilderEvent::Unchanged { summary, .. } => {
                let _ = tx.send(CatalogWorkerMessage::Unchanged {
                    summary: refresh_summary(summary),
                });
            }
            CatalogBuilderEvent::Changed { detail, .. } => {
                let _ = tx.send(CatalogWorkerMessage::Changed { detail });
            }
            CatalogBuilderEvent::Failure { stage, error, .. } => {
                terminal_seen = true;
                send_builder_failure(
                    tx,
                    plan,
                    catalog_ready_seen,
                    format!("catalog builder {stage} failed: {error}"),
                );
            }
            CatalogBuilderEvent::Done { .. } => {
                terminal_seen = true;
                let _ = tx.send(CatalogWorkerMessage::Done);
            }
        }
    }
    match child.wait() {
        Ok(_status) if handshake_seen && terminal_seen => {}
        Ok(status) => {
            send_builder_failure(
                tx,
                plan,
                catalog_ready_seen,
                format!("catalog builder exited {status}; handshake={handshake_seen} terminal={terminal_seen}"),
            );
        }
        Err(error) => {
            send_builder_failure(
                tx,
                plan,
                catalog_ready_seen,
                format!("wait for catalog builder: {error}"),
            );
        }
    }
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

fn decode_builder_event(line: &str) -> Result<CatalogBuilderEvent, String> {
    let event = serde_json::from_str::<CatalogBuilderEvent>(line)
        .map_err(|error| format!("invalid catalog builder event: {error}"))?;
    if event.protocol() != CATALOG_BUILDER_PROTOCOL_VERSION {
        return Err(format!(
            "catalog builder protocol {} is incompatible",
            event.protocol()
        ));
    }
    Ok(event)
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
pub(super) enum CatalogWorkerRequest {
    LoadOnly,
    StrictLoad,
    CheckStamp,
    ForceBuild,
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
    ProbeNavigationThenSqlite,
    ProbeSqlite,
    AlreadyLoadedReady,
    AlreadyProbedMissing,
    AlreadyProbedEmpty,
}

impl CatalogWorkerRequest {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::LoadOnly => "load_only",
            Self::StrictLoad => "strict_load",
            Self::CheckStamp => "check_stamp",
            Self::ForceBuild => "force_build",
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
    ForceBuild,
    FreshBuild,
}

impl CatalogWorkerPlan {
    fn label(self) -> &'static str {
        match self {
            Self::LoadOnly => "load_only",
            Self::CheckStamp => "check_stamp",
            Self::ForceBuild => "force_build",
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
        CatalogWorkerRequest::ForceBuild => return CatalogWorkerPlan::ForceBuild,
        CatalogWorkerRequest::FreshBuild => return CatalogWorkerPlan::FreshBuild,
        _ => {}
    }
    match cache_state {
        CatalogCacheState::Ready => match request {
            CatalogWorkerRequest::LoadOnly => CatalogWorkerPlan::LoadOnly,
            CatalogWorkerRequest::StrictLoad => CatalogWorkerPlan::LoadOnly,
            CatalogWorkerRequest::CheckStamp => CatalogWorkerPlan::CheckStamp,
            CatalogWorkerRequest::ForceBuild => CatalogWorkerPlan::ForceBuild,
            CatalogWorkerRequest::FreshBuild => CatalogWorkerPlan::FreshBuild,
        },
        CatalogCacheState::Empty | CatalogCacheState::Missing => CatalogWorkerPlan::ForceBuild,
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
    },
    LoadFailed {
        error: String,
    },
    FreshCleanupStarted,
    FreshCleanupCompleted {
        removed: usize,
    },
    SystemDiscovered {
        system_id: String,
    },
    SearchIndexBuildStarted {
        text_index_token: usize,
        games: usize,
        source: CatalogSource,
    },
    SearchIndexesReady {
        text_index_token: usize,
        games: usize,
        source: CatalogSource,
        timing: mister_magik_catalog::arcade_catalog::ArcadeTextIndexBuildTiming,
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
        publication_ack: Option<mpsc::Sender<()>>,
    },
    Persisted {
        summary: library_db::LibraryRefreshSummary,
        completed_build_seconds: Option<u64>,
    },
    PersistenceFailed {
        error: String,
    },
    Unchanged {
        summary: library_db::LibraryRefreshSummary,
    },
    Changed {
        detail: String,
    },
    Done,
}

#[derive(Default)]
struct CatalogProgressCoalescer {
    last_sent: Option<Instant>,
    last_phase: Option<library_db::CatalogProgressPhase>,
    last_title: String,
    last_percent: i32,
}

impl CatalogProgressCoalescer {
    fn should_send(
        &mut self,
        phase: library_db::CatalogProgressPhase,
        title: &str,
        percent: i32,
    ) -> bool {
        let now = Instant::now();
        let phase_changed = self.last_sent.is_none()
            || self.last_phase != Some(phase)
            || self.last_title != title
            || self.last_percent != percent
            || percent >= 0;
        let elapsed = self
            .last_sent
            .map(|last| now.duration_since(last))
            .unwrap_or(Duration::MAX);
        if !phase_changed && elapsed < Duration::from_millis(250) {
            return false;
        }
        self.last_sent = Some(now);
        self.last_phase = Some(phase);
        self.last_title.clear();
        self.last_title.push_str(title);
        self.last_percent = percent;
        true
    }
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
    });
}

pub(super) fn catalog_load_timing_detail(loaded: &library_db::LibraryCatalogLoad) -> String {
    format!(
        "games={} rows={} total_us={} open_us={} schema_check_us={} query_us={} query_prepare_us={} query_first_row_us={} query_row_read_us={} query_row_hydrate_us={} launch_plans_us={} systems_us={} catalog_us={} {}",
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
    tx: &mpsc::Sender<CatalogWorkerMessage>,
) {
    if let Some((catalog, stamp)) = loaded_catalog {
        repair_navigation_projection_cache_for_catalog(catalog, stamp, tx);
    } else if fallback_repair_allowed {
        repair_navigation_projection_cache(root, tx);
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
    tx: &mpsc::Sender<CatalogWorkerMessage>,
) {
    let started = Instant::now();
    let sqlite_path = library_db::default_sqlite_path();
    match library_db::catalog_projection_pair_current(&sqlite_path, stamp) {
        Ok(true) => {
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_navigation_repair_tsv".to_string(),
                detail: format!(
                    "status=current elapsed_us={}",
                    started.elapsed().as_micros()
                ),
            });
            return;
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
    match library_db::repair_catalog_projections_for_catalog(&sqlite_path, catalog, stamp) {
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

fn repair_navigation_projection_cache(root: &str, tx: &mpsc::Sender<CatalogWorkerMessage>) {
    let started = Instant::now();
    let sqlite_path = library_db::default_sqlite_path();
    let Some(stamp) = (match library_db::read_sqlite_catalog_stamp(&sqlite_path) {
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
    match library_db::catalog_projection_pair_current(&sqlite_path, &stamp) {
        Ok(true) => {
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_navigation_repair_tsv".to_string(),
                detail: format!(
                    "status=current elapsed_us={}",
                    started.elapsed().as_micros()
                ),
            });
            return;
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

    let load_t = Instant::now();
    let loaded = match library_db::load_arcade_catalog_from_sqlite(root) {
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
    let repair_t = Instant::now();
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
    if !loaded_catalog_stamp_still_current(&sqlite_path, loaded_stamp, started, tx) {
        return;
    }
    match library_db::repair_catalog_projections_for_catalog(
        &sqlite_path,
        &loaded.catalog,
        loaded_stamp,
    ) {
        Ok(()) => {
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_navigation_repair_tsv".to_string(),
                detail: format!(
                    "status=ok elapsed_us={} load_us={} repair_us={} games={}",
                    started.elapsed().as_micros(),
                    load_us,
                    repair_t.elapsed().as_micros(),
                    loaded.catalog.len()
                ),
            });
        }
        Err(e) => {
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_navigation_repair_tsv".to_string(),
                detail: format!(
                    "status=repair_failed elapsed_us={} load_us={} repair_us={} error={e}",
                    started.elapsed().as_micros(),
                    load_us,
                    repair_t.elapsed().as_micros()
                ),
            });
        }
    }
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
) -> Result<Option<library_db::LibraryCatalogLoad>, String> {
    let sqlite_path = library_db::default_sqlite_path();
    let summary_path = catalog_summary::summary_path_for_sqlite(&sqlite_path);
    let Some(summary) = catalog_summary::read_catalog_summary(&summary_path)? else {
        return Ok(None);
    };
    let stamp = catalog_stamp::CatalogStamp::from_lines(summary.catalog_stamp_lines);
    let Some(stored_stamp) = library_db::read_sqlite_catalog_stamp(&sqlite_path)? else {
        return Ok(None);
    };
    if stored_stamp != stamp {
        return Ok(None);
    }
    library_db::load_arcade_catalog_from_navigation_projection(root, &sqlite_path, &stamp)
}

pub(super) fn print_startup_event(start: Instant, name: &str, detail: impl std::fmt::Display) {
    let elapsed_ms = start.elapsed().as_millis();
    let detail = detail.to_string();
    boot_analytics::event(name, format!("since_run_ui_ms={elapsed_ms} {detail}"));
    crate::ui_logln!("startup_timing\t{name}\t{}ms\t{detail}", elapsed_ms);
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn catalog_builder_protocol_rejects_malformed_and_incompatible_events() {
        assert!(decode_builder_event("not-json").is_err());
        let incompatible = serde_json::json!({
            "event": "done",
            "protocol": CATALOG_BUILDER_PROTOCOL_VERSION + 1,
        });
        assert!(decode_builder_event(&incompatible.to_string())
            .unwrap_err()
            .contains("incompatible"));
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
            CatalogWorkerPlan::ForceBuild
        );
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Empty, CatalogWorkerRequest::CheckStamp),
            CatalogWorkerPlan::ForceBuild
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
    fn catalog_worker_refreshes_only_when_requested() {
        for state in [
            CatalogCacheState::Ready,
            CatalogCacheState::Empty,
            CatalogCacheState::Missing,
        ] {
            assert_eq!(
                catalog_worker_plan(state, CatalogWorkerRequest::ForceBuild),
                CatalogWorkerPlan::ForceBuild
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
            -1
        ));
        assert!(!coalescer.should_send(
            library_db::CatalogProgressPhase::ClassifyingLibrary,
            "Classifying library",
            -1
        ));
        coalescer.last_sent = Some(Instant::now() - Duration::from_millis(300));
        assert!(coalescer.should_send(
            library_db::CatalogProgressPhase::ClassifyingLibrary,
            "Classifying library",
            -1
        ));
    }

    #[test]
    fn catalog_progress_coalescer_sends_phase_and_percent_changes() {
        let mut coalescer = CatalogProgressCoalescer::default();
        assert!(coalescer.should_send(
            library_db::CatalogProgressPhase::ClassifyingLibrary,
            "Classifying library",
            -1
        ));
        assert!(coalescer.should_send(
            library_db::CatalogProgressPhase::IndexingLibrary,
            "Indexing library",
            90
        ));
        assert!(coalescer.should_send(
            library_db::CatalogProgressPhase::LoadingLibrary,
            "Loading library",
            100
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
