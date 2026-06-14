use super::*;

pub(super) fn start_library_catalog_worker(
    root: String,
    refresh_requested: bool,
) -> mpsc::Receiver<CatalogWorkerMessage> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("library-catalog".to_string())
        .spawn(move || {
            lower_background_priority();
            let progress_tx = tx.clone();
            let mut progress = move |title: &str, detail: &str| {
                let _ = progress_tx.send(CatalogWorkerMessage::Progress {
                    title: title.to_string(),
                    detail: detail.to_string(),
                });
            };
            let mut cache_state = CatalogCacheState::Missing;
            match library_db::load_arcade_catalog_from_sqlite(&root) {
                Ok(loaded) => {
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
                }
            }
            match catalog_worker_plan(cache_state, refresh_requested) {
                CatalogWorkerPlan::UseCacheOnly => {
                    let _ = tx.send(CatalogWorkerMessage::Done);
                    return;
                }
                CatalogWorkerPlan::RefreshInProcess => {
                    let (title, detail) = if cache_state == CatalogCacheState::Ready {
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
                    });
                }
            }
            let summary = match library_db::refresh_default_sqlite_database(Some(&mut progress)) {
                Ok(summary) => Some(summary),
                Err(e) => {
                    eprintln!("library refresh failed: {e}");
                    let _ = tx.send(CatalogWorkerMessage::Progress {
                        title: "Library scan failed".to_string(),
                        detail: e,
                    });
                    None
                }
            };
            if let Some(summary) = summary.as_ref().filter(|summary| summary.skipped) {
                if cache_state == CatalogCacheState::Ready {
                    let _ = tx.send(CatalogWorkerMessage::Unchanged {
                        summary: summary.clone(),
                    });
                    return;
                }
            }
            if summary.is_some() {
                let _ = tx.send(CatalogWorkerMessage::Progress {
                    title: "Loading library".to_string(),
                    detail: "Opening SQLite catalog...".to_string(),
                });
            }
            match library_db::load_arcade_catalog_from_sqlite(&root) {
                Ok(loaded) => {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogWorkerPlan {
    UseCacheOnly,
    RefreshInProcess,
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

pub(super) enum CatalogWorkerMessage {
    Progress {
        title: String,
        detail: String,
    },
    Ready {
        catalog: ArcadeCatalog,
        summary: Option<library_db::LibraryRefreshSummary>,
        load_us: u64,
    },
    Unchanged {
        summary: library_db::LibraryRefreshSummary,
    },
    Done,
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
        assert_eq!(
            catalog_worker_plan(CatalogCacheState::Ready, false),
            CatalogWorkerPlan::UseCacheOnly
        );
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
}
