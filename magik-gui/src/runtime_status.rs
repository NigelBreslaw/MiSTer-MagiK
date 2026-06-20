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
    pub rolling_fps: f64,
    pub rolling_prepare_us: u64,
    pub rolling_render_us: u64,
    pub rolling_custom_draw_us: u64,
    pub rolling_vsync_us: u64,
    pub rolling_present_us: u64,
    pub rolling_rows: u64,
    pub last_frame_ms_ago: u64,
    pub catalog_ready: bool,
    pub catalog_games: usize,
    pub catalog_systems: usize,
    pub catalog_refresh_done: bool,
    pub catalog_scan_visible: bool,
    pub catalog_scan_title: &'a str,
    pub catalog_scan_detail: &'a str,
    pub catalog_scan_percent: i32,
    pub arcade_selected: usize,
    pub arcade_visual_index: f32,
    pub preview_cache_state: &'a str,
    pub preview_transition_effect: &'a str,
    pub preview_transition_progress: f32,
    pub bench_scenario: &'a str,
    pub start_screen: &'a str,
    pub lock_screen: &'a str,
    pub route_reassert_count: u64,
    pub last_route_reassert_frame: u64,
    pub last_route_reassert_ok: bool,
    pub last_route_reassert_error: &'a str,
    pub launch_state: &'a str,
    pub loading_title: &'a str,
    pub input_pad_count: usize,
    pub active_pad_index: usize,
    pub active_pad_name: &'a str,
    pub active_pad_path: &'a str,
    pub last_raw_event: &'a str,
    pub last_input_ms_ago: u64,
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
    let mut map = serde_json::Map::new();
    macro_rules! insert {
        ($key:literal, $value:expr) => {
            map.insert($key.to_string(), json!($value));
        };
    }

    insert!("schema", "mister-magik-slint-status-v1");
    insert!("ts_unix_ms", ts_unix_ms);
    insert!("pid", pid);
    insert!("mode", "ui");
    insert!("scene", status.scene);
    insert!("screen", status.screen);
    insert!("frames", status.frames);
    insert!("fps_estimate", (status.fps_estimate * 10.0).round() / 10.0);
    insert!("rolling_fps", (status.rolling_fps * 10.0).round() / 10.0);
    insert!("rolling_prepare_us", status.rolling_prepare_us);
    insert!("rolling_render_us", status.rolling_render_us);
    insert!("rolling_custom_draw_us", status.rolling_custom_draw_us);
    insert!("rolling_vsync_us", status.rolling_vsync_us);
    insert!("rolling_present_us", status.rolling_present_us);
    insert!("rolling_rows", status.rolling_rows);
    insert!("last_frame_ms_ago", status.last_frame_ms_ago);
    insert!("catalog_ready", status.catalog_ready);
    insert!("catalog_games", status.catalog_games);
    insert!("catalog_systems", status.catalog_systems);
    insert!("catalog_refresh_done", status.catalog_refresh_done);
    insert!("catalog_scan_visible", status.catalog_scan_visible);
    insert!("catalog_scan_title", status.catalog_scan_title);
    insert!("catalog_scan_detail", status.catalog_scan_detail);
    insert!("catalog_scan_percent", status.catalog_scan_percent);
    insert!("arcade_selected", status.arcade_selected);
    insert!(
        "arcade_visual_index",
        (status.arcade_visual_index * 1000.0).round() / 1000.0
    );
    insert!("preview_cache_state", status.preview_cache_state);
    insert!(
        "preview_transition_effect",
        status.preview_transition_effect
    );
    insert!(
        "preview_transition_progress",
        (status.preview_transition_progress * 1000.0).round() / 1000.0
    );
    insert!("bench_scenario", status.bench_scenario);
    insert!("start_screen", status.start_screen);
    insert!("lock_screen", status.lock_screen);
    insert!("route_reassert_count", status.route_reassert_count);
    insert!(
        "last_route_reassert_frame",
        status.last_route_reassert_frame
    );
    insert!("last_route_reassert_ok", status.last_route_reassert_ok);
    insert!(
        "last_route_reassert_error",
        status.last_route_reassert_error
    );
    insert!("launch_state", status.launch_state);
    insert!("loading_title", status.loading_title);
    insert!("input_pad_count", status.input_pad_count);
    insert!("active_pad_index", status.active_pad_index);
    insert!("active_pad_name", status.active_pad_name);
    insert!("active_pad_path", status.active_pad_path);
    insert!("last_raw_event", status.last_raw_event);
    insert!("last_input_ms_ago", status.last_input_ms_ago);
    insert!("rss_kb", current_rss_kb());
    insert!("rss_hwm_kb", current_rss_hwm_kb());

    Value::Object(map)
}

fn current_rss_kb() -> u64 {
    proc_status_kb("VmRSS")
}

fn current_rss_hwm_kb() -> u64 {
    proc_status_kb("VmHWM")
}

fn proc_status_kb(key: &str) -> u64 {
    let Ok(text) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    text.lines()
        .find_map(|line| {
            let (line_key, rest) = line.split_once(':')?;
            (line_key == key).then(|| {
                rest.split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0)
            })
        })
        .unwrap_or(0)
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
                rolling_fps: 60.0,
                rolling_prepare_us: 1,
                rolling_render_us: 2,
                rolling_custom_draw_us: 3,
                rolling_vsync_us: 4,
                rolling_present_us: 5,
                rolling_rows: 6,
                last_frame_ms_ago: 7,
                catalog_ready: true,
                catalog_games: 9014,
                catalog_systems: 13,
                catalog_refresh_done: false,
                catalog_scan_visible: false,
                catalog_scan_title: "",
                catalog_scan_detail: "",
                catalog_scan_percent: -1,
                arcade_selected: 3,
                arcade_visual_index: 3.25,
                preview_cache_state: "exact",
                preview_transition_effect: "fade",
                preview_transition_progress: 0.5,
                bench_scenario: "held-scroll",
                start_screen: "arcade",
                lock_screen: "arcade",
                route_reassert_count: 2,
                last_route_reassert_frame: 120,
                last_route_reassert_ok: true,
                last_route_reassert_error: "",
                launch_state: "idle",
                loading_title: "",
                input_pad_count: 3,
                active_pad_index: 2,
                active_pad_name: "Pad",
                active_pad_path: "/dev/input/js0",
                last_raw_event: "type=1 num=0 val=1",
                last_input_ms_ago: 100,
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
        assert_eq!(value["rolling_fps"], 60.0);
        assert_eq!(value["rolling_present_us"], 5);
        assert_eq!(value["last_frame_ms_ago"], 7);
        assert_eq!(value["catalog_ready"], true);
        assert_eq!(value["catalog_games"], 9014);
        assert_eq!(value["catalog_systems"], 13);
        assert_eq!(value["catalog_refresh_done"], false);
        assert_eq!(value["catalog_scan_visible"], false);
        assert_eq!(value["arcade_selected"], 3);
        assert_eq!(value["arcade_visual_index"], 3.25);
        assert_eq!(value["preview_cache_state"], "exact");
        assert_eq!(value["preview_transition_effect"], "fade");
        assert_eq!(value["preview_transition_progress"], 0.5);
        assert_eq!(value["bench_scenario"], "held-scroll");
        assert_eq!(value["route_reassert_count"], 2);
        assert_eq!(value["last_route_reassert_ok"], true);
        assert_eq!(value["launch_state"], "idle");
        assert_eq!(value["input_pad_count"], 3);
        assert_eq!(value["active_pad_index"], 2);
        assert_eq!(value["active_pad_name"], "Pad");
        assert_eq!(value["last_raw_event"], "type=1 num=0 val=1");
        assert_eq!(value["last_input_ms_ago"], 100);
        assert!(value["rss_kb"].as_u64().is_some());
        assert!(value["rss_hwm_kb"].as_u64().is_some());
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
            rolling_fps: 59.9,
            rolling_prepare_us: 11,
            rolling_render_us: 22,
            rolling_custom_draw_us: 33,
            rolling_vsync_us: 44,
            rolling_present_us: 55,
            rolling_rows: 66,
            last_frame_ms_ago: 1,
            catalog_ready: false,
            catalog_games: 12,
            catalog_systems: 2,
            catalog_refresh_done: true,
            catalog_scan_visible: true,
            catalog_scan_title: "Indexing library",
            catalog_scan_detail: "Games found: 12",
            catalog_scan_percent: -1,
            arcade_selected: 0,
            arcade_visual_index: 0.0,
            preview_cache_state: "placeholder",
            preview_transition_effect: "fade",
            preview_transition_progress: 1.0,
            bench_scenario: "none",
            start_screen: "home",
            lock_screen: "none",
            route_reassert_count: 0,
            last_route_reassert_frame: 0,
            last_route_reassert_ok: false,
            last_route_reassert_error: "",
            launch_state: "loading",
            loading_title: "1942",
            input_pad_count: 1,
            active_pad_index: 0,
            active_pad_name: "Pad",
            active_pad_path: "/dev/input/js0",
            last_raw_event: "",
            last_input_ms_ago: 0,
        });

        let text = fs::read_to_string(STATUS_PATH).expect("status json should be written");
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema"], "mister-magik-slint-status-v1");
        assert_eq!(value["screen"], "arcade");
        assert_eq!(value["frames"], 7);
        assert_eq!(value["fps_estimate"], 60.0);
        assert_eq!(value["catalog_ready"], false);
        assert_eq!(value["catalog_refresh_done"], true);
        assert_eq!(value["catalog_scan_visible"], true);
        assert_eq!(value["catalog_scan_title"], "Indexing library");
        assert_eq!(value["loading_title"], "1942");
        assert!(!std::path::Path::new(&format!("{STATUS_PATH}.tmp")).exists());
    }
}
