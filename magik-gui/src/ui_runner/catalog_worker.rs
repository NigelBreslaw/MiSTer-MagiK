use super::*;

pub(super) fn start_library_catalog_worker(
    root: String,
    refresh_requested: bool,
) -> mpsc::Receiver<CatalogWorkerMessage> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("library-catalog".to_string())
        .spawn(move || {
            let progress_tx = tx.clone();
            let mut progress_coalescer = CatalogProgressCoalescer::default();
            let mut progress = move |title: &str, detail: &str| {
                if !progress_coalescer.should_send(title, detail) {
                    return;
                }
                let _ = progress_tx.send(CatalogWorkerMessage::Progress {
                    title: title.to_string(),
                    detail: detail.to_string(),
                    percent: catalog_scan_percent(title, detail),
                });
            };
            let mut cache_state = CatalogCacheState::Missing;
            match library_db::load_arcade_catalog_from_sqlite(&root) {
                Ok(loaded) => {
                    send_catalog_load_timing(&tx, "catalog_worker_cache_load", &loaded);
                    if loaded.catalog.games.is_empty() {
                        cache_state = CatalogCacheState::Empty;
                    } else {
                        cache_state =
                            if library_db::default_sqlite_preview_archive_fingerprint_unchanged() {
                                CatalogCacheState::Ready
                            } else {
                                CatalogCacheState::ReadyWithStalePreviewArchive
                            };
                        let _ = tx.send(CatalogWorkerMessage::Ready {
                            catalog: loaded.catalog,
                            summary: None,
                            load_us: loaded.us,
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
            let plan = catalog_worker_plan(cache_state, refresh_requested);
            if cache_state.has_usable_catalog() {
                lower_background_priority();
            }
            let _ = tx.send(CatalogWorkerMessage::Timing {
                name: "catalog_refresh_decision".to_string(),
                detail: format!(
                    "cache_state={} refresh_requested={} plan={}",
                    cache_state.label(),
                    refresh_requested,
                    plan.label()
                ),
            });
            match plan {
                CatalogWorkerPlan::UseCacheOnly => {
                    debug_assert!(!catalog_worker_plan_prewarm_required(plan));
                    skip_virtual_launch_cache_prewarm(&tx, plan);
                    let _ = tx.send(CatalogWorkerMessage::Done);
                    return;
                }
                CatalogWorkerPlan::RefreshInProcess => {
                    debug_assert!(catalog_worker_plan_prewarm_required(plan));
                    let (title, detail) = if cache_state.has_usable_catalog() {
                        (
                            "Validating library",
                            "Using cached catalog while checking for changed files...",
                        )
                    } else {
                        ("Indexing library", "No cached catalog; scanning library...")
                    };
                    let _ = tx.send(CatalogWorkerMessage::Progress {
                        title: title.to_string(),
                        detail: detail.to_string(),
                        percent: catalog_scan_percent(title, detail),
                    });
                }
            }
            if staged_ram_catalog_enabled(cache_state) {
                let artifact = match library_db::scan_default_library(Some(&mut progress)) {
                    Ok(artifact) => artifact,
                    Err(e) => {
                        eprintln!("library scan failed: {e}");
                        let _ = tx.send(CatalogWorkerMessage::Progress {
                            title: "Library scan failed".to_string(),
                            detail: e,
                            percent: -1,
                        });
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
                let _ = tx.send(CatalogWorkerMessage::Progress {
                    title: "Saving library".to_string(),
                    detail: "Writing catalog database before opening launcher...".to_string(),
                    percent: 90,
                });
                match artifact.save_default_sqlite_with_progress(Some(&mut progress)) {
                    Ok(summary) => {
                        let _ = tx.send(CatalogWorkerMessage::Persisted {
                            summary: summary.clone(),
                        });
                        materialize_virtual_launch_cache(&tx);
                        match library_db::load_arcade_catalog_from_sqlite(&root) {
                            Ok(loaded) => {
                                send_catalog_load_timing(
                                    &tx,
                                    "catalog_worker_persisted_load",
                                    &loaded,
                                );
                                let _ = tx.send(CatalogWorkerMessage::Ready {
                                    catalog: loaded.catalog,
                                    summary: Some(summary),
                                    load_us: loaded.us,
                                });
                            }
                            Err(e) => {
                                eprintln!("library catalog load failed after persistence: {e}");
                                let _ = tx.send(CatalogWorkerMessage::PersistenceFailed {
                                    error: format!("load persisted catalog: {e}"),
                                });
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("library persistence failed after RAM catalog ready: {e}");
                        let _ = tx.send(CatalogWorkerMessage::PersistenceFailed { error: e });
                    }
                }
                return;
            }
            let summary = match library_db::refresh_default_sqlite_database(Some(&mut progress)) {
                Ok(summary) => Some(summary),
                Err(e) => {
                    eprintln!("library refresh failed: {e}");
                    let _ = tx.send(CatalogWorkerMessage::Progress {
                        title: "Library scan failed".to_string(),
                        detail: e,
                        percent: -1,
                    });
                    None
                }
            };
            if let Some(summary) = summary.as_ref().filter(|summary| summary.skipped) {
                if cache_state == CatalogCacheState::Ready {
                    materialize_virtual_launch_cache(&tx);
                    let _ = tx.send(CatalogWorkerMessage::Unchanged {
                        summary: summary.clone(),
                    });
                    return;
                }
            }
            if summary.is_some() {
                materialize_virtual_launch_cache(&tx);
                let _ = tx.send(CatalogWorkerMessage::Progress {
                    title: "Loading library".to_string(),
                    detail: "Opening SQLite catalog...".to_string(),
                    percent: 100,
                });
            }
            match library_db::load_arcade_catalog_from_sqlite(&root) {
                Ok(loaded) => {
                    send_catalog_load_timing(&tx, "catalog_worker_refreshed_load", &loaded);
                    let _ = tx.send(CatalogWorkerMessage::Ready {
                        catalog: loaded.catalog,
                        summary,
                        load_us: loaded.us,
                    });
                }
                Err(e) => {
                    eprintln!("library catalog load failed: {e}");
                    let _ = tx.send(CatalogWorkerMessage::Progress {
                        title: "Library load failed".to_string(),
                        detail: e,
                        percent: -1,
                    });
                }
            }
        })
        .expect("spawn library-catalog");
    rx
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogCacheState {
    Ready,
    ReadyWithStalePreviewArchive,
    Empty,
    Missing,
}

impl CatalogCacheState {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::ReadyWithStalePreviewArchive => "ready_stale_preview_archive",
            Self::Empty => "empty",
            Self::Missing => "missing",
        }
    }

    fn has_usable_catalog(self) -> bool {
        matches!(self, Self::Ready | Self::ReadyWithStalePreviewArchive)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogWorkerPlan {
    UseCacheOnly,
    RefreshInProcess,
}

impl CatalogWorkerPlan {
    fn label(self) -> &'static str {
        match self {
            Self::UseCacheOnly => "use_cache_only",
            Self::RefreshInProcess => "refresh_in_process",
        }
    }
}

fn catalog_worker_plan_prewarm_required(plan: CatalogWorkerPlan) -> bool {
    matches!(plan, CatalogWorkerPlan::RefreshInProcess)
}

fn catalog_worker_plan(
    cache_state: CatalogCacheState,
    refresh_requested: bool,
) -> CatalogWorkerPlan {
    if refresh_requested {
        return CatalogWorkerPlan::RefreshInProcess;
    }
    match cache_state {
        CatalogCacheState::Ready => CatalogWorkerPlan::UseCacheOnly,
        CatalogCacheState::ReadyWithStalePreviewArchive
        | CatalogCacheState::Empty
        | CatalogCacheState::Missing => CatalogWorkerPlan::RefreshInProcess,
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
    Ready {
        catalog: ArcadeCatalog,
        summary: Option<library_db::LibraryRefreshSummary>,
        load_us: u64,
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
    Done,
}

fn catalog_scan_percent(title: &str, detail: &str) -> i32 {
    if title == "Loading library" {
        return 100;
    }
    if title == "Saving library" {
        if let Some(percent) = sqlite_import_percent(detail) {
            return percent;
        }
        if detail.starts_with("Finalizing ") {
            return 99;
        }
        return 90;
    }
    if matches!(title, "Saving library" | "Indexing library") && detail.starts_with("Writing ") {
        return 90;
    }
    -1
}

fn sqlite_import_percent(detail: &str) -> Option<i32> {
    let rest = detail.strip_prefix("Writing ")?;
    let mut parts = rest.split_whitespace();
    let written = parts.next()?.parse::<usize>().ok()?;
    if parts.next()? != "of" {
        return None;
    }
    let total = parts.next()?.parse::<usize>().ok()?;
    if total == 0 {
        return Some(90);
    }
    let percent = 90 + (written.min(total) * 9 / total) as i32;
    Some(percent.clamp(90, 99))
}

#[derive(Default)]
struct CatalogProgressCoalescer {
    last_sent: Option<Instant>,
    last_title: String,
    last_percent: i32,
}

impl CatalogProgressCoalescer {
    fn should_send(&mut self, title: &str, detail: &str) -> bool {
        let now = Instant::now();
        let percent = catalog_scan_percent(title, detail);
        let phase_changed = self.last_sent.is_none()
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
        self.last_title.clear();
        self.last_title.push_str(title);
        self.last_percent = percent;
        true
    }
}

pub(super) fn catalog_load_timing_detail(loaded: &library_db::LibraryCatalogLoad) -> String {
    format!(
        "games={} rows={} total_us={} open_us={} query_us={} systems_us={} catalog_us={}",
        loaded.catalog.len(),
        loaded.rows,
        loaded.us,
        loaded.open_us,
        loaded.query_us,
        loaded.systems_us,
        loaded.catalog_us
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

fn materialize_virtual_launch_cache(tx: &mpsc::Sender<CatalogWorkerMessage>) {
    let start = Instant::now();
    let summary = launcher::materialize_virtual_launch_cache_from_default_db();
    let _ = tx.send(CatalogWorkerMessage::Timing {
        name: "virtual_launch_cache_materialized".to_string(),
        detail: format!(
            "total={} written={} unchanged={} errors={} us={}",
            summary.total,
            summary.written,
            summary.unchanged,
            summary.errors,
            start.elapsed().as_micros()
        ),
    });
}

fn skip_virtual_launch_cache_prewarm(
    tx: &mpsc::Sender<CatalogWorkerMessage>,
    plan: CatalogWorkerPlan,
) {
    let _ = tx.send(CatalogWorkerMessage::Timing {
        name: "virtual_launch_cache_prewarm_skipped".to_string(),
        detail: format!("plan={} reason=catalog-cache-ready", plan.label()),
    });
}

pub(super) fn lower_background_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        let _ = libc::setpriority(libc::PRIO_PROCESS, 0, 10);
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
        let plan = catalog_worker_plan(CatalogCacheState::Ready, false);
        assert_eq!(plan, CatalogWorkerPlan::UseCacheOnly);
        assert!(!catalog_worker_plan_prewarm_required(plan));
    }

    #[test]
    fn catalog_worker_refreshes_missing_empty_or_stale_preview_cache_without_refresh() {
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Missing, false),
            CatalogWorkerPlan::RefreshInProcess
        );
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Empty, false),
            CatalogWorkerPlan::RefreshInProcess
        );
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::ReadyWithStalePreviewArchive, false),
            CatalogWorkerPlan::RefreshInProcess
        );
    }

    #[test]
    fn catalog_worker_refreshes_only_when_requested() {
        for state in [
            CatalogCacheState::Ready,
            CatalogCacheState::ReadyWithStalePreviewArchive,
            CatalogCacheState::Empty,
            CatalogCacheState::Missing,
        ] {
            assert_eq!(
                catalog_worker_plan(state, true),
                CatalogWorkerPlan::RefreshInProcess
            );
        }
    }

    #[test]
    fn missing_catalog_scans_before_persisting_without_using_stale_cache() {
        assert!(staged_ram_catalog_enabled(CatalogCacheState::Missing));
        assert!(staged_ram_catalog_enabled(CatalogCacheState::Empty));
        assert!(!staged_ram_catalog_enabled(CatalogCacheState::Ready));
        assert!(!staged_ram_catalog_enabled(
            CatalogCacheState::ReadyWithStalePreviewArchive
        ));
    }

    #[test]
    fn refresh_plan_keeps_virtual_launch_cache_prewarm() {
        assert!(catalog_worker_plan_prewarm_required(
            CatalogWorkerPlan::RefreshInProcess
        ));
    }

    #[test]
    fn catalog_progress_coalescer_throttles_repeated_scan_counts() {
        let mut coalescer = CatalogProgressCoalescer::default();
        assert!(coalescer.should_send("Classifying library", "Games found: 0"));
        assert!(!coalescer.should_send("Classifying library", "Games found: 250"));
        coalescer.last_sent = Some(Instant::now() - Duration::from_millis(300));
        assert!(coalescer.should_send("Classifying library", "Games found: 500"));
    }

    #[test]
    fn catalog_progress_coalescer_sends_phase_and_percent_changes() {
        let mut coalescer = CatalogProgressCoalescer::default();
        assert!(coalescer.should_send("Classifying library", "Games found: 0"));
        assert!(coalescer.should_send("Indexing library", "Writing 10 games, 2 archives..."));
        assert!(coalescer.should_send("Loading library", "Opening SQLite catalog..."));
    }

    #[test]
    fn catalog_scan_percent_tracks_sqlite_import_progress() {
        assert_eq!(
            catalog_scan_percent("Saving library", "Writing 0 of 100 games into SQLite..."),
            90
        );
        assert_eq!(
            catalog_scan_percent("Saving library", "Writing 50 of 100 games into SQLite..."),
            94
        );
        assert_eq!(
            catalog_scan_percent("Saving library", "Writing 100 of 100 games into SQLite..."),
            99
        );
        assert_eq!(
            catalog_scan_percent(
                "Saving library",
                "Finalizing catalog views and search indexes..."
            ),
            99
        );
    }
}
