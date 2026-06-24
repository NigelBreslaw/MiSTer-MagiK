//! Local crash reports for MiSTer MagiK.

use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_CRASH_DIR: &str = "/media/fat/mister-magik/crashes";
const STATUS_PATH: &str = "/tmp/mister-magik/status.json";
const EVENTS_PATH: &str = "/tmp/mister-magik/events.jsonl";
const SLINT_LOG_PATH: &str = "/tmp/mister-magik-slint.log";

pub fn install_panic_hook(args: Vec<String>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = write_panic_report(Path::new(DEFAULT_CRASH_DIR), &args, info);
        previous(info);
    }));
}

fn write_panic_report(
    dir: &Path,
    args: &[String],
    info: &std::panic::PanicHookInfo<'_>,
) -> io::Result<PathBuf> {
    let ts_unix_ms = unix_ms();
    let pid = process_id();
    let report_id = format!("report-slint-{ts_unix_ms}-{pid}");
    let mut report = panic_report_value(&report_id, ts_unix_ms, pid, args, info);
    if backtrace_enabled() {
        report["backtrace"] =
            Value::String(format!("{}", std::backtrace::Backtrace::force_capture()));
    }
    write_report_value(dir, &report_id, &report)
}

fn panic_report_value(
    report_id: &str,
    ts_unix_ms: u128,
    pid: u32,
    args: &[String],
    info: &std::panic::PanicHookInfo<'_>,
) -> Value {
    let panic_payload = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string());
    let location = info.location().map(|location| {
        json!({
            "file": location.file(),
            "line": location.line(),
            "column": location.column(),
        })
    });

    json!({
        "schema": "mister-magik-crash-report-v1",
        "source": "slint",
        "kind": "panic",
        "report_id": report_id,
        "ts_unix_ms": ts_unix_ms,
        "pid": pid,
        "process": {
            "arch": std::env::consts::ARCH,
            "package_version": env!("CARGO_PKG_VERSION"),
            "args": args,
            "proc_status": read_text_value("/proc/self/status"),
        },
        "panic": {
            "payload": panic_payload,
            "location": location.unwrap_or(Value::Null),
        },
        "files": {
            "slint_status": read_json_value(STATUS_PATH),
            "events_tail": tail_text_value(EVENTS_PATH, 80),
            "slint_log_tail": tail_text_value(SLINT_LOG_PATH, 120),
        },
    })
}

fn write_report_value(dir: &Path, report_id: &str, report: &Value) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{report_id}.json"));
    let tmp_path = dir.join(format!("{report_id}.json.tmp"));
    let latest_path = dir.join("latest.json");
    let latest_tmp_path = dir.join("latest.json.tmp");
    let mut bytes = serde_json::to_vec_pretty(report)?;
    bytes.push(b'\n');

    write_file_sync(&tmp_path, &bytes)?;
    fs::rename(&tmp_path, &path)?;
    write_file_sync(&latest_tmp_path, &bytes)?;
    fs::rename(&latest_tmp_path, &latest_path)?;
    Ok(path)
}

fn write_file_sync(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn read_json_value(path: &str) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

fn read_text_value(path: &str) -> Value {
    fs::read_to_string(path)
        .map(Value::String)
        .unwrap_or(Value::Null)
}

fn tail_text_value(path: &str, n: usize) -> Value {
    let Ok(text) = fs::read_to_string(path) else {
        return Value::Null;
    };
    let lines: Vec<_> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    Value::String(lines[start..].join("\n"))
}

fn backtrace_enabled() -> bool {
    std::env::var("MISTER_CRASH_BACKTRACE")
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn process_id() -> u32 {
    std::process::id()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", unix_ms()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn write_report_creates_report_and_latest_copy() {
        let dir = unique_temp_dir("mister-magik-crash-report");
        let report = json!({
            "schema": "mister-magik-crash-report-v1",
            "source": "slint",
            "kind": "panic",
            "report_id": "report-slint-123-45",
        });

        let path = write_report_value(&dir, "report-slint-123-45", &report).expect("write report");
        let latest = fs::read_to_string(dir.join("latest.json")).expect("latest");
        let written = fs::read_to_string(path).expect("report");

        assert_eq!(latest, written);
        assert_eq!(
            serde_json::from_str::<Value>(&latest).expect("json")["schema"],
            "mister-magik-crash-report-v1"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn tail_text_value_keeps_newest_lines() {
        let dir = unique_temp_dir("mister-magik-crash-tail");
        let path = dir.join("events.jsonl");
        fs::write(&path, "one\ntwo\nthree\n").expect("write events");

        assert_eq!(
            tail_text_value(path.to_str().expect("utf8"), 2),
            Value::String("two\nthree".to_string())
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn crash_report_read_helpers_fall_back_to_null_for_missing_or_bad_files() {
        let dir = unique_temp_dir("mister-magik-crash-read-helpers");
        let json_path = dir.join("status.json");
        let text_path = dir.join("log.txt");
        fs::write(&json_path, "{not json").expect("write malformed json");
        fs::write(&text_path, "line one\nline two\n").expect("write text");

        assert_eq!(
            read_json_value(json_path.to_str().expect("utf8")),
            Value::Null
        );
        assert_eq!(
            read_json_value(dir.join("missing.json").to_str().expect("utf8")),
            Value::Null
        );
        assert_eq!(
            read_text_value(text_path.to_str().expect("utf8")),
            Value::String("line one\nline two\n".to_string())
        );
        assert_eq!(
            tail_text_value(dir.join("missing.log").to_str().expect("utf8"), 10),
            Value::Null
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn crash_report_write_reports_directory_setup_failure() {
        let root = unique_temp_dir("mister-magik-crash-bad-dir");
        let dir = root.join("not-a-dir");
        fs::write(&dir, b"file blocks directory creation").expect("write file");
        let report = json!({
            "schema": "mister-magik-crash-report-v1",
        });

        let err =
            write_report_value(&dir, "bad-report", &report).expect_err("directory setup failure");

        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        let _ = fs::remove_dir_all(root);
    }
}
