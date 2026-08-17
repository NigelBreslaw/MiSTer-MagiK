// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded, persistent progress episodes for catalog workers that may never fail.

use serde::Serialize;
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const REPORT_SCHEMA: &str = "mister-magik-catalog-progress-v1";
const REPORT_PREFIX: &str = "progress-catalog-";
const LATEST_FILE: &str = "progress-latest.json";
const REPORT_QUEUE_CAPACITY: usize = 16;
const MAX_REPORT_BYTES: usize = 96 * 1024;
const MAX_RETAINED_REPORTS: usize = 24;
const MAX_RETAINED_BYTES: u64 = 2 * 1024 * 1024;
const RETENTION_MS: u128 = 48 * 60 * 60 * 1000;
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(120);
const STALL_AFTER_ACTIVE: Duration = Duration::from_secs(5 * 60);
const STALL_REPEAT_INTERVAL: Duration = Duration::from_secs(5 * 60);
const MAX_JSON_INPUT_BYTES: u64 = 512 * 1024;
const MAX_LOG_BYTES: usize = 24 * 1024;
const MAX_LOG_LINES: usize = 96;

static REPORT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static REPORT_WRITER: OnceLock<Option<SyncSender<(PathBuf, String, CatalogProgressEvidence)>>> =
    OnceLock::new();

#[derive(Clone, Debug, Serialize)]
pub struct CatalogProgressEvidence {
    pub state: String,
    pub operation: String,
    pub execution_mode: String,
    pub cooperative_policy: String,
    pub root: String,
    pub phase: String,
    pub detail: String,
    pub percent: i32,
    pub activity_kind: String,
    pub activity_count: u64,
    pub wall_elapsed_ms: u64,
    pub active_elapsed_ms: u64,
    pub inactive_elapsed_ms: u64,
    pub intentionally_paused: bool,
    pub worker_running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_target: Option<mister_magik_catalog::builder_protocol::CatalogScanTargetProgress>,
}

pub struct CatalogProgressMonitor {
    episode_id: Option<String>,
    operation: String,
    execution_mode: String,
    cooperative_policy: String,
    root: String,
    phase: String,
    detail: String,
    percent: i32,
    activity_kind: String,
    activity_count: u64,
    started_at: Instant,
    last_tick: Instant,
    last_persisted_at: Instant,
    active_elapsed: Duration,
    inactive_elapsed: Duration,
    last_tick_active: bool,
    stall_reported: bool,
    scan_target: Option<mister_magik_catalog::builder_protocol::CatalogScanTargetProgress>,
}

impl CatalogProgressMonitor {
    pub fn new(now: Instant) -> Self {
        Self {
            episode_id: None,
            operation: String::new(),
            execution_mode: String::new(),
            cooperative_policy: String::new(),
            root: String::new(),
            phase: String::new(),
            detail: String::new(),
            percent: -1,
            activity_kind: String::new(),
            activity_count: 0,
            started_at: now,
            last_tick: now,
            last_persisted_at: now,
            active_elapsed: Duration::ZERO,
            inactive_elapsed: Duration::ZERO,
            last_tick_active: false,
            stall_reported: false,
            scan_target: None,
        }
    }

    pub fn start(
        &mut self,
        root: String,
        operation: &str,
        execution_mode: &str,
        now: Instant,
    ) -> CatalogProgressEvidence {
        self.episode_id = Some(new_episode_id());
        self.operation = operation.to_string();
        self.execution_mode = execution_mode.to_string();
        self.cooperative_policy = if execution_mode == "foreground_exclusive" {
            "unrestricted"
        } else {
            "continuous_cpu0"
        }
        .to_string();
        self.root = root;
        self.phase = "starting".to_string();
        self.detail.clear();
        self.percent = -1;
        self.activity_kind = "worker-start".to_string();
        self.activity_count = 0;
        self.started_at = now;
        self.last_tick = now;
        self.last_persisted_at = now;
        self.active_elapsed = Duration::ZERO;
        self.inactive_elapsed = Duration::ZERO;
        self.last_tick_active = execution_mode == "foreground_exclusive";
        self.stall_reported = false;
        self.scan_target = None;
        self.evidence("running", true)
    }

    pub fn note_activity(
        &mut self,
        activity_kind: &str,
        phase: &str,
        detail: &str,
        percent: i32,
        now: Instant,
    ) -> Option<CatalogProgressEvidence> {
        self.episode_id.as_ref()?;
        let recovered_from_stall = self.stall_reported;
        self.advance(now);
        self.activity_kind = activity_kind.to_string();
        self.phase = phase.to_string();
        self.detail = detail.to_string();
        truncate_string(&mut self.detail, 8 * 1024);
        self.percent = percent;
        self.activity_count = self.activity_count.saturating_add(1);
        self.inactive_elapsed = Duration::ZERO;
        self.stall_reported = false;
        recovered_from_stall.then(|| {
            self.last_persisted_at = now;
            self.evidence("running", true)
        })
    }

    pub fn tick(
        &mut self,
        worker_running: bool,
        background_work_allowed: bool,
        now: Instant,
    ) -> Option<CatalogProgressEvidence> {
        self.episode_id.as_ref()?;
        self.advance(now);
        self.last_tick_active = worker_running
            && (self.execution_mode == "foreground_exclusive" || background_work_allowed);

        let stalled = worker_running && self.inactive_elapsed >= STALL_AFTER_ACTIVE;
        let persist_due = now.saturating_duration_since(self.last_persisted_at)
            >= if stalled && self.stall_reported {
                STALL_REPEAT_INTERVAL
            } else {
                SNAPSHOT_INTERVAL
            };
        if !persist_due {
            return None;
        }
        self.last_persisted_at = now;
        if stalled {
            self.stall_reported = true;
        }
        Some(self.evidence(
            if stalled {
                "stalled"
            } else if self.last_tick_active {
                "running"
            } else {
                "paused"
            },
            worker_running,
        ))
    }

    pub fn note_scan_target(
        &mut self,
        target: mister_magik_catalog::builder_protocol::CatalogScanTargetProgress,
    ) {
        self.execution_mode.clone_from(&target.execution_mode);
        self.cooperative_policy
            .clone_from(&target.cooperative_policy);
        self.scan_target = Some(target);
    }

    pub fn finish(
        &mut self,
        state: &str,
        detail: &str,
        now: Instant,
    ) -> Option<CatalogProgressEvidence> {
        self.episode_id.as_ref()?;
        self.advance(now);
        self.activity_kind = "worker-finish".to_string();
        self.phase = state.to_string();
        self.detail = detail.to_string();
        truncate_string(&mut self.detail, 8 * 1024);
        self.last_tick_active = false;
        let evidence = self.evidence(state, false);
        self.episode_id = None;
        Some(evidence)
    }

    pub fn episode_id(&self) -> Option<&str> {
        self.episode_id.as_deref()
    }

    fn advance(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;
        if self.last_tick_active {
            self.active_elapsed = self.active_elapsed.saturating_add(elapsed);
            self.inactive_elapsed = self.inactive_elapsed.saturating_add(elapsed);
        }
    }

    fn evidence(&self, state: &str, worker_running: bool) -> CatalogProgressEvidence {
        CatalogProgressEvidence {
            state: state.to_string(),
            operation: self.operation.clone(),
            execution_mode: self.execution_mode.clone(),
            cooperative_policy: self.cooperative_policy.clone(),
            root: self.root.clone(),
            phase: self.phase.clone(),
            detail: self.detail.clone(),
            percent: self.percent,
            activity_kind: self.activity_kind.clone(),
            activity_count: self.activity_count,
            wall_elapsed_ms: duration_ms(self.last_tick.saturating_duration_since(self.started_at)),
            active_elapsed_ms: duration_ms(self.active_elapsed),
            inactive_elapsed_ms: duration_ms(self.inactive_elapsed),
            intentionally_paused: worker_running && !self.last_tick_active,
            worker_running,
            scan_target: self.scan_target.clone(),
        }
    }
}

pub fn enqueue(episode_id: impl Into<String>, evidence: CatalogProgressEvidence) -> PathBuf {
    let latest = latest_path();
    if let Some(writer) = report_writer() {
        match writer.try_send((report_dir(), episode_id.into(), evidence)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                log_report_error(format_args!("catalog progress report queue full"));
            }
            Err(TrySendError::Disconnected(_)) => {
                log_report_error(format_args!("catalog progress report queue disconnected"));
            }
        }
    }
    latest
}

pub fn latest_relative_path() -> &'static str {
    "diagnostics/catalog/progress-latest.json"
}

pub fn latest_path() -> PathBuf {
    report_dir().join(LATEST_FILE)
}

fn report_writer() -> Option<&'static SyncSender<(PathBuf, String, CatalogProgressEvidence)>> {
    REPORT_WRITER
        .get_or_init(|| {
            let (sender, receiver) =
                mpsc::sync_channel::<(PathBuf, String, CatalogProgressEvidence)>(
                    REPORT_QUEUE_CAPACITY,
                );
            match std::thread::Builder::new()
                .name("catalog-progress-report".to_string())
                .spawn(move || {
                    mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                        mister_magik_catalog::runtime_thread::RuntimeThreadRole::CatalogWorker,
                    );
                    while let Ok((dir, episode_id, evidence)) = receiver.recv() {
                        if let Err(error) = write_report(&dir, &episode_id, evidence, unix_ms()) {
                            log_report_error(format_args!(
                                "catalog progress report write failed: {error}"
                            ));
                        }
                    }
                }) {
                Ok(_) => Some(sender),
                Err(error) => {
                    log_report_error(format_args!(
                        "catalog progress report worker failed to start: {error}"
                    ));
                    None
                }
            }
        })
        .as_ref()
}

fn report_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("MISTER_CATALOG_DIAGNOSTICS_DIR") {
        return PathBuf::from(path);
    }
    mister_magik_catalog::device_layout::current_app_path("diagnostics/catalog")
}

fn write_report(
    dir: &Path,
    episode_id: &str,
    evidence: CatalogProgressEvidence,
    now_ms: u128,
) -> io::Result<PathBuf> {
    let value = report_value(episode_id, now_ms, evidence);
    let mut encoded = serde_json::to_vec_pretty(&value)?;
    encoded.push(b'\n');
    if encoded.len() > MAX_REPORT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "catalog progress report exceeds 96 KiB",
        ));
    }
    fs::create_dir_all(dir)?;
    let report_path = dir.join(format!("{episode_id}.json"));
    write_atomic(&report_path, &encoded)?;
    write_atomic(&dir.join(LATEST_FILE), &encoded)?;
    sync_parent_dir(&report_path);
    prune_reports(dir, episode_id, now_ms)?;
    Ok(report_path)
}

fn report_value(episode_id: &str, now_ms: u128, evidence: CatalogProgressEvidence) -> Value {
    json!({
        "schema": REPORT_SCHEMA,
        "episode_id": episode_id,
        "updated_unix_ms": now_ms,
        "pid": std::process::id(),
        "build": crate::build_identity::BuildIdentity::current(),
        "progress": evidence,
        "files": {
            "build_progress": file_snapshot(
                &mister_magik_catalog::catalog_config::default_build_progress_path()
            ),
            "catalog_state": file_snapshot(
                &mister_magik_catalog::catalog_state::default_path()
            ),
        },
        "journal": build_progress_summary(
            &mister_magik_catalog::catalog_config::default_build_progress_path()
        ),
        "snapshots": {
            "runtime": read_json_snapshot("/tmp/mister-magik/status.json"),
            "main": read_json_snapshot("/tmp/mister-magik/main-status.json"),
        },
        "events": filtered_event_tail(
            "/tmp/mister-magik/events.jsonl",
            std::process::id()
        ),
        "application_log": filtered_application_log_tail("/tmp/mister-magik-slint.log"),
    })
}

fn build_progress_summary(path: &Path) -> Value {
    match mister_magik_catalog::build_progress::read_summary(path) {
        Ok(Some(summary)) => json!({
            "available": true,
            "summary": summary,
        }),
        Ok(None) => json!({
            "available": false,
        }),
        Err(error) => json!({
            "available": false,
            "error": error,
        }),
    }
}

fn file_snapshot(path: &Path) -> Value {
    match fs::metadata(path) {
        Ok(metadata) => json!({
            "path": path,
            "exists": true,
            "bytes": metadata.len(),
            "modified_unix_ms": metadata.modified().ok().map(system_time_ms),
        }),
        Err(error) => json!({
            "path": path,
            "exists": false,
            "error_kind": format!("{:?}", error.kind()),
        }),
    }
}

fn read_json_snapshot(path: &str) -> Option<Value> {
    let file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len > MAX_JSON_INPUT_BYTES {
        return Some(json!({
            "projection_error": "snapshot exceeds bounded JSON input",
            "bytes": len,
        }));
    }
    let value: Value = serde_json::from_reader(file).ok()?;
    let Value::Object(source) = value else {
        return None;
    };
    let keys = [
        "schema",
        "ts_unix_ms",
        "pid",
        "mode",
        "scene",
        "screen",
        "launcher_state",
        "launcher_pid",
        "catalog_ready",
        "catalog_games",
        "catalog_partial",
        "catalog_refresh_policy",
        "catalog_worker_enabled",
        "catalog_worker_running",
        "catalog_progress_report",
        "last_frame_ms_ago",
        "present_backend",
        "present_status",
    ];
    let projected = keys
        .into_iter()
        .filter_map(|key| {
            source
                .get(key)
                .cloned()
                .map(|value| (key.to_string(), value))
        })
        .collect();
    Some(Value::Object(projected))
}

fn filtered_event_tail(path: &str, pid: u32) -> Vec<String> {
    let Ok(bytes) = read_bounded(path, MAX_LOG_BYTES) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text
        .lines()
        .rev()
        .filter(|line| {
            let Ok(event) = serde_json::from_str::<Value>(line) else {
                return false;
            };
            if event.get("pid").and_then(Value::as_u64) != Some(u64::from(pid)) {
                return false;
            }
            let line = line.to_ascii_lowercase();
            if line.contains("screenshot") && !line.contains("error") && !line.contains("fail") {
                return false;
            }
            ["catalog", "library", "scan", "builder", "sqlite"]
                .iter()
                .any(|token| line.contains(token))
        })
        .take(MAX_LOG_LINES)
        .map(|line| {
            let mut line = line.to_string();
            truncate_string(&mut line, 2_048);
            line
        })
        .collect::<Vec<_>>();
    lines.reverse();
    lines
}

fn filtered_application_log_tail(path: &str) -> Vec<String> {
    let Ok(bytes) = read_bounded(path, MAX_LOG_BYTES) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut previous: Option<String> = None;
    let mut lines = Vec::new();
    for line in text.lines().rev() {
        let lower = line.to_ascii_lowercase();
        if !["catalog", "library", "scan", "builder", "sqlite"]
            .iter()
            .any(|token| lower.contains(token))
            || (lower.contains("screenshot") && !lower.contains("error") && !lower.contains("fail"))
            || previous.as_deref() == Some(line)
        {
            continue;
        }
        let mut line = line.to_string();
        truncate_string(&mut line, 2_048);
        previous = Some(line.clone());
        lines.push(line);
        if lines.len() >= MAX_LOG_LINES {
            break;
        }
    }
    lines.reverse();
    lines
}

fn read_bounded(path: &str, limit: usize) -> io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(limit as u64);
    let mut bytes = Vec::with_capacity(limit.min(len as usize));
    let mut reader = io::BufReader::new(file);
    use std::io::Seek;
    reader.seek(io::SeekFrom::Start(start))?;
    reader.take(limit as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    Ok(())
}

fn sync_parent_dir(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(dir) = fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }
}

fn prune_reports(dir: &Path, latest_episode_id: &str, now_ms: u128) -> io::Result<()> {
    let mut reports = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if !name.starts_with(REPORT_PREFIX) || !name.ends_with(".json") {
                return None;
            }
            let timestamp = report_timestamp_ms(name);
            let metadata = entry.metadata().ok()?;
            Some((path, timestamp, metadata.len()))
        })
        .collect::<Vec<_>>();
    reports.sort_by_key(|(_, timestamp, _)| *timestamp);
    let keep_latest = format!("{latest_episode_id}.json");

    if valid_retention_clock(now_ms, &reports) {
        let cutoff = now_ms.saturating_sub(RETENTION_MS);
        remove_matching(&mut reports, &keep_latest, |(_, timestamp, _)| {
            timestamp.is_some_and(|timestamp| timestamp < cutoff)
        });
    }
    while reports.len() > MAX_RETAINED_REPORTS {
        remove_oldest(&mut reports, &keep_latest)?;
    }
    while reports.iter().map(|(_, _, bytes)| *bytes).sum::<u64>() > MAX_RETAINED_BYTES {
        if reports.len() <= 1 {
            break;
        }
        remove_oldest(&mut reports, &keep_latest)?;
    }
    Ok(())
}

fn remove_matching(
    reports: &mut Vec<(PathBuf, Option<u128>, u64)>,
    keep_latest: &str,
    predicate: impl Fn(&(PathBuf, Option<u128>, u64)) -> bool,
) {
    let mut index = 0;
    while index < reports.len() {
        let keep = reports[index].0.file_name().and_then(|name| name.to_str()) == Some(keep_latest);
        if !keep && predicate(&reports[index]) && fs::remove_file(&reports[index].0).is_ok() {
            reports.remove(index);
        } else {
            index += 1;
        }
    }
}

fn remove_oldest(
    reports: &mut Vec<(PathBuf, Option<u128>, u64)>,
    keep_latest: &str,
) -> io::Result<()> {
    let index = reports
        .iter()
        .position(|(path, _, _)| {
            path.file_name().and_then(|name| name.to_str()) != Some(keep_latest)
        })
        .unwrap_or(0);
    fs::remove_file(&reports[index].0)?;
    reports.remove(index);
    Ok(())
}

fn valid_retention_clock(now_ms: u128, reports: &[(PathBuf, Option<u128>, u64)]) -> bool {
    now_ms >= RETENTION_MS
        && reports
            .iter()
            .filter_map(|(_, timestamp, _)| *timestamp)
            .all(|timestamp| timestamp <= now_ms.saturating_add(24 * 60 * 60 * 1000))
}

fn report_timestamp_ms(name: &str) -> Option<u128> {
    name.strip_prefix(REPORT_PREFIX)?
        .split('-')
        .next()?
        .parse()
        .ok()
}

fn new_episode_id() -> String {
    format!(
        "{REPORT_PREFIX}{}-{}-{}",
        unix_ms(),
        std::process::id(),
        REPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn unix_ms() -> u128 {
    system_time_ms(SystemTime::now())
}

fn system_time_ms(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

fn truncate_string(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let end = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= max_bytes.saturating_sub(3))
        .last()
        .unwrap_or(0);
    value.truncate(end);
    value.push_str("...");
}

fn log_report_error(arguments: std::fmt::Arguments<'_>) {
    let _ = crate::fallible_log::stderr_line(arguments);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mister-magik-catalog-report-{name}-{}-{}",
            std::process::id(),
            REPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn oversized_runtime_status_is_fully_parsed_then_projected() {
        let path = temp_file("runtime-status.json");
        let padding = "x".repeat(32 * 1024);
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "schema": "mister-magik-slint-status-v1",
                "pid": 42,
                "catalog_ready": true,
                "catalog_games": 906,
                "large_unrelated_field": padding,
            }))
            .unwrap(),
        )
        .unwrap();

        let snapshot = read_json_snapshot(path.to_str().unwrap()).unwrap();
        assert_eq!(snapshot["pid"], 42);
        assert_eq!(snapshot["catalog_games"], 906);
        assert!(snapshot.get("large_unrelated_field").is_none());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn event_tail_excludes_previous_launcher_pids() {
        let path = temp_file("events.jsonl");
        fs::write(
            &path,
            "{\"pid\":41,\"event\":\"catalog_old\"}\n\
             {\"pid\":42,\"event\":\"catalog_current\"}\n\
             {\"pid\":42,\"event\":\"screenshot_media_catalog_ensure\"}\n",
        )
        .unwrap();

        let events = filtered_event_tail(path.to_str().unwrap(), 42);
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("catalog_current"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn monitor_ignores_intentional_background_pauses() {
        let start = Instant::now();
        let mut monitor = CatalogProgressMonitor::new(start);
        monitor.start(
            "/media/fat".to_string(),
            "build",
            "background_interactive",
            start,
        );
        assert!(
            monitor
                .tick(true, false, start + Duration::from_secs(10 * 60))
                .is_some_and(|report| report.state == "paused")
        );
        assert_eq!(monitor.inactive_elapsed, Duration::ZERO);
    }

    #[test]
    fn monitor_reports_and_recovers_from_active_stall() {
        let start = Instant::now();
        let mut monitor = CatalogProgressMonitor::new(start);
        monitor.start(
            "/media/fat".to_string(),
            "build",
            "foreground_exclusive",
            start,
        );
        let stalled = monitor
            .tick(true, true, start + STALL_AFTER_ACTIVE)
            .unwrap();
        assert_eq!(stalled.state, "stalled");
        assert_eq!(stalled.inactive_elapsed_ms, duration_ms(STALL_AFTER_ACTIVE));

        let recovered_at = start + STALL_AFTER_ACTIVE + Duration::from_secs(1);
        let recovered = monitor.note_activity(
            "progress",
            "Scanning",
            "Games found: 500000",
            50,
            recovered_at,
        );
        assert_eq!(monitor.inactive_elapsed, Duration::ZERO);
        assert!(!monitor.stall_reported);
        assert!(recovered.is_some_and(|report| report.state == "running"));
    }

    #[test]
    fn completed_episode_is_not_reused() {
        let start = Instant::now();
        let mut monitor = CatalogProgressMonitor::new(start);
        monitor.start(
            "/media/fat".to_string(),
            "build",
            "foreground_exclusive",
            start,
        );
        let first = monitor.episode_id().unwrap().to_string();
        assert!(
            monitor
                .finish("completed", "games=500000", start + Duration::from_secs(1))
                .is_some()
        );
        assert!(monitor.episode_id().is_none());
        monitor.start(
            "/media/fat".to_string(),
            "build",
            "foreground_exclusive",
            start + Duration::from_secs(2),
        );
        assert_ne!(monitor.episode_id().unwrap(), first);
    }

    #[test]
    fn reports_are_atomic_bounded_and_retained() {
        let dir = std::env::temp_dir().join(format!(
            "mister-magik-catalog-progress-{}-{}",
            std::process::id(),
            REPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        for index in 0..(MAX_RETAINED_REPORTS + 3) {
            let episode_id = format!("{REPORT_PREFIX}{}-1-{index}", 1_000 + index);
            write_report(
                &dir,
                &episode_id,
                CatalogProgressEvidence {
                    state: "running".to_string(),
                    operation: "build".to_string(),
                    execution_mode: "foreground_exclusive".to_string(),
                    cooperative_policy: "unrestricted".to_string(),
                    root: "/media/fat".to_string(),
                    phase: "Scanning".to_string(),
                    detail: "Games found".to_string(),
                    percent: 50,
                    activity_kind: "progress".to_string(),
                    activity_count: index as u64,
                    wall_elapsed_ms: 1,
                    active_elapsed_ms: 1,
                    inactive_elapsed_ms: 0,
                    intentionally_paused: false,
                    worker_running: true,
                    scan_target: None,
                },
                0,
            )
            .unwrap();
        }
        let reports = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(REPORT_PREFIX))
            })
            .count();
        assert!(reports <= MAX_RETAINED_REPORTS);
        assert!(dir.join(LATEST_FILE).exists());
        let latest: Value =
            serde_json::from_slice(&fs::read(dir.join(LATEST_FILE)).unwrap()).unwrap();
        assert_eq!(
            latest["build"]["build_number"],
            crate::build_identity::BuildIdentity::current().build_number
        );
        assert_eq!(
            latest["build"]["source_revision"],
            crate::build_identity::BuildIdentity::current().source_revision
        );
        assert!(
            fs::read_dir(&dir)
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp-"))
        );
        let _ = fs::remove_dir_all(dir);
    }
}
