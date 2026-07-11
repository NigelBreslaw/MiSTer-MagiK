use crate::builder_protocol::{
    BuilderSummary, CatalogBuilderEvent, CATALOG_BUILDER_PROTOCOL_VERSION,
};
use crate::catalog_navigation::write_catalog_navigation_snapshot;
use crate::library_db;
use crate::runtime_thread::{apply_runtime_thread_policy, RuntimeThreadRole};
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuilderOperation {
    Check,
    Build,
    Rebuild,
}

impl BuilderOperation {
    fn label(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Build => "build",
            Self::Rebuild => "rebuild",
        }
    }
}

pub fn run(
    operation: BuilderOperation,
    mut emit: impl FnMut(CatalogBuilderEvent),
) -> Result<(), String> {
    let protocol = CATALOG_BUILDER_PROTOCOL_VERSION;
    let run_id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    emit(CatalogBuilderEvent::Handshake {
        protocol,
        operation: operation.label().into(),
        run_id,
    });
    let _lock = BuilderLock::acquire().map_err(|error| fail(protocol, "lock", error, &mut emit))?;

    if operation == BuilderOperation::Check {
        return check(protocol, &mut emit);
    }

    apply_runtime_thread_policy(if operation == BuilderOperation::Build {
        RuntimeThreadRole::CatalogForeground
    } else {
        RuntimeThreadRole::CatalogWorker
    });
    let scanned = {
        let protocol_output = RefCell::new(&mut emit);
        let mut scan_progress = |title: &str, detail: &str| {
            (protocol_output.borrow_mut())(CatalogBuilderEvent::Progress {
                protocol,
                title: title.into(),
                detail: detail.into(),
            });
        };
        let mut scan_events = |event: library_db::LibraryScanEvent| match event {
            library_db::LibraryScanEvent::SystemDiscovered { system_id } => {
                (protocol_output.borrow_mut())(CatalogBuilderEvent::SystemDiscovered {
                    protocol,
                    system_id,
                });
            }
        };
        library_db::scan_default_library_ram_foreground_with_events(
            Some(&mut scan_progress),
            Some(&mut scan_events),
        )
    }
    .map_err(|error| fail(protocol, "scan", error, &mut emit))?;
    let stats = scanned.stats().clone();
    emit(CatalogBuilderEvent::Timing {
        protocol,
        name: "library_scan_complete".into(),
        detail: format!(
            "scan_us={} discover_us={} classify_us={} discoveries={} normal_files={} containers={} entries={}",
            stats.scan_us, stats.discover_us, stats.classify_us, stats.discoveries,
            stats.normal_files, stats.containers, stats.entries
        ),
    });
    let artifact = scanned.complete_coverage_audit();
    let root = crate::arcade_catalog::DEFAULT_ARCADE_ROOT;
    let started = Instant::now();
    let catalog = artifact.catalog(root);
    let load_us = started.elapsed().as_micros() as u64;
    let snapshot_path = snapshot_path();
    if let Some(parent) = snapshot_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| fail(protocol, "snapshot", error.to_string(), &mut emit))?;
    }
    write_catalog_navigation_snapshot(&snapshot_path, &catalog, artifact.stamp())
        .map_err(|error| fail(protocol, "snapshot", error, &mut emit))?;
    emit(CatalogBuilderEvent::CatalogReady {
        protocol,
        snapshot_path: snapshot_path.display().to_string(),
        games: catalog.len(),
        load_us,
    });
    apply_runtime_thread_policy(RuntimeThreadRole::CatalogWorker);
    let mut progress = |title: &str, detail: &str| {
        emit(CatalogBuilderEvent::Progress {
            protocol,
            title: title.into(),
            detail: detail.into(),
        });
    };
    let summary = artifact
        .save_default_sqlite_with_catalog_projection(&catalog, Some(&mut progress))
        .map_err(|error| fail(protocol, "persist", error, &mut emit))?;
    emit(CatalogBuilderEvent::Persisted {
        protocol,
        summary: summary.into(),
    });
    let _ = std::fs::remove_file(snapshot_path);
    emit(CatalogBuilderEvent::Done { protocol });
    Ok(())
}

struct BuilderLock(File);

impl BuilderLock {
    fn acquire() -> Result<Self, String> {
        let path = std::env::var_os("MISTER_CATALOG_BUILDER_LOCK")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp/mister-magik/catalog-builder.lock"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create catalog builder lock directory: {e}"))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
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

fn check(protocol: u32, emit: &mut impl FnMut(CatalogBuilderEvent)) -> Result<(), String> {
    match library_db::default_sqlite_catalog_stamp_check() {
        Ok(check) if check.unchanged => {
            let summary = library_db::default_sqlite_cached_summary(check.check_us)
                .map_err(|error| fail(protocol, "summary", error, emit))?;
            emit(CatalogBuilderEvent::Unchanged {
                protocol,
                summary: BuilderSummary::from(summary),
            });
        }
        Ok(check) => emit(CatalogBuilderEvent::Changed {
            protocol,
            detail: format!(
                "Catalog inputs changed; rebuild required. {}",
                check.drift.detail
            ),
        }),
        Err(error) => return Err(fail(protocol, "check", error, emit)),
    }
    emit(CatalogBuilderEvent::Done { protocol });
    Ok(())
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
    });
    error
}
