use super::*;
use mister_magik_catalog::{catalog_stamp, catalog_summary};

pub(super) fn start_library_catalog_worker(
    root: String,
    request: CatalogWorkerRequest,
    initial_cache: CatalogWorkerInitialCache,
) -> mpsc::Receiver<CatalogWorkerMessage> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("library-catalog".to_string())
        .spawn(move || {
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
                                    let _ = tx.send(CatalogWorkerMessage::Ready {
                                        catalog: loaded.catalog,
                                        summary: None,
                                        load_us: loaded.us,
                                        source: CatalogSource::FullSqlite,
                                    });
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
                                let _ = tx.send(CatalogWorkerMessage::Ready {
                                    catalog: loaded.catalog,
                                    summary: None,
                                    load_us: loaded.us,
                                    source: CatalogSource::FullSqlite,
                                });
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
            if cache_state.has_usable_catalog() {
                lower_background_priority();
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
                let artifact = match library_db::scan_default_library_with_events(
                    Some(&mut progress),
                    Some(&mut scan_events),
                ) {
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
                send_catalog_progress(
                    &tx,
                    library_db::CatalogProgress::saving_before_opening_launcher(),
                );
                match artifact.save_default_sqlite_with_catalog(&root, Some(&mut progress)) {
                    Ok(refresh) => {
                        let summary = refresh.summary;
                        let loaded = refresh.catalog;
                        let _ = tx.send(CatalogWorkerMessage::Persisted {
                            summary: summary.clone(),
                        });
                        send_catalog_load_timing(&tx, "catalog_worker_saved_catalog", &loaded);
                        let _ = tx.send(CatalogWorkerMessage::Ready {
                            catalog: loaded.catalog,
                            summary: Some(summary),
                            load_us: loaded.us,
                            source: CatalogSource::FreshBuild,
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
                                    return;
                                }
                                Err(e) => {
                                    let _ = tx.send(CatalogWorkerMessage::Timing {
                                        name: "catalog_cached_summary_failed".to_string(),
                                        detail: e,
                                    });
                                    restore_catalog_worker_priority();
                                    let _ = tx.send(CatalogWorkerMessage::Changed {
                                        detail:
                                            "Catalog summary unavailable; rebuild required."
                                                .to_string(),
                                    });
                                    return;
                                }
                            }
                        }
                        restore_catalog_worker_priority();
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
                        restore_catalog_worker_priority();
                        let _ = tx.send(CatalogWorkerMessage::Changed {
                            detail: "Catalog stamp check failed; rebuild required.".to_string(),
                        });
                        return;
                    }
                }
            }
            restore_catalog_worker_priority();
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

fn load_navigation_projection_cache(
    root: &str,
) -> Result<Option<library_db::LibraryCatalogLoad>, String> {
    let sqlite_path = library_db::default_sqlite_path();
    let summary_path = catalog_summary::summary_path_for_sqlite(&sqlite_path);
    let Some(summary) = catalog_summary::read_catalog_summary(&summary_path)? else {
        return Ok(None);
    };
    let stamp = catalog_stamp::CatalogStamp::from_lines(summary.catalog_stamp_lines);
    library_db::load_arcade_catalog_from_navigation_projection(root, &sqlite_path, &stamp)
}

pub(super) fn lower_background_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        let _ = libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

fn restore_catalog_worker_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        let _ = libc::setpriority(libc::PRIO_PROCESS, 0, 0);
    }
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
