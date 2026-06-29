use super::*;
use mister_magik_catalog::runtime_thread::{apply_runtime_thread_policy, RuntimeThreadRole};
use mister_magik_catalog::{arcade_catalog::ArcadeCatalog, catalog_stamp, catalog_summary};

pub(super) fn start_library_catalog_worker(
    root: String,
    request: CatalogWorkerRequest,
    initial_cache: CatalogWorkerInitialCache,
) -> mpsc::Receiver<CatalogWorkerMessage> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("library-catalog".to_string())
        .spawn(move || {
            apply_runtime_thread_policy(RuntimeThreadRole::CatalogWorker);
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
                            let _ = tx.send(CatalogWorkerMessage::Ready {
                                catalog: loaded.catalog,
                                summary: None,
                                load_us: loaded.us,
                                source: CatalogSource::NavigationProjection,
                                durable_save_pending: false,
                            });
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
                            eprintln!("library navigation projection load failed: {e}");
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
                                    let _ = tx.send(CatalogWorkerMessage::Ready {
                                        catalog: loaded.catalog,
                                        summary: None,
                                        load_us: loaded.us,
                                        source: CatalogSource::FullSqlite,
                                        durable_save_pending: false,
                                    });
                                    if let Some(stamp) = loaded.stamp {
                                        projection_repair_catalog =
                                            Some((catalog_for_repair, stamp));
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("library catalog cache load failed: {e}");
                                let _ = tx.send(CatalogWorkerMessage::Timing {
                                    name: "catalog_worker_cache_load_failed".to_string(),
                                    detail: e,
                                });
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
                                let _ = tx.send(CatalogWorkerMessage::Ready {
                                    catalog: loaded.catalog,
                                    summary: None,
                                    load_us: loaded.us,
                                    source: CatalogSource::FullSqlite,
                                    durable_save_pending: false,
                                });
                                if let Some(stamp) = loaded.stamp {
                                    projection_repair_catalog = Some((catalog_for_repair, stamp));
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("library catalog cache load failed: {e}");
                            let _ = tx.send(CatalogWorkerMessage::Timing {
                                name: "catalog_worker_cache_load_failed".to_string(),
                                detail: e,
                            });
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
            }
            let plan = catalog_worker_plan(cache_state, request);
            let first_catalog_build =
                plan == CatalogWorkerPlan::ForceBuild && staged_ram_catalog_enabled(cache_state);
            if first_catalog_build {
                // First database creation owns the machine until the RAM catalog is ready.
                // Smooth scan-screen animation is secondary to meeting the first-scan gate.
                apply_runtime_thread_policy(RuntimeThreadRole::CatalogForeground);
            }
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_refresh_decision".to_string(),
                detail: format!(
                    "cache_state={} request={} plan={}",
                    cache_state.label(),
                    request.label(),
                    plan.label()
                ),
            });
            match plan {
                CatalogWorkerPlan::LoadOnly => {
                    let _ = tx.send(CatalogWorkerMessage::Done);
                    repair_navigation_projection_cache_after_ready(
                        &root,
                        projection_repair_catalog.as_ref(),
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
            }
            if staged_ram_catalog_enabled(cache_state) {
                let bootstrap = library_db::bootstrap_default_library_progress(Some(&mut progress));
                let _ = tx.send(CatalogWorkerMessage::Timing {
                    name: "bootstrap_scan_complete".to_string(),
                    detail: format!(
                        "launchers={} scan_us={}",
                        bootstrap.launchers, bootstrap.scan_us
                    ),
                });
                if bootstrap.launchers > 50 {
                    send_catalog_progress(
                        &tx,
                        library_db::CatalogProgress::finding_games_found(bootstrap.launchers),
                    );
                }
                let artifact_result = if first_catalog_build {
                    library_db::scan_default_library_foreground_with_events(
                        Some(&mut progress),
                        Some(&mut scan_events),
                    )
                } else {
                    library_db::scan_default_library_with_events(
                        Some(&mut progress),
                        Some(&mut scan_events),
                    )
                };
                let artifact = match artifact_result {
                    Ok(artifact) => artifact,
                    Err(e) => {
                        eprintln!("library scan failed: {e}");
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
                let projection_t = Instant::now();
                let summary_projection =
                    catalog_summary::CatalogSummaryProjection::from_catalog(&catalog, artifact.stamp());
                let navigation_projection =
                    library_db::CatalogNavigationProjection::from_catalog(&catalog, artifact.stamp());
                let projection_us = projection_t.elapsed().as_micros() as u64;
                let _ = tx.send(CatalogWorkerMessage::Ready {
                    catalog,
                    summary: None,
                    load_us,
                    source: CatalogSource::FreshBuild,
                    durable_save_pending: true,
                });
                let _ = tx.send(CatalogWorkerMessage::Timing {
                    name: "catalog_worker_ram_catalog".to_string(),
                    detail: format!(
                        "games={catalog_len} catalog_us={load_us} projection_us={projection_us}"
                    ),
                });
                send_catalog_progress(
                    &tx,
                    library_db::CatalogProgress::saving_before_opening_launcher(),
                );
                match artifact.save_default_sqlite_with_projections(
                    summary_projection,
                    navigation_projection,
                    Some(&mut progress),
                ) {
                    Ok(summary) => {
                        let _ = tx.send(CatalogWorkerMessage::Persisted {
                            summary: summary.clone(),
                        });
                        let _ = tx.send(CatalogWorkerMessage::Timing {
                            name: "catalog_worker_saved_catalog".to_string(),
                            detail: format!(
                                "games={catalog_len} skipped=precomputed_projection"
                            ),
                        });
                    }
                    Err(e) => {
                        eprintln!("library persistence failed after RAM catalog ready: {e}");
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
                                "unchanged={} check_us={} compute_us={} open_us={} read_us={} compare_us={} stored={} current={} stored_lines={} current_lines={}",
                                check.unchanged,
                                check.check_us,
                                check.compute_us,
                                check.open_us,
                                check.read_us,
                                check.compare_us,
                                check.stored_fingerprint.as_deref().unwrap_or("missing"),
                                check.current_fingerprint,
                                check.stored_lines,
                                check.current_lines
                            ),
                        });
                        if check.unchanged {
                            match library_db::default_sqlite_cached_summary(check.check_us) {
                                Ok(summary) => {
                                    let _ = tx.send(CatalogWorkerMessage::Unchanged { summary });
                                    repair_navigation_projection_cache_after_ready(
                                        &root,
                                        projection_repair_catalog.as_ref(),
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
                            detail: "Catalog stamp changed; rebuild required.".to_string(),
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
            let refresh = match library_db::rebuild_default_sqlite_database_with_catalog(
                &root,
                Some(&mut progress),
                Some(&mut scan_events),
            ) {
                Ok(refresh) => Some(refresh),
                Err(e) => {
                    eprintln!("library refresh failed: {e}");
                    send_catalog_progress(&tx, library_db::CatalogProgress::library_scan_failed(e));
                    None
                }
            };
            let rebuilt = refresh.is_some();
            if rebuilt {
                send_catalog_progress(&tx, library_db::CatalogProgress::loading_sqlite_catalog());
            }
            match refresh {
                Some(refresh) => {
                    let summary = refresh.summary;
                    let loaded = refresh.catalog;
                    send_catalog_load_timing(&tx, "catalog_worker_saved_catalog", &loaded);
                    let _ = tx.send(CatalogWorkerMessage::Ready {
                        catalog: loaded.catalog,
                        summary: Some(summary),
                        load_us: loaded.us,
                        source: CatalogSource::FreshBuild,
                        durable_save_pending: false,
                    });
                }
                None => {
                    send_catalog_progress(
                        &tx,
                        library_db::CatalogProgress::library_load_failed(
                            "library refresh did not produce a catalog".to_string(),
                        ),
                    );
                }
            }
        })
        .expect("spawn library-catalog");
    rx
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogWorkerRequest {
    LoadOnly,
    CheckStamp,
    ForceBuild,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogWorkerInitialCache {
    ProbeNavigationThenSqlite,
    ProbeSqlite,
    AlreadyLoadedReady,
}

impl CatalogWorkerRequest {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::LoadOnly => "load_only",
            Self::CheckStamp => "check_stamp",
            Self::ForceBuild => "force_build",
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
}

impl CatalogWorkerPlan {
    fn label(self) -> &'static str {
        match self {
            Self::LoadOnly => "load_only",
            Self::CheckStamp => "check_stamp",
            Self::ForceBuild => "force_build",
        }
    }
}

fn catalog_worker_plan(
    cache_state: CatalogCacheState,
    request: CatalogWorkerRequest,
) -> CatalogWorkerPlan {
    if request == CatalogWorkerRequest::ForceBuild {
        return CatalogWorkerPlan::ForceBuild;
    }
    match cache_state {
        CatalogCacheState::Ready => match request {
            CatalogWorkerRequest::LoadOnly => CatalogWorkerPlan::LoadOnly,
            CatalogWorkerRequest::CheckStamp => CatalogWorkerPlan::CheckStamp,
            CatalogWorkerRequest::ForceBuild => CatalogWorkerPlan::ForceBuild,
        },
        CatalogCacheState::Empty | CatalogCacheState::Missing => CatalogWorkerPlan::ForceBuild,
    }
}

fn staged_ram_catalog_enabled(cache_state: CatalogCacheState) -> bool {
    !cache_state.has_usable_catalog()
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
    SystemDiscovered {
        system_id: String,
    },
    Ready {
        catalog: ArcadeCatalog,
        summary: Option<library_db::LibraryRefreshSummary>,
        load_us: u64,
        source: CatalogSource,
        durable_save_pending: bool,
    },
    Persisted {
        summary: library_db::LibraryRefreshSummary,
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
    tx: &mpsc::Sender<CatalogWorkerMessage>,
) {
    if let Some((catalog, stamp)) = loaded_catalog {
        repair_navigation_projection_cache_for_catalog(catalog, stamp, tx);
    } else {
        repair_navigation_projection_cache(root, tx);
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
    println!("startup_timing\t{name}\t{}ms\t{detail}", elapsed_ms);
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn missing_catalog_scans_before_persisting_without_using_stale_cache() {
        assert!(staged_ram_catalog_enabled(CatalogCacheState::Missing));
        assert!(staged_ram_catalog_enabled(CatalogCacheState::Empty));
        assert!(!staged_ram_catalog_enabled(CatalogCacheState::Ready));
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
