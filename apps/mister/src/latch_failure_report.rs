// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded, persistent support reports for latch presentation failures.

use mister_magik_mister_runtime::latch_readiness::LatchFailureEvidence;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const REPORT_SCHEMA: &str = "mister-magik-latch-failure-report-v1";
const REPORT_PREFIX: &str = "report-latch-";
const LATEST_FILE: &str = "latest.json";
const RETENTION_MS: u128 = 48 * 60 * 60 * 1_000;
const MAX_RETAINED_REPORTS: usize = 48;
const MAX_TOTAL_REPORT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_REPORT_BYTES: usize = 128 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 12 * 1024;
const MAX_LOG_BYTES: usize = 16 * 1024;
const MAX_LOG_LINES: usize = 96;

static REPORT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static EPISODE_ID: OnceLock<String> = OnceLock::new();
static LAST_EVIDENCE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static REPORT_WRITER: OnceLock<Option<Sender<(PathBuf, String, LatchFailureEvidence)>>> =
    OnceLock::new();

pub fn latest_relative_path() -> &'static str {
    "diagnostics/latch/latest.json"
}

pub fn latest_path() -> PathBuf {
    report_dir().join(LATEST_FILE)
}

pub fn enqueue(evidence: LatchFailureEvidence) -> PathBuf {
    let latest = latest_path();
    let dedupe_key = serde_json::to_string(&evidence).unwrap_or_default();
    let changed = LAST_EVIDENCE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map(|mut previous| {
            if previous.as_deref() == Some(&dedupe_key) {
                false
            } else {
                *previous = Some(dedupe_key);
                true
            }
        })
        .unwrap_or(true);
    if !changed {
        return latest;
    }

    let episode_id = EPISODE_ID
        .get_or_init(|| {
            format!(
                "{REPORT_PREFIX}{}-{}-{}",
                unix_ms(),
                std::process::id(),
                REPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            )
        })
        .clone();
    if let Some(writer) = report_writer()
        && let Err(error) = writer.send((report_dir(), episode_id, evidence))
    {
        log_report_error(format_args!("latch failure report queue failed: {error}"));
    }
    latest
}

fn report_writer() -> Option<&'static Sender<(PathBuf, String, LatchFailureEvidence)>> {
    REPORT_WRITER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<(PathBuf, String, LatchFailureEvidence)>();
            match std::thread::Builder::new()
                .name("latch-failure-report".to_string())
                .spawn(move || {
                    while let Ok((dir, episode_id, evidence)) = receiver.recv() {
                        if let Err(error) = write_report(&dir, &episode_id, evidence, unix_ms()) {
                            log_report_error(format_args!(
                                "latch failure report write failed: {error}"
                            ));
                        }
                    }
                }) {
                Ok(_) => Some(sender),
                Err(error) => {
                    log_report_error(format_args!(
                        "latch failure report worker failed to start: {error}"
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
    mister_magik_catalog::device_layout::current_app_path("diagnostics/latch")
}

fn write_report(
    dir: &Path,
    episode_id: &str,
    evidence: LatchFailureEvidence,
    now_ms: u128,
) -> io::Result<PathBuf> {
    let value = report_value(episode_id, now_ms, evidence);
    let mut encoded = serde_json::to_vec_pretty(&value)?;
    encoded.push(b'\n');
    if encoded.len() > MAX_REPORT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "latch failure report exceeds 128 KiB",
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

fn report_value(episode_id: &str, now_ms: u128, mut evidence: LatchFailureEvidence) -> Value {
    for detail in [&mut evidence.detail, &mut evidence.latest_detail] {
        truncate_string(detail, 8 * 1024);
    }
    json!({
        "schema": REPORT_SCHEMA,
        "episode_id": episode_id,
        "updated_unix_ms": now_ms,
        "pid": std::process::id(),
        "build": {
            "package_version": env!("CARGO_PKG_VERSION"),
            "arch": std::env::consts::ARCH,
        },
        "failure": evidence,
        "snapshots": {
            "runtime": read_json_snapshot("/tmp/mister-magik/status.json"),
            "main": read_json_snapshot("/tmp/mister-magik/main-status.json"),
            "readiness": read_json_snapshot("/tmp/mister-magik/latch-readiness.json"),
        },
        "events": filtered_log_tail("/tmp/mister-magik/events.jsonl"),
        "application_log": filtered_log_tail("/tmp/mister-magik-slint.log"),
    })
}

fn read_json_snapshot(path: &str) -> Option<Value> {
    let bytes = read_bounded(path, MAX_SNAPSHOT_BYTES).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn filtered_log_tail(path: &str) -> Vec<String> {
    let Ok(bytes) = read_bounded(path, MAX_LOG_BYTES) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text
        .lines()
        .rev()
        .filter(|line| {
            let line = line.to_ascii_lowercase();
            [
                "latch", "fpga", "display", "compatib", "catalog", "library", "scan",
            ]
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
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("report.json");
    let tmp = path.with_file_name(format!(".{file_name}.tmp"));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(tmp, path)
}

fn prune_reports(dir: &Path, active_episode_id: &str, now_ms: u128) -> io::Result<()> {
    let active_file = format!("{active_episode_id}.json");
    let mut reports = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            (name.starts_with(REPORT_PREFIX) && name.ends_with(".json")).then(|| {
                let timestamp = report_timestamp(name).unwrap_or(0);
                let bytes = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                (path, timestamp, bytes)
            })
        })
        .collect::<Vec<_>>();

    reports.sort_by_key(|(_, timestamp, _)| *timestamp);
    let expired = reports
        .iter()
        .filter(|(path, timestamp, _)| {
            path.file_name().and_then(|name| name.to_str()) != Some(&active_file)
                && *timestamp > 0
                && now_ms.saturating_sub(*timestamp) > RETENTION_MS
                && now_ms >= *timestamp
        })
        .map(|(path, _, _)| path.clone())
        .collect::<HashSet<_>>();
    for path in &expired {
        fs::remove_file(path)?;
    }
    reports.retain(|(path, _, _)| !expired.contains(path));

    let mut total_bytes = reports.iter().map(|(_, _, bytes)| *bytes).sum::<u64>();
    while reports.len() > MAX_RETAINED_REPORTS || total_bytes > MAX_TOTAL_REPORT_BYTES {
        let Some(index) = reports.iter().position(|(path, _, _)| {
            path.file_name().and_then(|name| name.to_str()) != Some(&active_file)
        }) else {
            break;
        };
        let (path, _, bytes) = reports.remove(index);
        fs::remove_file(path)?;
        total_bytes = total_bytes.saturating_sub(bytes);
    }
    Ok(())
}

fn report_timestamp(name: &str) -> Option<u128> {
    name.strip_prefix(REPORT_PREFIX)?
        .split('-')
        .next()?
        .parse()
        .ok()
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

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mister_magik_mister_runtime::latch_readiness::{
        LatchFailure, LatchFailureReason, LatchFailureStage,
    };

    fn evidence() -> LatchFailureEvidence {
        LatchFailureEvidence::from(&LatchFailure::runtime(
            LatchFailureStage::FpgaStatus,
            LatchFailureReason::FpgaTransportFailed,
            "FPGA SPI timeout waiting for ACK high on word 0x0058",
        ))
    }

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mister-magik-{name}-{}-{}",
            std::process::id(),
            REPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn write_fixture(dir: &Path, timestamp: u128, sequence: usize, bytes: usize) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join(format!("{REPORT_PREFIX}{timestamp}-1-{sequence}.json")),
            vec![b'x'; bytes],
        )
        .unwrap();
    }

    #[test]
    fn report_is_atomic_and_bounded() {
        let dir = temp_dir("latch-report");
        let id = format!("{REPORT_PREFIX}1000-1-1");
        write_report(&dir, &id, evidence(), 1_000).unwrap();
        let latest = fs::read(dir.join(LATEST_FILE)).unwrap();
        assert!(latest.len() <= MAX_REPORT_BYTES);
        assert_eq!(
            serde_json::from_slice::<Value>(&latest).unwrap()["schema"],
            REPORT_SCHEMA
        );
        assert!(
            fs::read_dir(&dir)
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn oversized_failure_detail_is_truncated() {
        let dir = temp_dir("latch-detail");
        let id = format!("{REPORT_PREFIX}1000-1-1");
        let mut oversized = evidence();
        oversized.detail = "x".repeat(MAX_REPORT_BYTES * 2);
        oversized.latest_detail = oversized.detail.clone();
        write_report(&dir, &id, oversized, 1_000).unwrap();
        assert!(fs::metadata(dir.join(LATEST_FILE)).unwrap().len() <= MAX_REPORT_BYTES as u64);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn reports_older_than_48_hours_are_removed() {
        let dir = temp_dir("latch-age");
        let now = RETENTION_MS + 10_000;
        write_fixture(&dir, 1, 1, 16);
        write_fixture(&dir, now, 2, 16);
        prune_reports(&dir, &format!("{REPORT_PREFIX}{now}-1-2"), now).unwrap();
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_or_future_timestamps_remain_count_bounded() {
        let dir = temp_dir("latch-count");
        for sequence in 0..MAX_RETAINED_REPORTS + 4 {
            write_fixture(&dir, 0, sequence, 16);
        }
        let active = format!("{REPORT_PREFIX}0-1-{}.json", MAX_RETAINED_REPORTS + 3);
        prune_reports(&dir, active.trim_end_matches(".json"), RETENTION_MS + 1).unwrap();
        assert_eq!(fs::read_dir(&dir).unwrap().count(), MAX_RETAINED_REPORTS);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn reports_remain_total_size_bounded() {
        let dir = temp_dir("latch-size");
        for sequence in 0..10 {
            write_fixture(&dir, sequence as u128 + 1, sequence, 1024 * 1024);
        }
        let active = format!("{REPORT_PREFIX}10-1-9");
        prune_reports(&dir, &active, 10).unwrap();
        let total = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.metadata().ok())
            .map(|metadata| metadata.len())
            .sum::<u64>();
        assert!(total <= MAX_TOTAL_REPORT_BYTES);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn report_timestamp_parses_episode_name() {
        assert_eq!(report_timestamp("report-latch-1234-5-6.json"), Some(1234));
        assert_eq!(report_timestamp("latest.json"), None);
    }

    #[test]
    fn report_relative_path_uses_each_fixed_device_layout() {
        use mister_magik_catalog::device_layout::DeviceLayout;

        assert_eq!(
            DeviceLayout::Public.app_path(latest_relative_path()),
            PathBuf::from("/media/fat/mister-magik/diagnostics/latch/latest.json")
        );
        assert_eq!(
            DeviceLayout::Dev.app_path(latest_relative_path()),
            PathBuf::from("/media/fat/mister-magik-dev/diagnostics/latch/latest.json")
        );
    }
}
