use super::*;

pub(super) fn start_library_catalog_worker(root: String) -> mpsc::Receiver<CatalogWorkerMessage> {
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
            let mut cached_catalog_ready = false;
            match library_db::load_arcade_catalog_from_sqlite(&root) {
                Ok(loaded) => {
                    cached_catalog_ready = !loaded.catalog.games.is_empty();
                    let _ = tx.send(CatalogWorkerMessage::Ready {
                        catalog: loaded.catalog,
                        summary: None,
                        load_us: loaded.us,
                    });
                }
                Err(e) => {
                    eprintln!("library catalog cache load failed: {e}");
                    let _ = tx.send(CatalogWorkerMessage::Progress {
                        title: "Indexing library".to_string(),
                        detail: "No cached catalog; scanning library...".to_string(),
                    });
                }
            }
            if cached_catalog_ready && !catalog_refresh_requested() {
                let _ = tx.send(CatalogWorkerMessage::Done);
                return;
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
                if cached_catalog_ready {
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
