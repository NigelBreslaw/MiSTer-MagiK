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
    pub idle: bool,
    pub idle_loops: u64,
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
    pub catalog_scan_message: &'a str,
    pub catalog_scan_title: &'a str,
    pub catalog_scan_detail: &'a str,
    pub catalog_scan_percent: i32,
    pub catalog_background_scan_visible: bool,
    pub confirm_visible: bool,
    pub confirm_title: &'a str,
    pub confirm_selected: i32,
    pub confirm_left_label: &'a str,
    pub confirm_right_label: &'a str,
    pub arcade_selected: usize,
    pub arcade_visual_index: f32,
    pub preview_cache_state: &'a str,
    pub preview_transition_effect: &'a str,
    pub preview_transition_progress: f32,
    pub composition_state: &'a str,
    pub composition_recovery_count: u64,
    pub last_composition_invariant_kind: &'a str,
    pub last_composition_invariant_detail: &'a str,
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
    pub startup_mode: &'a str,
    pub startup_reveal_state: &'a str,
    pub revealed: bool,
    pub input_enabled: bool,
    pub reveal_ms: u64,
    pub input_enabled_ms: u64,
    pub frame_budget: FrameBudgetStatus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameBudgetStatus {
    pub budget_us: u64,
    pub frames_total: u64,
    pub over_budget_total: u64,
    pub over_20ms_total: u64,
    pub over_33ms_total: u64,
    pub max_wall_us: u64,
    pub latest_over_budget_frame: u64,
    pub latest_over_budget_wall_us: u64,
    pub max_vsync_miss_streak: u64,
    pub vsync_total: u64,
    pub fallback_total: u64,
    pub timeout_total: u64,
    pub error_total: u64,
    pub window_frames: u64,
    pub window_over_budget: u64,
    pub window_over_20ms: u64,
    pub window_over_33ms: u64,
    pub window_max_wall_us: u64,
    pub window_max_vsync_miss_streak: u64,
    pub window_prepare_us: u64,
    pub window_render_us: u64,
    pub window_custom_draw_us: u64,
    pub window_vsync_us: u64,
    pub window_present_us: u64,
}

pub fn event(name: &str, detail: impl std::fmt::Display) {
    let _ = create_dir_all(DIR);
    let row = event_value(name, &detail.to_string(), unix_ms(), std::process::id());
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
    let value = launcher_status_value(status, unix_ms(), std::process::id());
    let tmp = format!("{STATUS_PATH}.tmp");
    if std::fs::write(&tmp, format!("{value}\n")).is_ok() {
        let _ = std::fs::rename(tmp, STATUS_PATH);
    }
}

fn event_value(name: &str, detail: &str, ts_unix_ms: u128, pid: u32) -> Value {
    json!({
        "ts_unix_ms": ts_unix_ms,
        "source": "slint",
        "pid": pid,
        "event": name,
        "detail": detail,
    })
}

fn launcher_status_value(status: LauncherStatus<'_>, ts_unix_ms: u128, pid: u32) -> Value {
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
    insert!("idle", status.idle);
    insert!("idle_loops", status.idle_loops);
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
    insert!("catalog_scan_message", status.catalog_scan_message);
    insert!("catalog_scan_title", status.catalog_scan_title);
    insert!("catalog_scan_detail", status.catalog_scan_detail);
    insert!("catalog_scan_percent", status.catalog_scan_percent);
    insert!(
        "catalog_background_scan_visible",
        status.catalog_background_scan_visible
    );
    insert!("confirm_visible", status.confirm_visible);
    insert!("confirm_title", status.confirm_title);
    insert!("confirm_selected", status.confirm_selected);
    insert!("confirm_left_label", status.confirm_left_label);
    insert!("confirm_right_label", status.confirm_right_label);
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
    insert!("composition_state", status.composition_state);
    insert!(
        "composition_recovery_count",
        status.composition_recovery_count
    );
    insert!(
        "last_composition_invariant_kind",
        status.last_composition_invariant_kind
    );
    insert!(
        "last_composition_invariant_detail",
        status.last_composition_invariant_detail
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
    insert!("startup_mode", status.startup_mode);
    insert!("startup_reveal_state", status.startup_reveal_state);
    insert!("revealed", status.revealed);
    insert!("input_enabled", status.input_enabled);
    insert!("reveal_ms", status.reveal_ms);
    insert!("input_enabled_ms", status.input_enabled_ms);
    insert!(
        "frame_budget",
        json!({
            "budget_us": status.frame_budget.budget_us,
            "frames_total": status.frame_budget.frames_total,
            "over_budget_total": status.frame_budget.over_budget_total,
            "over_20ms_total": status.frame_budget.over_20ms_total,
            "over_33ms_total": status.frame_budget.over_33ms_total,
            "max_wall_us": status.frame_budget.max_wall_us,
            "latest_over_budget_frame": status.frame_budget.latest_over_budget_frame,
            "latest_over_budget_wall_us": status.frame_budget.latest_over_budget_wall_us,
            "max_vsync_miss_streak": status.frame_budget.max_vsync_miss_streak,
            "vsync_total": status.frame_budget.vsync_total,
            "fallback_total": status.frame_budget.fallback_total,
            "timeout_total": status.frame_budget.timeout_total,
            "error_total": status.frame_budget.error_total,
            "window_frames": status.frame_budget.window_frames,
            "window_over_budget": status.frame_budget.window_over_budget,
            "window_over_20ms": status.frame_budget.window_over_20ms,
            "window_over_33ms": status.frame_budget.window_over_33ms,
            "window_max_wall_us": status.frame_budget.window_max_wall_us,
            "window_max_vsync_miss_streak": status.frame_budget.window_max_vsync_miss_streak,
            "window_prepare_us": status.frame_budget.window_prepare_us,
            "window_render_us": status.frame_budget.window_render_us,
            "window_custom_draw_us": status.frame_budget.window_custom_draw_us,
            "window_vsync_us": status.frame_budget.window_vsync_us,
            "window_present_us": status.frame_budget.window_present_us,
        })
    );
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
                idle: true,
                idle_loops: 12,
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
                catalog_scan_message: "Scanning for games",
                catalog_scan_title: "",
                catalog_scan_detail: "",
                catalog_scan_percent: -1,
                catalog_background_scan_visible: false,
                confirm_visible: true,
                confirm_title: "Library changed",
                confirm_selected: 0,
                confirm_left_label: "Continue",
                confirm_right_label: "Rebuild",
                arcade_selected: 3,
                arcade_visual_index: 3.25,
                preview_cache_state: "exact",
                preview_transition_effect: "fade",
                preview_transition_progress: 0.5,
                composition_state: "mixed-arcade",
                composition_recovery_count: 0,
                last_composition_invariant_kind: "",
                last_composition_invariant_detail: "",
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
                startup_mode: "warm_catalog",
                startup_reveal_state: "input_enabled",
                revealed: true,
                input_enabled: true,
                reveal_ms: 37,
                input_enabled_ms: 37,
                frame_budget: FrameBudgetStatus {
                    budget_us: 16_667,
                    frames_total: 42,
                    over_budget_total: 2,
                    over_20ms_total: 1,
                    over_33ms_total: 0,
                    max_wall_us: 21_000,
                    latest_over_budget_frame: 40,
                    latest_over_budget_wall_us: 18_000,
                    max_vsync_miss_streak: 1,
                    vsync_total: 40,
                    fallback_total: 1,
                    timeout_total: 1,
                    error_total: 0,
                    window_frames: 30,
                    window_over_budget: 1,
                    window_over_20ms: 0,
                    window_over_33ms: 0,
                    window_max_wall_us: 18_000,
                    window_max_vsync_miss_streak: 1,
                    window_prepare_us: 100,
                    window_render_us: 2_000,
                    window_custom_draw_us: 3_000,
                    window_vsync_us: 8_000,
                    window_present_us: 900,
                },
            },
            5678,
            101,
        );

        assert_eq!(value["schema"], "mister-magik-slint-status-v1");
        assert_eq!(value["frame_budget"]["budget_us"], 16_667);
        assert_eq!(value["frame_budget"]["over_budget_total"], 2);
        assert_eq!(value["frame_budget"]["window_present_us"], 900);
        assert_eq!(value["ts_unix_ms"], 5678);
        assert_eq!(value["pid"], 101);
        assert_eq!(value["mode"], "ui");
        assert_eq!(value["scene"], "launcher");
        assert_eq!(value["screen"], "home");
        assert_eq!(value["frames"], 42);
        assert_eq!(value["idle"], true);
        assert_eq!(value["idle_loops"], 12);
        assert_eq!(value["fps_estimate"], 59.9);
        assert_eq!(value["rolling_fps"], 60.0);
        assert_eq!(value["rolling_present_us"], 5);
        assert_eq!(value["last_frame_ms_ago"], 7);
        assert_eq!(value["catalog_ready"], true);
        assert_eq!(value["catalog_games"], 9014);
        assert_eq!(value["catalog_systems"], 13);
        assert_eq!(value["catalog_refresh_done"], false);
        assert_eq!(value["catalog_scan_visible"], false);
        assert_eq!(value["catalog_scan_message"], "Scanning for games");
        assert_eq!(value["catalog_background_scan_visible"], false);
        assert_eq!(value["confirm_visible"], true);
        assert_eq!(value["confirm_title"], "Library changed");
        assert_eq!(value["confirm_selected"], 0);
        assert_eq!(value["confirm_left_label"], "Continue");
        assert_eq!(value["confirm_right_label"], "Rebuild");
        assert_eq!(value["arcade_selected"], 3);
        assert_eq!(value["arcade_visual_index"], 3.25);
        assert_eq!(value["preview_cache_state"], "exact");
        assert_eq!(value["preview_transition_effect"], "fade");
        assert_eq!(value["preview_transition_progress"], 0.5);
        assert_eq!(value["composition_state"], "mixed-arcade");
        assert_eq!(value["composition_recovery_count"], 0);
        assert_eq!(value["last_composition_invariant_kind"], "");
        assert_eq!(value["last_composition_invariant_detail"], "");
        assert_eq!(value["bench_scenario"], "held-scroll");
        assert_eq!(value["route_reassert_count"], 2);
        assert_eq!(value["last_route_reassert_ok"], true);
        assert_eq!(value["launch_state"], "idle");
        assert_eq!(value["input_pad_count"], 3);
        assert_eq!(value["active_pad_index"], 2);
        assert_eq!(value["startup_mode"], "warm_catalog");
        assert_eq!(value["startup_reveal_state"], "input_enabled");
        assert_eq!(value["revealed"], true);
        assert_eq!(value["input_enabled"], true);
        assert_eq!(value["reveal_ms"], 37);
        assert_eq!(value["input_enabled_ms"], 37);
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
            idle: false,
            idle_loops: 0,
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
            catalog_scan_message: "Updating Library",
            catalog_scan_title: "Indexing library",
            catalog_scan_detail: "Games found: 12",
            catalog_scan_percent: -1,
            catalog_background_scan_visible: true,
            confirm_visible: false,
            confirm_title: "",
            confirm_selected: 0,
            confirm_left_label: "",
            confirm_right_label: "",
            arcade_selected: 0,
            arcade_visual_index: 0.0,
            preview_cache_state: "placeholder",
            preview_transition_effect: "fade",
            preview_transition_progress: 1.0,
            composition_state: "full-slint",
            composition_recovery_count: 0,
            last_composition_invariant_kind: "",
            last_composition_invariant_detail: "",
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
            startup_mode: "cold_no_catalog",
            startup_reveal_state: "catalog_progress_visible",
            revealed: false,
            input_enabled: false,
            reveal_ms: 0,
            input_enabled_ms: 0,
            frame_budget: FrameBudgetStatus::default(),
        });

        let text = fs::read_to_string(STATUS_PATH).expect("status json should be written");
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema"], "mister-magik-slint-status-v1");
        assert_eq!(value["screen"], "arcade");
        assert_eq!(value["frames"], 7);
        assert_eq!(value["idle"], false);
        assert_eq!(value["idle_loops"], 0);
        assert_eq!(value["fps_estimate"], 60.0);
        assert_eq!(value["catalog_ready"], false);
        assert_eq!(value["catalog_refresh_done"], true);
        assert_eq!(value["catalog_scan_visible"], true);
        assert_eq!(value["catalog_scan_message"], "Updating Library");
        assert_eq!(value["catalog_scan_title"], "Indexing library");
        assert_eq!(value["catalog_background_scan_visible"], true);
        assert_eq!(value["confirm_visible"], false);
        assert_eq!(value["loading_title"], "1942");
        assert_eq!(value["frame_budget"]["frames_total"], 0);
        assert!(!std::path::Path::new(&format!("{STATUS_PATH}.tmp")).exists());
    }
}
