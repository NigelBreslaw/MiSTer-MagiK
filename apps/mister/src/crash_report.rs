// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Local crash reports for MiSTer MagiK.

use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const STATUS_PATH: &str = "/tmp/mister-magik/status.json";
const EVENTS_PATH: &str = "/tmp/mister-magik/events.jsonl";
const SLINT_LOG_PATH: &str = "/tmp/mister-magik-slint.log";

pub fn install_panic_hook(args: Vec<String>) {
    let crash_dir = mister_magik_catalog::device_layout::current_app_path("crashes");
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = write_panic_report(&crash_dir, &args, info);
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
        "build": crate::build_identity::BuildIdentity::current(),
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
    let mut fault_control =
        mister_magik_mister_runtime::direct_reset_fault::process_fault_control();
    write_report_value_with_fault_control(
        &SystemReportIo,
        dir,
        report_id,
        report,
        &mut fault_control,
    )
}

#[cfg(test)]
fn write_report_value_with(
    report_io: &impl ReportIo,
    dir: &Path,
    report_id: &str,
    report: &Value,
) -> io::Result<PathBuf> {
    let mut fault_control = mister_magik_catalog::fs_fault::NoopDirectResetFaultControl;
    write_report_value_with_fault_control(report_io, dir, report_id, report, &mut fault_control)
}

fn write_report_value_with_fault_control(
    report_io: &impl ReportIo,
    dir: &Path,
    report_id: &str,
    report: &Value,
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> io::Result<PathBuf> {
    report_io.create_dir_all(dir)?;
    let path = dir.join(format!("{report_id}.json"));
    let tmp_path = dir.join(format!("{report_id}.json.tmp"));
    let latest_path = dir.join("latest.json");
    let latest_tmp_path = dir.join("latest.json.tmp");
    let mut bytes = serde_json::to_vec_pretty(report)?;
    bytes.push(b'\n');

    report_io.write_file_sync(&tmp_path, &bytes)?;
    mister_magik_catalog::fs_fault::maybe_fault_with_control(
        "crash_report.report.after_temp_sync",
        &path,
        fault_control,
    );
    report_io.rename(&tmp_path, &path)?;
    mister_magik_catalog::fs_fault::maybe_fault_with_control(
        "crash_report.report.after_rename",
        &path,
        fault_control,
    );
    report_io.write_file_sync(&latest_tmp_path, &bytes)?;
    mister_magik_catalog::fs_fault::maybe_fault_with_control(
        "crash_report.latest.after_temp_sync",
        &latest_path,
        fault_control,
    );
    report_io.rename(&latest_tmp_path, &latest_path)?;
    mister_magik_catalog::fs_fault::maybe_fault_with_control(
        "crash_report.latest.after_rename",
        &latest_path,
        fault_control,
    );
    Ok(path)
}

trait ReportIo {
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    fn write_file_sync(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
}

struct SystemReportIo;

impl ReportIo for SystemReportIo {
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }

    fn write_file_sync(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        write_file_sync(path, bytes)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }
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
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[derive(Default)]
    struct RecordingFaultControl {
        points: Vec<String>,
    }

    impl mister_magik_catalog::fs_fault::DirectResetFaultControl for RecordingFaultControl {
        fn request_direct_reset(
            &mut self,
            request: &mister_magik_catalog::fs_fault::DirectResetFaultRequest,
        ) -> mister_magik_catalog::fs_fault::DirectResetFaultOutcome {
            self.points.push(request.point().to_string());
            mister_magik_catalog::fs_fault::DirectResetFaultOutcome::Noop
        }
    }

    struct ScriptedReportIo {
        events: RefCell<Vec<String>>,
        fail_at: Option<usize>,
    }

    impl ScriptedReportIo {
        fn record(&self, event: String) -> io::Result<()> {
            let mut events = self.events.borrow_mut();
            events.push(event);
            if self.fail_at == Some(events.len()) {
                return Err(io::Error::other("scripted report I/O failure"));
            }
            Ok(())
        }
    }

    impl ReportIo for ScriptedReportIo {
        fn create_dir_all(&self, path: &Path) -> io::Result<()> {
            self.record(format!("mkdir {}", path.display()))
        }

        fn write_file_sync(&self, path: &Path, _bytes: &[u8]) -> io::Result<()> {
            self.record(format!("write {}", path.display()))
        }

        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            self.record(format!("rename {} {}", from.display(), to.display()))
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nonce = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("{prefix}-{}-{nanos}-{nonce}", std::process::id()));
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

        let mut fault_control = RecordingFaultControl::default();
        let path = write_report_value_with_fault_control(
            &SystemReportIo,
            &dir,
            "report-slint-123-45",
            &report,
            &mut fault_control,
        )
        .expect("write report");
        let latest = fs::read_to_string(dir.join("latest.json")).expect("latest");
        let written = fs::read_to_string(path).expect("report");

        assert_eq!(latest, written);
        assert_eq!(
            serde_json::from_str::<Value>(&latest).expect("json")["schema"],
            "mister-magik-crash-report-v1"
        );
        assert_eq!(
            fault_control.points,
            vec![
                "crash_report.report.after_temp_sync",
                "crash_report.report.after_rename",
                "crash_report.latest.after_temp_sync",
                "crash_report.latest.after_rename",
            ]
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

    #[test]
    fn crash_report_publishes_versioned_report_before_latest_pointer() {
        let report_io = ScriptedReportIo {
            events: RefCell::new(Vec::new()),
            fail_at: None,
        };
        let dir = Path::new("/reports");

        let path = write_report_value_with(
            &report_io,
            dir,
            "report-slint-1-2",
            &json!({"schema": "mister-magik-crash-report-v1"}),
        )
        .expect("scripted report");

        assert_eq!(path, PathBuf::from("/reports/report-slint-1-2.json"));
        assert_eq!(
            *report_io.events.borrow(),
            [
                "mkdir /reports",
                "write /reports/report-slint-1-2.json.tmp",
                "rename /reports/report-slint-1-2.json.tmp /reports/report-slint-1-2.json",
                "write /reports/latest.json.tmp",
                "rename /reports/latest.json.tmp /reports/latest.json",
            ]
        );
    }

    #[test]
    fn crash_report_stops_at_each_failed_atomic_io_step() {
        for fail_at in 1..=5 {
            let report_io = ScriptedReportIo {
                events: RefCell::new(Vec::new()),
                fail_at: Some(fail_at),
            };

            let error = write_report_value_with(
                &report_io,
                Path::new("/reports"),
                "report-slint-1-2",
                &json!({"schema": "mister-magik-crash-report-v1"}),
            )
            .expect_err("injected failure must stop publication");

            assert_eq!(error.kind(), io::ErrorKind::Other);
            assert_eq!(report_io.events.borrow().len(), fail_at);
        }
    }
}
