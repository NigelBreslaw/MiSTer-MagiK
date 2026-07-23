// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Agent-readable runtime status and recent events.

use serde_json::{json, Value};
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::Path;
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
    pub status_sequence: u64,
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
    pub arcade_drawer_open: bool,
    pub arcade_drawer_level: &'a str,
    pub arcade_drawer_selected: usize,
    pub arcade_drawer_requested_hash: u64,
    pub arcade_drawer_rendered_hash: u64,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    pub recent_frames: Vec<FrameBudgetRecentFrame>,
    pub slow_frames: Vec<FrameBudgetSlowFrame>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameBudgetRecentFrame {
    pub frame: u64,
    pub wall_us: u64,
    pub prepare_us: u64,
    pub render_us: u64,
    pub custom_draw_us: u64,
    pub vsync_us: u64,
    pub present_us: u64,
    pub cpu_prepare_us: u64,
    pub cpu_render_us: u64,
    pub cpu_custom_draw_us: u64,
    pub cpu_vsync_us: u64,
    pub cpu_present_us: u64,
    pub process_cpu_us: u64,
    pub vsync_source: &'static str,
    pub vsync_miss_streak: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameBudgetSlowFrame {
    pub frame: u64,
    pub severity: &'static str,
    pub wall_us: u64,
    pub warning_us: u64,
    pub budget_us: u64,
    pub over_budget_us: u64,
    pub dominant_phase: &'static str,
    pub prepare_us: u64,
    pub render_us: u64,
    pub custom_draw_us: u64,
    pub vsync_us: u64,
    pub present_us: u64,
    pub present_bytes: u64,
    pub wasted_present_bytes: u64,
    pub copied_rows: u32,
    pub direct_preview_rows: u32,
    pub dirty_y0: u32,
    pub dirty_y1: u32,
    pub catalog_worker_us: u64,
    pub catalog_message_count: u32,
    pub catalog_backlog: u32,
    pub catalog_ready_deferred: bool,
    pub catalog_ready_deferred_age_us: u64,
    pub media_worker_us: u64,
    pub media_gate_us: u64,
    pub preview_schedule_us: u64,
    pub preview_apply_us: u64,
    pub preview_worker_drained: u32,
    pub preview_ready_processed: u32,
    pub preview_selected_processed: u32,
    pub preview_prefetch_processed: u32,
    pub preview_stale_results: u32,
    pub preview_cache_inserts: u32,
    pub preview_cache_evictions: u32,
    pub preview_failed_results: u32,
    pub preview_backlog: u32,
    pub status_write_due: bool,
    pub status_string_copy_us: u64,
    pub status_string_copy_bytes: u64,
    pub analytics_mode: &'static str,
    pub vsync_source: &'static str,
    pub vsync_miss_streak: u32,
    pub vsync_stale_hits: u32,
    pub vsync_wait_start_age_us: u64,
    pub vsync_accepted_hit_age_us: u64,
    pub frame_start_phase_us: u64,
    pub present_phase_us: u64,
}

pub fn event(name: &str, detail: impl std::fmt::Display) {
    let _ = create_dir_all(DIR);
    let row = event_value(
        name,
        &detail.to_string(),
        unix_ms(),
        boot_ms(),
        std::process::id(),
    );
    append_event_row(Path::new(EVENTS_PATH), &row);
}

fn append_event_row(path: &Path, row: &Value) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
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

fn event_value(name: &str, detail: &str, ts_unix_ms: u128, ts_boot_ms: u64, pid: u32) -> Value {
    json!({
        "ts_unix_ms": ts_unix_ms,
        "ts_boot_ms": ts_boot_ms,
        "source": "slint",
        "pid": pid,
        "event": name,
        "detail": detail,
    })
}

fn boot_ms() -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|value| value.split_whitespace().next()?.parse::<f64>().ok())
        .map(|seconds| (seconds * 1000.0).round() as u64)
        .unwrap_or(0)
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
    insert!("status_sequence", status.status_sequence);
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
    insert!("arcade_drawer_open", status.arcade_drawer_open);
    insert!("arcade_drawer_level", status.arcade_drawer_level);
    insert!("arcade_drawer_selected", status.arcade_drawer_selected);
    insert!(
        "arcade_drawer_requested_hash",
        status.arcade_drawer_requested_hash
    );
    insert!(
        "arcade_drawer_rendered_hash",
        status.arcade_drawer_rendered_hash
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
    map.insert(
        "frame_budget".to_string(),
        frame_budget_status_value(&status.frame_budget),
    );
    insert!("rss_kb", current_rss_kb());
    insert!("rss_hwm_kb", current_rss_hwm_kb());

    Value::Object(map)
}

fn frame_budget_status_value(status: &FrameBudgetStatus) -> Value {
    json!({
        "budget_us": status.budget_us,
        "frames_total": status.frames_total,
        "over_budget_total": status.over_budget_total,
        "over_20ms_total": status.over_20ms_total,
        "over_33ms_total": status.over_33ms_total,
        "max_wall_us": status.max_wall_us,
        "latest_over_budget_frame": status.latest_over_budget_frame,
        "latest_over_budget_wall_us": status.latest_over_budget_wall_us,
        "max_vsync_miss_streak": status.max_vsync_miss_streak,
        "vsync_total": status.vsync_total,
        "fallback_total": status.fallback_total,
        "timeout_total": status.timeout_total,
        "error_total": status.error_total,
        "window_frames": status.window_frames,
        "window_over_budget": status.window_over_budget,
        "window_over_20ms": status.window_over_20ms,
        "window_over_33ms": status.window_over_33ms,
        "window_max_wall_us": status.window_max_wall_us,
        "window_max_vsync_miss_streak": status.window_max_vsync_miss_streak,
        "window_prepare_us": status.window_prepare_us,
        "window_render_us": status.window_render_us,
        "window_custom_draw_us": status.window_custom_draw_us,
        "window_vsync_us": status.window_vsync_us,
        "window_present_us": status.window_present_us,
        "recent_frames": status.recent_frames.iter().map(frame_budget_recent_frame_value).collect::<Vec<_>>(),
        "slow_frames": status.slow_frames.iter().map(frame_budget_slow_frame_value).collect::<Vec<_>>(),
    })
}

fn frame_budget_recent_frame_value(frame: &FrameBudgetRecentFrame) -> Value {
    json!({
        "frame": frame.frame,
        "wall_us": frame.wall_us,
        "prepare_us": frame.prepare_us,
        "render_us": frame.render_us,
        "custom_draw_us": frame.custom_draw_us,
        "vsync_us": frame.vsync_us,
        "present_us": frame.present_us,
        "cpu_prepare_us": frame.cpu_prepare_us,
        "cpu_render_us": frame.cpu_render_us,
        "cpu_custom_draw_us": frame.cpu_custom_draw_us,
        "cpu_vsync_us": frame.cpu_vsync_us,
        "cpu_present_us": frame.cpu_present_us,
        "process_cpu_us": frame.process_cpu_us,
        "vsync_source": frame.vsync_source,
        "vsync_miss_streak": frame.vsync_miss_streak,
    })
}

fn frame_budget_slow_frame_value(frame: &FrameBudgetSlowFrame) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("frame".into(), json!(frame.frame));
    object.insert("severity".into(), json!(frame.severity));
    object.insert("wall_us".into(), json!(frame.wall_us));
    object.insert("warning_us".into(), json!(frame.warning_us));
    object.insert("budget_us".into(), json!(frame.budget_us));
    object.insert("over_budget_us".into(), json!(frame.over_budget_us));
    object.insert("dominant_phase".into(), json!(frame.dominant_phase));
    object.insert("prepare_us".into(), json!(frame.prepare_us));
    object.insert("render_us".into(), json!(frame.render_us));
    object.insert("custom_draw_us".into(), json!(frame.custom_draw_us));
    object.insert("vsync_us".into(), json!(frame.vsync_us));
    object.insert("present_us".into(), json!(frame.present_us));
    object.insert("present_bytes".into(), json!(frame.present_bytes));
    object.insert(
        "wasted_present_bytes".into(),
        json!(frame.wasted_present_bytes),
    );
    object.insert("copied_rows".into(), json!(frame.copied_rows));
    object.insert(
        "direct_preview_rows".into(),
        json!(frame.direct_preview_rows),
    );
    object.insert("dirty_y0".into(), json!(frame.dirty_y0));
    object.insert("dirty_y1".into(), json!(frame.dirty_y1));
    object.insert("catalog_worker_us".into(), json!(frame.catalog_worker_us));
    object.insert(
        "catalog_message_count".into(),
        json!(frame.catalog_message_count),
    );
    object.insert("catalog_backlog".into(), json!(frame.catalog_backlog));
    object.insert(
        "catalog_ready_deferred".into(),
        json!(frame.catalog_ready_deferred),
    );
    object.insert(
        "catalog_ready_deferred_age_us".into(),
        json!(frame.catalog_ready_deferred_age_us),
    );
    object.insert("media_worker_us".into(), json!(frame.media_worker_us));
    object.insert("media_gate_us".into(), json!(frame.media_gate_us));
    object.insert(
        "preview_schedule_us".into(),
        json!(frame.preview_schedule_us),
    );
    object.insert("preview_apply_us".into(), json!(frame.preview_apply_us));
    object.insert(
        "preview_worker_drained".into(),
        json!(frame.preview_worker_drained),
    );
    object.insert(
        "preview_ready_processed".into(),
        json!(frame.preview_ready_processed),
    );
    object.insert(
        "preview_selected_processed".into(),
        json!(frame.preview_selected_processed),
    );
    object.insert(
        "preview_prefetch_processed".into(),
        json!(frame.preview_prefetch_processed),
    );
    object.insert(
        "preview_stale_results".into(),
        json!(frame.preview_stale_results),
    );
    object.insert(
        "preview_cache_inserts".into(),
        json!(frame.preview_cache_inserts),
    );
    object.insert(
        "preview_cache_evictions".into(),
        json!(frame.preview_cache_evictions),
    );
    object.insert(
        "preview_failed_results".into(),
        json!(frame.preview_failed_results),
    );
    object.insert("preview_backlog".into(), json!(frame.preview_backlog));
    object.insert("status_write_due".into(), json!(frame.status_write_due));
    object.insert(
        "status_string_copy_us".into(),
        json!(frame.status_string_copy_us),
    );
    object.insert(
        "status_string_copy_bytes".into(),
        json!(frame.status_string_copy_bytes),
    );
    object.insert("analytics_mode".into(), json!(frame.analytics_mode));
    object.insert("vsync_source".into(), json!(frame.vsync_source));
    object.insert("vsync_miss_streak".into(), json!(frame.vsync_miss_streak));
    object.insert("vsync_stale_hits".into(), json!(frame.vsync_stale_hits));
    object.insert(
        "vsync_wait_start_age_us".into(),
        json!(frame.vsync_wait_start_age_us),
    );
    object.insert(
        "vsync_accepted_hit_age_us".into(),
        json!(frame.vsync_accepted_hit_age_us),
    );
    object.insert(
        "frame_start_phase_us".into(),
        json!(frame.frame_start_phase_us),
    );
    object.insert("present_phase_us".into(), json!(frame.present_phase_us));
    Value::Object(object)
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
        let value = event_value("first_frame", "catalog_ready=true", 1234, 5678, 99);
        assert_eq!(value["ts_unix_ms"], 1234);
        assert_eq!(value["ts_boot_ms"], 5678);
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
                status_sequence: 9,
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
                arcade_drawer_open: true,
                arcade_drawer_level: "Decades",
                arcade_drawer_selected: 0,
                arcade_drawer_requested_hash: 123,
                arcade_drawer_rendered_hash: 123,
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
                    recent_frames: vec![FrameBudgetRecentFrame {
                        frame: 42,
                        wall_us: 18_000,
                        prepare_us: 100,
                        render_us: 2_000,
                        custom_draw_us: 3_000,
                        vsync_us: 8_000,
                        present_us: 900,
                        cpu_prepare_us: 10,
                        cpu_render_us: 20,
                        cpu_custom_draw_us: 30,
                        cpu_vsync_us: 1,
                        cpu_present_us: 5,
                        process_cpu_us: 75,
                        vsync_source: "vsync",
                        vsync_miss_streak: 1,
                    }],
                    slow_frames: vec![FrameBudgetSlowFrame {
                        frame: 41,
                        severity: "drop",
                        wall_us: 22_000,
                        warning_us: 16_000,
                        budget_us: 16_667,
                        over_budget_us: 5_333,
                        dominant_phase: "custom-draw",
                        prepare_us: 100,
                        render_us: 2_000,
                        custom_draw_us: 10_000,
                        vsync_us: 8_000,
                        present_us: 900,
                        present_bytes: 640,
                        wasted_present_bytes: 128,
                        copied_rows: 10,
                        direct_preview_rows: 4,
                        dirty_y0: 12,
                        dirty_y1: 22,
                        catalog_worker_us: 77,
                        catalog_message_count: 3,
                        catalog_backlog: 2,
                        catalog_ready_deferred: true,
                        catalog_ready_deferred_age_us: 400,
                        media_worker_us: 88,
                        media_gate_us: 11,
                        preview_schedule_us: 55,
                        preview_apply_us: 66,
                        preview_worker_drained: 5,
                        preview_ready_processed: 4,
                        preview_selected_processed: 1,
                        preview_prefetch_processed: 3,
                        preview_stale_results: 1,
                        preview_cache_inserts: 4,
                        preview_cache_evictions: 2,
                        preview_failed_results: 1,
                        preview_backlog: 6,
                        status_write_due: true,
                        status_string_copy_us: 7,
                        status_string_copy_bytes: 96,
                        analytics_mode: "wall",
                        vsync_source: "fallback",
                        vsync_miss_streak: 2,
                        vsync_stale_hits: 3,
                        vsync_wait_start_age_us: 12_345,
                        vsync_accepted_hit_age_us: 456,
                        frame_start_phase_us: 7_890,
                        present_phase_us: 321,
                    }],
                },
            },
            5678,
            101,
        );

        assert_eq!(value["schema"], "mister-magik-slint-status-v1");
        assert_eq!(value["frame_budget"]["budget_us"], 16_667);
        assert_eq!(value["frame_budget"]["over_budget_total"], 2);
        assert_eq!(value["frame_budget"]["window_present_us"], 900);
        assert_eq!(value["frame_budget"]["recent_frames"][0]["frame"], 42);
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["process_cpu_us"],
            75
        );
        assert_eq!(value["frame_budget"]["slow_frames"][0]["frame"], 41);
        assert_eq!(value["frame_budget"]["slow_frames"][0]["severity"], "drop");
        assert_eq!(
            value["frame_budget"]["slow_frames"][0]["dominant_phase"],
            "custom-draw"
        );
        assert_eq!(
            value["frame_budget"]["slow_frames"][0]["catalog_message_count"],
            3
        );
        assert_eq!(
            value["frame_budget"]["slow_frames"][0]["status_string_copy_bytes"],
            96
        );
        assert_eq!(
            value["frame_budget"]["slow_frames"][0]["preview_worker_drained"],
            5
        );
        assert_eq!(
            value["frame_budget"]["slow_frames"][0]["preview_cache_evictions"],
            2
        );
        assert_eq!(
            value["frame_budget"]["slow_frames"][0]["analytics_mode"],
            "wall"
        );
        assert_eq!(
            value["frame_budget"]["slow_frames"][0]["vsync_wait_start_age_us"],
            12_345
        );
        assert_eq!(
            value["frame_budget"]["slow_frames"][0]["present_phase_us"],
            321
        );
        assert_eq!(value["ts_unix_ms"], 5678);
        assert_eq!(value["pid"], 101);
        assert_eq!(value["mode"], "ui");
        assert_eq!(value["scene"], "launcher");
        assert_eq!(value["screen"], "home");
        assert_eq!(value["frames"], 42);
        assert_eq!(value["idle"], true);
        assert_eq!(value["idle_loops"], 12);
        assert_eq!(value["status_sequence"], 9);
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
        assert_eq!(value["arcade_drawer_open"], true);
        assert_eq!(value["arcade_drawer_level"], "Decades");
        assert_eq!(value["arcade_drawer_selected"], 0);
        assert_eq!(value["arcade_drawer_requested_hash"], 123);
        assert_eq!(value["arcade_drawer_rendered_hash"], 123);
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
        let name = unique_name("coverage_event");
        let path = std::env::temp_dir().join(format!("{name}.jsonl"));
        let row = event_value(&name, "detail=ok", unix_ms(), boot_ms(), std::process::id());
        append_event_row(&path, &row);

        let text = fs::read_to_string(&path).expect("events jsonl should be written");
        let _ = fs::remove_file(path);
        let row: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(row["source"], "slint");
        assert_eq!(row["event"], name);
        assert_eq!(row["detail"], "detail=ok");
        assert!(row["ts_unix_ms"].as_u64().is_some());
        assert!(row["ts_boot_ms"].as_u64().is_some());
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
            status_sequence: 1,
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
            arcade_drawer_open: false,
            arcade_drawer_level: "Filters",
            arcade_drawer_selected: 0,
            arcade_drawer_requested_hash: 0,
            arcade_drawer_rendered_hash: 0,
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
