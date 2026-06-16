//! Agent-readable runtime status and recent events.

use serde_json::{json, Value};
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

const DIR: &str = "/tmp/mister-magik";
const STATUS_PATH: &str = "/tmp/mister-magik/status.json";
const EVENTS_PATH: &str = "/tmp/mister-magik/events.jsonl";

pub struct LauncherStatus<'a> {
    pub scene: &'a str,
    pub screen: &'a str,
    pub frames: u64,
    pub fps_estimate: f64,
    pub last_frame_ms_ago: u64,
    pub catalog_ready: bool,
    pub catalog_games: usize,
    pub catalog_systems: usize,
    pub catalog_refresh_done: bool,
    pub launch_state: &'a str,
    pub loading_title: &'a str,
    pub input_pad_count: usize,
    pub active_pad_index: usize,
}

pub fn event(name: &str, detail: impl std::fmt::Display) {
    let _ = create_dir_all(DIR);
    let row = event_value(name, &detail.to_string(), unix_ms(), unsafe {
        libc::getpid()
    });
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(EVENTS_PATH)
    {
        let _ = writeln!(file, "{row}");
    }
}

pub fn write_launcher_status(status: LauncherStatus<'_>) {
    let _ = create_dir_all(DIR);
    let value = launcher_status_value(status, unix_ms(), unsafe { libc::getpid() });
    let tmp = format!("{STATUS_PATH}.tmp");
    if std::fs::write(&tmp, format!("{value}\n")).is_ok() {
        let _ = std::fs::rename(tmp, STATUS_PATH);
    }
}

fn event_value(name: &str, detail: &str, ts_unix_ms: u128, pid: libc::pid_t) -> Value {
    json!({
        "ts_unix_ms": ts_unix_ms,
        "source": "slint",
        "pid": pid,
        "event": name,
        "detail": detail,
    })
}

fn launcher_status_value(status: LauncherStatus<'_>, ts_unix_ms: u128, pid: libc::pid_t) -> Value {
    json!({
        "schema": "mister-magik-slint-status-v1",
        "ts_unix_ms": ts_unix_ms,
        "pid": pid,
        "mode": "ui",
        "scene": status.scene,
        "screen": status.screen,
        "frames": status.frames,
        "fps_estimate": (status.fps_estimate * 10.0).round() / 10.0,
        "last_frame_ms_ago": status.last_frame_ms_ago,
        "catalog_ready": status.catalog_ready,
        "catalog_games": status.catalog_games,
        "catalog_systems": status.catalog_systems,
        "catalog_refresh_done": status.catalog_refresh_done,
        "launch_state": status.launch_state,
        "loading_title": status.loading_title,
        "input_pad_count": status.input_pad_count,
        "active_pad_index": status.active_pad_index,
    })
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_name(prefix: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{prefix}-{nanos}")
    }

    struct FileRestore {
        path: &'static str,
        original: Option<Vec<u8>>,
    }

    impl FileRestore {
        fn capture(path: &'static str) -> Self {
            Self {
                path,
                original: fs::read(path).ok(),
            }
        }
    }

    impl Drop for FileRestore {
        fn drop(&mut self) {
            match &self.original {
                Some(bytes) => {
                    let _ = fs::write(self.path, bytes);
                }
                None => {
                    let _ = fs::remove_file(self.path);
                }
            }
        }
    }

    #[test]
    fn event_value_uses_agent_readable_schema() {
        let value = event_value("first_frame", "catalog_ready=true", 1234, 99);
        assert_eq!(value["ts_unix_ms"], 1234);
        assert_eq!(value["source"], "slint");
        assert_eq!(value["pid"], 99);
        assert_eq!(value["event"], "first_frame");
        assert_eq!(value["detail"], "catalog_ready=true");
    }

    #[test]
    fn launcher_status_value_contains_runtime_state_and_rounds_fps() {
        let value = launcher_status_value(
            LauncherStatus {
                scene: "launcher",
                screen: "home",
                frames: 42,
                fps_estimate: 59.94,
                last_frame_ms_ago: 7,
                catalog_ready: true,
                catalog_games: 9014,
                catalog_systems: 13,
                catalog_refresh_done: false,
                launch_state: "idle",
                loading_title: "",
                input_pad_count: 3,
                active_pad_index: 2,
            },
            5678,
            101,
        );

        assert_eq!(value["schema"], "mister-magik-slint-status-v1");
        assert_eq!(value["ts_unix_ms"], 5678);
        assert_eq!(value["pid"], 101);
        assert_eq!(value["mode"], "ui");
        assert_eq!(value["scene"], "launcher");
        assert_eq!(value["screen"], "home");
        assert_eq!(value["frames"], 42);
        assert_eq!(value["fps_estimate"], 59.9);
        assert_eq!(value["last_frame_ms_ago"], 7);
        assert_eq!(value["catalog_ready"], true);
        assert_eq!(value["catalog_games"], 9014);
        assert_eq!(value["catalog_systems"], 13);
        assert_eq!(value["catalog_refresh_done"], false);
        assert_eq!(value["launch_state"], "idle");
        assert_eq!(value["input_pad_count"], 3);
        assert_eq!(value["active_pad_index"], 2);
    }

    #[test]
    fn event_appends_jsonl_row_to_runtime_event_file() {
        let _restore = FileRestore::capture(EVENTS_PATH);
        let _ = fs::remove_file(EVENTS_PATH);
        let name = unique_name("coverage_event");
        event(&name, "detail=ok");

        let text = fs::read_to_string(EVENTS_PATH).expect("events jsonl should be written");
        let row: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(row["source"], "slint");
        assert_eq!(row["event"], name);
        assert_eq!(row["detail"], "detail=ok");
        assert!(row["ts_unix_ms"].as_u64().is_some());
        assert!(row["pid"].as_i64().is_some());
    }

    #[test]
    fn write_launcher_status_replaces_status_file_atomically() {
        let _restore = FileRestore::capture(STATUS_PATH);
        let _ = fs::remove_file(STATUS_PATH);
        let _ = fs::remove_file(format!("{STATUS_PATH}.tmp"));

        write_launcher_status(LauncherStatus {
            scene: "launcher",
            screen: "arcade",
            frames: 7,
            fps_estimate: 60.04,
            last_frame_ms_ago: 1,
            catalog_ready: false,
            catalog_games: 12,
            catalog_systems: 2,
            catalog_refresh_done: true,
            launch_state: "loading",
            loading_title: "1942",
            input_pad_count: 1,
            active_pad_index: 0,
        });

        let text = fs::read_to_string(STATUS_PATH).expect("status json should be written");
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema"], "mister-magik-slint-status-v1");
        assert_eq!(value["screen"], "arcade");
        assert_eq!(value["frames"], 7);
        assert_eq!(value["fps_estimate"], 60.0);
        assert_eq!(value["catalog_ready"], false);
        assert_eq!(value["catalog_refresh_done"], true);
        assert_eq!(value["loading_title"], "1942");
        assert!(!std::path::Path::new(&format!("{STATUS_PATH}.tmp")).exists());
    }
}
