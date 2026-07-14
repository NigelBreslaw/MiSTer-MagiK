use crate::arcade_catalog::ArcadeCatalog;
use crate::builder_protocol::{
    BuilderSummary, CatalogBuilderEvent, CATALOG_BUILDER_PROTOCOL_VERSION,
};
use crate::catalog_build_record;
use crate::catalog_navigation::write_catalog_navigation_snapshot_with_timing;
use crate::library_db;
use crate::runtime_thread::{apply_runtime_thread_policy, RuntimeThreadRole};
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
    FreshBuild,
}

impl BuilderOperation {
    fn label(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Build => "build",
            Self::Rebuild => "rebuild",
            Self::FreshBuild => "fresh-build",
        }
    }
}

pub fn run(
    operation: BuilderOperation,
    mut emit: impl FnMut(CatalogBuilderEvent),
) -> Result<(), String> {
    let mut backend = SystemBuilderBackend;
    run_with_backend(
        operation,
        BuilderConfig::production(),
        &mut backend,
        &mut emit,
    )
}

#[derive(Clone, Debug)]
struct BuilderConfig {
    lock_path: PathBuf,
    snapshot_path: PathBuf,
    sqlite_path: PathBuf,
    run_id: String,
}

impl BuilderConfig {
    fn production() -> Self {
        Self {
            lock_path: std::env::var_os("MISTER_CATALOG_BUILDER_LOCK")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(crate::builder_protocol::DEFAULT_CATALOG_BUILDER_LOCK_PATH)
                }),
            snapshot_path: snapshot_path(),
            sqlite_path: crate::catalog_config::default_sqlite_path(),
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
enum CheckDecision {
    Unchanged(BuilderSummary),
    Changed(String),
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
    type Prepared;

    fn fresh_cleanup(&mut self) -> Result<usize, String>;
    fn check(&mut self) -> Result<CheckOutput, StageFailure>;
    fn scan(
        &mut self,
        progress: &mut dyn FnMut(&str, &str),
        system_discovered: &mut dyn FnMut(String),
    ) -> Result<StageOutput<Self::Scan>, String>;
    fn prepare(&mut self, scan: Self::Scan) -> Result<StageOutput<Self::Prepared>, String>;
    fn games(&self, prepared: &Self::Prepared) -> usize;
    fn load_us(&self, prepared: &Self::Prepared) -> u64;
    fn write_snapshot(
        &mut self,
        path: &Path,
        prepared: &Self::Prepared,
    ) -> Result<Vec<(String, String)>, String>;
    fn persist(
        &mut self,
        prepared: Self::Prepared,
        progress: &mut dyn FnMut(&str, &str),
    ) -> Result<BuilderSummary, String>;
    fn write_build_duration(
        &mut self,
        sqlite_path: &Path,
        elapsed: Duration,
    ) -> Result<u64, String>;
}

fn run_with_backend<B: BuilderBackend>(
    operation: BuilderOperation,
    config: BuilderConfig,
    backend: &mut B,
    emit: &mut impl FnMut(CatalogBuilderEvent),
) -> Result<(), String> {
    let run_started = Instant::now();
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
            CheckDecision::Changed(detail) => {
                emit(CatalogBuilderEvent::Changed { protocol, detail });
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

    // Both creation and explicit rebuild own the dedicated full-screen catalog
    // UI. Keep the coordinator eligible for both CPUs until CatalogReady; only
    // checks and post-ready persistence belong to the background policy.
    apply_runtime_thread_policy(RuntimeThreadRole::CatalogForeground);
    let scanned = {
        let protocol_output = RefCell::new(&mut *emit);
        let mut scan_progress = |title: &str, detail: &str| {
            (protocol_output.borrow_mut())(CatalogBuilderEvent::Progress {
                protocol,
                title: title.into(),
                detail: detail.into(),
            });
        };
        backend.scan(&mut scan_progress, &mut |system_id| {
            (protocol_output.borrow_mut())(CatalogBuilderEvent::SystemDiscovered {
                protocol,
                system_id,
            });
        })
    }
    .map_err(|error| fail(protocol, "scan", error, emit))?;
    emit_timings(protocol, scanned.timings, emit);
    let prepared = backend
        .prepare(scanned.value)
        .map_err(|error| fail(protocol, "prepare-catalog", error, emit))?;
    emit_timings(protocol, prepared.timings, emit);
    let games = backend.games(&prepared.value);
    let load_us = backend.load_us(&prepared.value);
    let snapshot_path = config.snapshot_path;
    let mut snapshot_cleanup = SnapshotCleanup::new(snapshot_path.clone());
    let snapshot_started = Instant::now();
    if let Some(parent) = snapshot_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| fail(protocol, "snapshot", error.to_string(), emit))?;
    }
    snapshot_cleanup.arm();
    let snapshot_timings = backend
        .write_snapshot(&snapshot_path, &prepared.value)
        .map_err(|error| fail(protocol, "snapshot", error, emit))?;
    emit_timings(protocol, snapshot_timings, emit);
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
    apply_runtime_thread_policy(RuntimeThreadRole::CatalogWorker);
    let mut progress = |title: &str, detail: &str| {
        emit(CatalogBuilderEvent::Progress {
            protocol,
            title: title.into(),
            detail: detail.into(),
        });
    };
    let summary = backend
        .persist(prepared.value, &mut progress)
        .map_err(|error| fail(protocol, "persist", error, emit))?;
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
    emit(CatalogBuilderEvent::Persisted {
        protocol,
        summary: builder_summary,
    });
    snapshot_cleanup.remove_now();
    emit(CatalogBuilderEvent::Done { protocol });
    Ok(())
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

struct PreparedBuild {
    artifact: library_db::LibraryScanArtifact,
    catalog: ArcadeCatalog,
    load_us: u64,
}

struct SystemBuilderBackend;

impl BuilderBackend for SystemBuilderBackend {
    type Scan = library_db::LibraryRamScanArtifact;
    type Prepared = PreparedBuild;

    fn fresh_cleanup(&mut self) -> Result<usize, String> {
        library_db::remove_default_catalog_artifacts()
    }

    fn check(&mut self) -> Result<CheckOutput, StageFailure> {
        let check = library_db::default_sqlite_catalog_stamp_check()
            .map_err(|error| StageFailure::new("check", error))?;
        let timing_detail = format!(
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
        );
        let decision = if check.unchanged {
            CheckDecision::Unchanged(BuilderSummary::from(
                library_db::default_sqlite_cached_summary(check.check_us)
                    .map_err(|error| StageFailure::new("summary", error))?,
            ))
        } else {
            CheckDecision::Changed(format!(
                "Catalog inputs changed; rebuild required. {}",
                check.drift.detail
            ))
        };
        Ok(CheckOutput {
            timing_detail,
            decision,
        })
    }

    fn scan(
        &mut self,
        progress: &mut dyn FnMut(&str, &str),
        system_discovered: &mut dyn FnMut(String),
    ) -> Result<StageOutput<Self::Scan>, String> {
        let mut scan_events = |event: library_db::LibraryScanEvent| match event {
            library_db::LibraryScanEvent::SystemDiscovered { system_id } => {
                system_discovered(system_id);
            }
        };
        let scanned = library_db::scan_default_library_ram_foreground_with_events(
            Some(progress),
            Some(&mut scan_events),
        )?;
        let stats = scanned.stats();
        let detail = format!(
            "scan_us={} discover_us={} classify_us={} discoveries={} normal_files={} containers={} entries={}",
            stats.scan_us,
            stats.discover_us,
            stats.classify_us,
            stats.discoveries,
            stats.normal_files,
            stats.containers,
            stats.entries
        );
        Ok(StageOutput {
            value: scanned,
            timings: vec![("library_scan_complete".into(), detail)],
        })
    }

    fn prepare(&mut self, scan: Self::Scan) -> Result<StageOutput<Self::Prepared>, String> {
        let root = crate::arcade_catalog::DEFAULT_ARCADE_ROOT;
        let (artifact, catalog, timing) =
            scan.complete_coverage_audit_and_catalog_foreground(root)?;
        let load_us = timing.catalog_us;
        let timings = vec![
            (
                "builder_deferred_audit_stamp".into(),
                format!(
                    "elapsed_us={} audit_us={} stamp_us={} audit_rows={}",
                    timing.audit_stamp_worker_us,
                    timing.audit_us,
                    timing.stamp_us,
                    artifact.stats().audit_rows
                ),
            ),
            (
                "builder_catalog_projection".into(),
                format!("elapsed_us={} games={}", load_us, catalog.len()),
            ),
            (
                "builder_catalog_prepare_overlap".into(),
                format!(
                    "wall_us={} audit_stamp_worker_us={} audit_us={} stamp_us={} catalog_us={} overlapped_us={} mode=scoped-dual-core worker_role={} worker_affinity={}",
                    timing.wall_us,
                    timing.audit_stamp_worker_us,
                    timing.audit_us,
                    timing.stamp_us,
                    timing.catalog_us,
                    timing.overlapped_us,
                    RuntimeThreadRole::CatalogForeground.label(),
                    RuntimeThreadRole::CatalogForeground
                        .default_policy()
                        .affinity
                        .label(),
                ),
            ),
        ];
        Ok(StageOutput {
            value: PreparedBuild {
                artifact,
                catalog,
                load_us,
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
    ) -> Result<Vec<(String, String)>, String> {
        let timing = write_catalog_navigation_snapshot_with_timing(
            path,
            &prepared.catalog,
            prepared.artifact.stamp(),
        )?;
        Ok(vec![(
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
        )])
    }

    fn persist(
        &mut self,
        prepared: Self::Prepared,
        progress: &mut dyn FnMut(&str, &str),
    ) -> Result<BuilderSummary, String> {
        prepared
            .artifact
            .save_default_sqlite_with_catalog_projection(&prepared.catalog, Some(progress))
            .map(BuilderSummary::from)
    }

    fn write_build_duration(
        &mut self,
        sqlite_path: &Path,
        elapsed: Duration,
    ) -> Result<u64, String> {
        catalog_build_record::write_completed_build_duration(sqlite_path, elapsed)
    }
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
    });
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    #[derive(Default)]
    struct FakeBackend {
        fail_stage: Option<&'static str>,
        cleanup_removed: usize,
        check_unchanged: bool,
        calls: Vec<&'static str>,
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
        type Prepared = ();

        fn fresh_cleanup(&mut self) -> Result<usize, String> {
            self.calls.push("fresh-cleanup");
            self.fail("fresh-cleanup")?;
            Ok(self.cleanup_removed)
        }

        fn check(&mut self) -> Result<CheckOutput, StageFailure> {
            self.calls.push("check");
            self.fail("check")
                .map_err(|error| StageFailure::new("check", error))?;
            let decision = if self.check_unchanged {
                CheckDecision::Unchanged(BuilderSummary::default())
            } else {
                CheckDecision::Changed("fixture changed".into())
            };
            Ok(CheckOutput {
                timing_detail: "fixture timing".into(),
                decision,
            })
        }

        fn scan(
            &mut self,
            progress: &mut dyn FnMut(&str, &str),
            system_discovered: &mut dyn FnMut(String),
        ) -> Result<StageOutput<Self::Scan>, String> {
            self.calls.push("scan");
            self.fail("scan")?;
            progress("Scanning", "fixture");
            system_discovered("arcade".into());
            Ok(StageOutput {
                value: (),
                timings: vec![("library_scan_complete".into(), "fixture".into())],
            })
        }

        fn prepare(&mut self, _scan: Self::Scan) -> Result<StageOutput<Self::Prepared>, String> {
            self.calls.push("prepare-catalog");
            self.fail("prepare-catalog")?;
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
        ) -> Result<Vec<(String, String)>, String> {
            self.calls.push("snapshot");
            std::fs::write(path, b"fixture-navigation").map_err(|error| error.to_string())?;
            self.fail("snapshot")?;
            Ok(vec![(
                "builder_navigation_snapshot".into(),
                "fixture".into(),
            )])
        }

        fn persist(
            &mut self,
            _prepared: Self::Prepared,
            progress: &mut dyn FnMut(&str, &str),
        ) -> Result<BuilderSummary, String> {
            self.calls.push("persist");
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
            run_id: format!("fixture-{id}"),
        }
    }

    fn event_name(event: &CatalogBuilderEvent) -> &'static str {
        match event {
            CatalogBuilderEvent::Handshake { .. } => "handshake",
            CatalogBuilderEvent::Progress { .. } => "progress",
            CatalogBuilderEvent::SystemDiscovered { .. } => "system-discovered",
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
        assert_eq!(
            backend.calls,
            [
                "scan",
                "prepare-catalog",
                "snapshot",
                "persist",
                "build-duration"
            ]
        );
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
    fn every_backend_failure_emits_one_staged_failure_and_cleans_snapshot() {
        for stage in [
            "fresh-cleanup",
            "scan",
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
            assert!(!events
                .iter()
                .any(|event| matches!(event, CatalogBuilderEvent::Done { .. })));
        }
    }

    #[test]
    fn real_lock_rejects_overlap_and_is_reusable_after_drop() {
        let config = fixture_config("lock");
        let held = BuilderLock::acquire(&config.lock_path).unwrap();
        let mut backend = FakeBackend::default();
        let mut events = Vec::new();
        assert!(run_with_backend(
            BuilderOperation::Build,
            config.clone(),
            &mut backend,
            &mut |event| events.push(event),
        )
        .is_err());
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
