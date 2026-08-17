// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded, persistent support reports for catalog failures.

use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const REPORT_SCHEMA: &str = "mister-magik-catalog-failure-v1";
const MAX_REPORT_BYTES: usize = 64 * 1024;
const MAX_RETAINED_REPORTS: usize = 5;
const MAX_EVENT_LINES: usize = 48;
const MAX_EVENT_BYTES: usize = 24 * 1024;
const LATEST_FILE: &str = "latest.json";

static REPORT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static REPORTED_FAILURES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static REPORT_WRITER: OnceLock<Option<Sender<(PathBuf, CatalogFailureReport)>>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct CatalogFailureReport {
    pub code: String,
    pub stage: String,
    pub operation: String,
    pub detail: String,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub system_id: Option<String>,
    pub generation: Option<String>,
    pub usable_catalog: bool,
    pub games: usize,
    pub systems: usize,
    pub durable_generation: Option<String>,
    pub recovery_actions: Vec<String>,
}

pub fn latest_relative_path() -> &'static str {
    "diagnostics/catalog/latest.json"
}

pub fn latest_path() -> PathBuf {
    report_dir().join(LATEST_FILE)
}

pub fn schema_versions(detail: &str) -> (Option<String>, Option<String>) {
    (
        token_after(detail, "expected "),
        token_after(detail, "found ").or_else(|| token_after(detail, "actual ")),
    )
}

pub fn enqueue(report: CatalogFailureReport) -> PathBuf {
    let latest = latest_path();
    let dedupe_key = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        report.code, report.stage, report.operation, report.detail
    );
    let inserted = REPORTED_FAILURES
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|mut seen| seen.insert(dedupe_key))
        .unwrap_or(true);
    if !inserted {
        return latest;
    }
    if let Some(writer) = report_writer()
        && let Err(error) = writer.send((report_dir(), report))
    {
        log_report_error(format_args!("catalog failure report queue failed: {error}"));
    }
    latest
}

fn report_writer() -> Option<&'static Sender<(PathBuf, CatalogFailureReport)>> {
    REPORT_WRITER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<(PathBuf, CatalogFailureReport)>();
            match std::thread::Builder::new()
                .name("catalog-failure-report".to_string())
                .spawn(move || {
                    while let Ok((dir, report)) = receiver.recv() {
                        if let Err(error) = write_report(&dir, report) {
                            log_report_error(format_args!(
                                "catalog failure report write failed: {error}"
                            ));
                        }
                    }
                }) {
                Ok(_) => Some(sender),
                Err(error) => {
                    log_report_error(format_args!(
                        "catalog failure report worker failed to start: {error}"
                    ));
                    None
                }
            }
        })
        .as_ref()
}

fn log_report_error(arguments: std::fmt::Arguments<'_>) {
    let _ = crate::fallible_log::stderr_line(arguments);
}

fn report_dir() -> PathBuf {
    mister_magik_catalog::device_layout::current_app_path("diagnostics/catalog")
}

fn write_report(dir: &Path, report: CatalogFailureReport) -> io::Result<PathBuf> {
    let ts_unix_ms = unix_ms();
    let sequence = REPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let report_id = format!(
        "report-catalog-{ts_unix_ms}-{}-{sequence}",
        std::process::id()
    );
    let value = report_value(&report_id, ts_unix_ms, report);
    write_report_value(dir, &report_id, &value)
}

fn report_value(report_id: &str, ts_unix_ms: u128, mut report: CatalogFailureReport) -> Value {
    truncate_string(&mut report.detail, 8192);
    json!({
        "schema": REPORT_SCHEMA,
        "report_id": report_id,
        "ts_unix_ms": ts_unix_ms,
        "pid": std::process::id(),
        "build": crate::build_identity::BuildIdentity::current(),
        "failure": {
            "code": report.code,
            "stage": report.stage,
            "operation": report.operation,
            "detail": report.detail,
            "expected": report.expected,
            "actual": report.actual,
            "system_id": report.system_id,
            "generation": report.generation,
        },
        "catalog": {
            "usable": report.usable_catalog,
            "games": report.games,
            "systems": report.systems,
            "durable_generation": report.durable_generation,
        },
        "recovery": {
            "offered": report.recovery_actions,
        },
        "events": catalog_event_history(),
    })
}

fn write_report_value(dir: &Path, report_id: &str, value: &Value) -> io::Result<PathBuf> {
    let mut encoded = serde_json::to_vec_pretty(value)?;
    encoded.push(b'\n');
    if encoded.len() > MAX_REPORT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "catalog failure report exceeds 64 KiB",
        ));
    }
    fs::create_dir_all(dir)?;
    let report_path = dir.join(format!("{report_id}.json"));
    let report_tmp = dir.join(format!(".{report_id}.json.tmp"));
    let latest_path = dir.join(LATEST_FILE);
    let latest_tmp = dir.join(".latest.json.tmp");
    write_file_sync(&report_tmp, &encoded)?;
    fs::rename(&report_tmp, &report_path)?;
    write_file_sync(&latest_tmp, &encoded)?;
    fs::rename(&latest_tmp, &latest_path)?;
    sync_parent_dir(&latest_path);
    prune_reports(dir, report_id)?;
    Ok(report_path)
}

fn prune_reports(dir: &Path, latest_report_id: &str) -> io::Result<()> {
    let mut reports = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("report-catalog-") && name.ends_with(".json"))
        })
        .collect::<Vec<_>>();
    reports.sort();
    let keep_latest = format!("{latest_report_id}.json");
    while reports.len() > MAX_RETAINED_REPORTS {
        let remove_index = reports
            .iter()
            .position(|path| path.file_name().and_then(|name| name.to_str()) != Some(&keep_latest))
            .unwrap_or(0);
        fs::remove_file(reports.remove(remove_index))?;
    }
    Ok(())
}

fn catalog_event_history() -> Vec<String> {
    let Ok(text) = fs::read_to_string("/tmp/mister-magik/events.jsonl") else {
        return Vec::new();
    };
    let mut bytes = 0usize;
    let mut lines = text
        .lines()
        .rev()
        .filter(|line| line.contains("catalog") || line.contains("library"))
        .filter_map(|line| {
            if bytes.saturating_add(line.len()) > MAX_EVENT_BYTES {
                return None;
            }
            bytes += line.len();
            Some(line.to_string())
        })
        .take(MAX_EVENT_LINES)
        .collect::<Vec<_>>();
    lines.reverse();
    lines
}

fn write_file_sync(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn sync_parent_dir(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(dir) = fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }
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

fn token_after(value: &str, marker: &str) -> Option<String> {
    let start = value.find(marker)? + marker.len();
    let token = value[start..]
        .split(|character: char| character.is_whitespace() || matches!(character, ',' | ';'))
        .next()?
        .trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '-');
    (!token.is_empty()).then(|| token.to_string())
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(index: usize) -> CatalogFailureReport {
        CatalogFailureReport {
            code: "publish_failed".to_string(),
            stage: "persist".to_string(),
            operation: format!("rebuild-{index}"),
            detail: "disk full".to_string(),
            expected: None,
            actual: None,
            system_id: None,
            generation: None,
            usable_catalog: true,
            games: 919,
            systems: 24,
            durable_generation: Some("old".to_string()),
            recovery_actions: vec!["continue".to_string(), "full_rebuild".to_string()],
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mister-magik-{name}-{}-{}",
            std::process::id(),
            REPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn report_is_atomic_bounded_and_retains_five() {
        let dir = temp_dir("catalog-failure-report");
        for index in 0..7 {
            write_report(&dir, fixture(index)).unwrap();
        }
        let latest = fs::read(dir.join(LATEST_FILE)).unwrap();
        assert!(latest.len() <= MAX_REPORT_BYTES);
        let value = serde_json::from_slice::<Value>(&latest).unwrap();
        assert_eq!(value["schema"], REPORT_SCHEMA);
        assert_eq!(
            value["build"]["build_number"],
            crate::build_identity::BuildIdentity::current().build_number
        );
        assert_eq!(
            value["build"]["source_revision"],
            crate::build_identity::BuildIdentity::current().source_revision
        );
        let retained = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("report-catalog-"))
            })
            .count();
        assert_eq!(retained, MAX_RETAINED_REPORTS);
        assert!(
            fs::read_dir(&dir)
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn oversized_detail_is_truncated_before_encoding() {
        let mut report = fixture(0);
        report.detail = "x".repeat(MAX_REPORT_BYTES * 2);
        let value = report_value("report-catalog-test", 0, report);
        let encoded = serde_json::to_vec(&value).unwrap();
        assert!(encoded.len() < MAX_REPORT_BYTES);
        assert!(
            value["failure"]["detail"]
                .as_str()
                .unwrap()
                .ends_with("...")
        );
    }

    #[test]
    fn schema_context_is_extracted_when_available() {
        assert_eq!(
            schema_versions("unsupported shard schema: expected 3, found 2"),
            (Some("3".to_string()), Some("2".to_string()))
        );
    }
}
