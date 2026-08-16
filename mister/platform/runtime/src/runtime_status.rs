// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Agent-readable runtime status and recent events.

use serde::Serialize;
use serde_json::{Value, json};
use std::fs::{OpenOptions, create_dir_all};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, TryLockError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DIR: &str = "/tmp/mister-magik";
const STATUS_PATH: &str = "/tmp/mister-magik/status.json";
const EVENTS_PATH: &str = "/tmp/mister-magik/events.jsonl";

macro_rules! launcher_status_types {
    (
        values { $( $value:ident : $value_ty:ty ),* $(,)? }
        strings { $( $string:ident ),* $(,)? }
    ) => {
        pub struct LauncherStatus<'a> {
            $( pub $value: $value_ty, )*
            $( pub $string: &'a str, )*
        }

        struct OwnedLauncherStatus {
            $( $value: $value_ty, )*
            $( $string: String, )*
        }

        impl From<LauncherStatus<'_>> for OwnedLauncherStatus {
            fn from(status: LauncherStatus<'_>) -> Self {
                Self {
                    $( $value: status.$value, )*
                    $( $string: status.$string.to_owned(), )*
                }
            }
        }
    };
}

launcher_status_types! {
    values {
        frames: u64,
        idle: bool,
        idle_loops: u64,
        status_sequence: u64,
        state_revision: u64,
        presented_state_revision: u64,
        action_sequence: u64,
        presented_action_sequence: u64,
        fps_estimate: f64,
        framebuffer_width: usize,
        framebuffer_height: usize,
        rolling_fps: f64,
        rolling_prepare_us: u64,
        rolling_render_us: u64,
        rolling_custom_draw_us: u64,
        rolling_vsync_us: u64,
        rolling_present_us: u64,
        rolling_rows: u64,
        last_frame_ms_ago: u64,
        vsync_period_us: u64,
        present_buffer: u8,
        latch_publish_us: u64,
        latch_sequence: u16,
        latch_flip_count: u16,
        latch_drop_count: u16,
        startup_intro: Option<StartupIntroCadenceStatus>,
        display_frozen: bool,
        catalog_ready: bool,
        catalog_games: usize,
        catalog_systems: usize,
        catalog_refresh_done: bool,
        catalog_worker_enabled: bool,
        selected_game_has_preview: bool,
        catalog_scan_visible: bool,
        catalog_scan_percent: i32,
        catalog_background_scan_visible: bool,
        confirm_visible: bool,
        confirm_selected: i32,
        arcade_selected: usize,
        arcade_visual_index: f32,
        arcade_scroll_y: i32,
        arcade_drawer_open: bool,
        arcade_drawer_selected: usize,
        arcade_drawer_requested_hash: u64,
        arcade_drawer_rendered_hash: u64,
        arcade_search_active: bool,
        arcade_search_results: usize,
        preview_transition_progress: f32,
        preview_presentation_generation: u64,
        screensaver_active_cards: usize,
        composition_recovery_count: u64,
        direct_layer_retirement_generation: u64,
        direct_layer_retirement_receipt_sequence: u16,
        direct_layer_retirement_receipt_slot: u8,
        direct_layer_retirement_receipt_route_epoch: u16,
        route_reassert_count: u64,
        last_route_reassert_frame: u64,
        last_route_reassert_ok: bool,
        input_pad_count: usize,
        active_pad_index: usize,
        last_input_ms_ago: u64,
        revealed: bool,
        input_enabled: bool,
        reveal_ms: u64,
        input_enabled_ms: u64,
        process_start_monotonic_us: u64,
        exact_context_monotonic_us: u64,
        preview_ready_monotonic_us: u64,
        first_correct_present_monotonic_us: u64,
        frame_budget: FrameBudgetStatus,
    }
    strings {
        build_package_version,
        build_version,
        build_number,
        build_source_revision,
        build_source_dirty,
        build_time,
        build_arch,
        scene,
        screen,
        effective_view,
        return_screen,
        menu_id,
        selected_item_id,
        active_collection_id,
        selected_system_id,
        selected_game_id,
        selected_game_title,
        preview_asset_key,
        catalog_generation,
        output_route,
        crt_font_experiment,
        vsync_source,
        present_backend,
        present_status,
        latch_failure_state,
        latch_failure_stage,
        latch_failure_reason,
        latch_failure_detail,
        catalog_refresh_policy,
        screensaver_profile_state,
        catalog_scan_message,
        catalog_scan_title,
        catalog_scan_detail,
        confirm_title,
        confirm_message,
        confirm_left_label,
        confirm_right_label,
        arcade_drawer_level,
        arcade_search_status,
        arcade_search_query,
        preview_cache_state,
        preview_presentation_state,
        preview_transition_effect,
        composition_state,
        direct_layer_retirement_state,
        direct_layer_retirement_obligations,
        direct_layer_retirement_receipt,
        last_composition_invariant_kind,
        last_composition_invariant_detail,
        bench_scenario,
        start_screen,
        lock_screen,
        last_route_reassert_error,
        launch_state,
        loading_title,
        active_pad_name,
        active_pad_path,
        last_raw_event,
        startup_mode,
        startup_reveal_state,
        return_source,
        return_phase,
        return_fallback_reason,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PresentationTelemetrySnapshotStatus {
    pub owned_vblank_count: u32,
    pub presented_vblank_count: u32,
    pub repeated_vblank_count: u32,
    pub ownership_loss_count: u32,
    pub active_sequence: u16,
    pub magik_ownership: bool,
    pub pending: bool,
    pub lifetime_invariant_valid: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StartupIntroCadenceStatus {
    pub schema: &'static str,
    pub source: &'static str,
    pub available: bool,
    pub qualified: bool,
    pub dropped_frames: Option<u64>,
    pub software_estimated_dropped_frames: u64,
    pub confirmed_frames: u64,
    pub cabinet_wait_frames: u64,
    pub expected_refresh_intervals: u64,
    pub pacing_failures: u64,
    pub max_confirmation_gap_us: u64,
    pub elapsed_us: Option<u64>,
    pub owned_vblank_delta: Option<u32>,
    pub presented_vblank_delta: Option<u32>,
    pub repeated_vblank_delta: Option<u32>,
    pub ownership_loss_delta: Option<u32>,
    pub start: Option<PresentationTelemetrySnapshotStatus>,
    pub end: Option<PresentationTelemetrySnapshotStatus>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FrameBudgetRecentFrame {
    pub frame: u64,
    pub screensaver_active: bool,
    pub screensaver_active_cards: usize,
    pub screensaver_renderer: &'static str,
    pub navigation_transition_edge: &'static str,
    pub navigation_transition_route: &'static str,
    pub navigation_transition_direction: &'static str,
    pub navigation_transition_renderer: &'static str,
    pub navigation_transition_orientation: &'static str,
    pub settings_navigation_benchmark_leg: u8,
    pub navigation_transition_us: u64,
    pub navigation_transition_base_copy_us: u64,
    pub navigation_transition_settings_blit_us: u64,
    pub navigation_transition_card_scale_us: u64,
    pub navigation_transition_destination_reveal_us: u64,
    pub navigation_transition_overlay_us: u64,
    pub navigation_snapshot_locked: bool,
    pub navigation_slint_render_called: bool,
    pub navigation_status_quiesce_wait_us: u64,
    pub navigation_status_quiesce_timeout: bool,
    pub orientation_transition_active: bool,
    pub orientation_transition_leg: u8,
    pub orientation_transition_effect: &'static str,
    pub orientation_transition_from: &'static str,
    pub orientation_transition_to: &'static str,
    pub orientation_begin_us: u64,
    pub orientation_source_snapshot_us: u64,
    pub orientation_layout_us: u64,
    pub orientation_controlled_slint_raster_us: u64,
    pub orientation_transition_destination_capture_us: u64,
    pub orientation_damage_rotation_us: u64,
    pub orientation_damage_build_us: u64,
    pub orientation_source_snapshot_bytes: u64,
    pub orientation_destination_snapshot_bytes: u64,
    pub orientation_effect_read_bytes: u64,
    pub orientation_effect_write_bytes: u64,
    pub orientation_damage_rects_before: u32,
    pub orientation_damage_rects_after: u32,
    pub orientation_transition_fill_us: u64,
    pub orientation_transition_map_us: u64,
    pub orientation_transition_crossfade_us: u64,
    pub orientation_transition_cache_restore_us: u64,
    pub orientation_transition_total_us: u64,
    pub orientation_transition_mapped_pixels: u64,
    pub orientation_transition_blended_pixels: u64,
    pub orientation_transition_progress_ppm: u32,
    pub crt_backdrop_prepare_us: u64,
    pub crt_backdrop_prepare_pixels: u32,
    pub crt_backdrop_blend_us: u64,
    pub crt_backdrop_blend_pixels: u32,
    pub wall_us: u64,
    pub prepare_us: u64,
    pub slint_timer_dispatch_us: u64,
    pub navigation_commit_us: u64,
    pub bridge_sync_us: u64,
    pub unattributed_prepare_us: u64,
    pub render_us: u64,
    pub custom_draw_us: u64,
    pub vsync_us: u64,
    pub present_us: u64,
    pub cpu_prepare_us: u64,
    pub cpu_render_us: u64,
    pub cpu_custom_draw_us: u64,
    pub cpu_vsync_us: u64,
    pub cpu_frame_tail_us: u64,
    pub process_cpu_us: u64,
    pub completion_monotonic_us: u64,
    pub vsync_source: &'static str,
    pub vsync_period_us: u64,
    pub vsync_miss_streak: u32,
    pub vsync_stale_hits: u32,
    pub vsync_wait_start_age_us: u64,
    pub vsync_accepted_hit_age_us: u64,
    pub frame_start_phase_us: u64,
    pub present_phase_us: u64,
    pub main_present_status: &'static str,
    pub main_present_copy_path: &'static str,
    pub main_present_request_us: u64,
    pub main_present_sequence: u16,
    pub main_present_post_active_sequence: u16,
    pub main_present_post_pending_sequence: u16,
    pub main_present_post_pending: bool,
    pub main_present_active_sequence: u16,
    pub main_present_pending: bool,
    pub main_present_completion_poll_count: u16,
    pub main_present_completion_poll_wall_us: u64,
    pub main_present_completion_poll_cpu_us: u64,
    pub main_present_hidden_copy_us: u64,
    pub main_present_flip_count: u16,
    pub main_present_drop_count: u16,
    pub status_write_due: bool,
    pub runtime_status_write_us: u64,
    pub status_publish_mode: &'static str,
    pub status_enqueue_us: u64,
    pub status_worker_write_us: u64,
    pub status_replaced_count: u64,
    pub status_submitted_sequence: u64,
    pub status_written_sequence: u64,
    pub status_worker_errors: u64,
    pub status_worker_active: bool,
    pub clock_update_due: bool,
    pub clock_update_us: u64,
    pub screensaver_archive_poll_us: u64,
    pub screensaver_card_adopt_us: u64,
    pub screensaver_parade_advance_us: u64,
    pub screensaver_background_us: u64,
    pub screensaver_draw_order_us: u64,
    pub screensaver_tile_blit_us: u64,
    pub screensaver_raster_held_cards: u64,
    pub screensaver_raster_moved_cards: u64,
    pub screensaver_raster_hold_layer_mask: u8,
    pub screensaver_raster_visible_layer_mask: u8,
    pub screensaver_phase_bank_bytes: u64,
    pub frame_production_class: &'static str,
    pub frame_production_sequence: u64,
    pub frame_production_render_start_phase_us: u64,
    pub frame_production_ready_depth: u64,
    pub frame_production_ready_age_us: u64,
    pub frame_production_render_wall_us: u64,
    pub frame_production_starvation_count: u64,
    pub frame_production_cancelled: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
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
    pub slint_timer_dispatch_us: u64,
    pub navigation_commit_us: u64,
    pub bridge_sync_us: u64,
    pub unattributed_prepare_us: u64,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeStatusPublisherMetrics {
    pub submitted_sequence: u64,
    pub written_sequence: u64,
    pub replaced_count: u64,
    pub last_worker_duration_us: u64,
    pub worker_errors: u64,
    pub worker_active: bool,
}

#[derive(Default)]
struct RuntimeStatusPublisherCounters {
    submitted_sequence: AtomicU64,
    written_sequence: AtomicU64,
    replaced_count: AtomicU64,
    last_worker_duration_us: AtomicU64,
    worker_errors: AtomicU64,
    worker_active: AtomicBool,
}

pub struct RuntimeStatusPublisher {
    slot: Arc<RuntimeStatusSlot>,
    worker: Option<thread::JoinHandle<()>>,
    counters: Arc<RuntimeStatusPublisherCounters>,
}

#[derive(Default)]
struct RuntimeStatusSlotState {
    pending: Option<OwnedLauncherStatus>,
    shutdown: bool,
}

#[derive(Default)]
struct RuntimeStatusSlot {
    state: Mutex<RuntimeStatusSlotState>,
    ready: Condvar,
}

impl RuntimeStatusPublisher {
    pub fn new() -> Self {
        Self::spawn(PathBuf::from(STATUS_PATH), Duration::ZERO)
    }

    fn spawn(path: PathBuf, worker_delay: Duration) -> Self {
        let slot = Arc::new(RuntimeStatusSlot::default());
        let worker_slot = Arc::clone(&slot);
        let counters = Arc::new(RuntimeStatusPublisherCounters::default());
        let worker_counters = Arc::clone(&counters);
        let worker = thread::Builder::new()
            .name("runtime-status".into())
            .spawn(move || {
                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::RuntimeStatus,
                );
                let mut json_buffer = Vec::with_capacity(64 * 1024);
                loop {
                    let status = {
                        let mut state = worker_slot
                            .state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        while state.pending.is_none() && !state.shutdown {
                            state = worker_slot
                                .ready
                                .wait(state)
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                        }
                        match state.pending.take() {
                            Some(status) => status,
                            None if state.shutdown => break,
                            None => continue,
                        }
                    };
                    if !worker_delay.is_zero() {
                        thread::sleep(worker_delay);
                    }
                    worker_counters.worker_active.store(true, Ordering::Release);
                    write_owned_launcher_status(&path, status, &worker_counters, &mut json_buffer);
                    worker_counters
                        .worker_active
                        .store(false, Ordering::Release);
                }
            })
            .ok();
        if worker.is_none() {
            counters.worker_errors.fetch_add(1, Ordering::Relaxed);
        }
        Self {
            slot,
            worker,
            counters,
        }
    }

    pub fn submit(&self, status: LauncherStatus<'_>) -> bool {
        let sequence = status.status_sequence;
        let snapshot = OwnedLauncherStatus::from(status);
        let mut state = match self.slot.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::WouldBlock) => return false,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
        };
        if state.shutdown {
            return false;
        }
        if state.pending.replace(snapshot).is_some() {
            self.counters.replaced_count.fetch_add(1, Ordering::Relaxed);
        }
        self.counters
            .submitted_sequence
            .fetch_max(sequence, Ordering::Release);
        drop(state);
        self.slot.ready.notify_one();
        true
    }

    pub fn metrics(&self) -> RuntimeStatusPublisherMetrics {
        RuntimeStatusPublisherMetrics {
            submitted_sequence: self.counters.submitted_sequence.load(Ordering::Acquire),
            written_sequence: self.counters.written_sequence.load(Ordering::Acquire),
            replaced_count: self.counters.replaced_count.load(Ordering::Relaxed),
            last_worker_duration_us: self
                .counters
                .last_worker_duration_us
                .load(Ordering::Relaxed),
            worker_errors: self.counters.worker_errors.load(Ordering::Relaxed),
            worker_active: self.counters.worker_active.load(Ordering::Acquire),
        }
    }

    #[cfg(test)]
    fn new_for_test(path: PathBuf, worker_delay: Duration) -> Self {
        Self::spawn(path, worker_delay)
    }
}

impl Default for RuntimeStatusPublisher {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RuntimeStatusPublisher {
    fn drop(&mut self) {
        {
            let mut state = self
                .slot
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.shutdown = true;
        }
        self.slot.ready.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
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

fn write_owned_launcher_status(
    path: &Path,
    status: OwnedLauncherStatus,
    counters: &RuntimeStatusPublisherCounters,
    json_buffer: &mut Vec<u8>,
) {
    let started = Instant::now();
    let sequence = status.status_sequence;
    let result = (|| -> std::io::Result<()> {
        write_launcher_status_json(
            json_buffer,
            &status,
            unix_ms(),
            std::process::id(),
            counters,
        )
        .map_err(std::io::Error::other)?;
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        let tmp = PathBuf::from(format!("{}.tmp", path.display()));
        std::fs::write(&tmp, json_buffer.as_slice())?;
        std::fs::rename(tmp, path)
    })();
    let duration_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
    counters
        .last_worker_duration_us
        .store(duration_us, Ordering::Relaxed);
    if result.is_ok() {
        counters.written_sequence.store(sequence, Ordering::Release);
    } else {
        counters.worker_errors.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Serialize)]
struct RuntimeBuildStatus<'a> {
    package_version: &'a str,
    version: &'a str,
    build_number: &'a str,
    source_revision: &'a str,
    source_dirty: Option<bool>,
    build_time: &'a str,
    arch: &'a str,
}

#[derive(Serialize)]
struct CatalogRecoveryStatus<'a> {
    left: &'a str,
    right: &'a str,
    selected: i32,
}

#[derive(Serialize)]
struct CatalogFailureStatus<'a> {
    code: &'a str,
    detail: &'a str,
    usable_catalog: bool,
    report_path: &'static str,
    recovery: CatalogRecoveryStatus<'a>,
}

fn write_launcher_status_json(
    buffer: &mut Vec<u8>,
    status: &OwnedLauncherStatus,
    ts_unix_ms: u128,
    pid: u32,
    counters: &RuntimeStatusPublisherCounters,
) -> Result<(), serde_json::Error> {
    buffer.clear();
    buffer.push(b'{');
    let mut first = true;
    macro_rules! field {
        ($name:literal, $value:expr) => {{
            if !first {
                buffer.push(b',');
            }
            first = false;
            serde_json::to_writer(&mut *buffer, $name)?;
            buffer.push(b':');
            serde_json::to_writer(&mut *buffer, &$value)?;
        }};
    }

    field!("schema", "mister-magik-slint-status-v1");
    field!("ts_unix_ms", ts_unix_ms);
    field!("pid", pid);
    field!("mode", "ui");
    field!(
        "build",
        RuntimeBuildStatus {
            package_version: &status.build_package_version,
            version: &status.build_version,
            build_number: &status.build_number,
            source_revision: &status.build_source_revision,
            source_dirty: match status.build_source_dirty.as_str() {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            },
            build_time: &status.build_time,
            arch: &status.build_arch,
        }
    );
    field!("scene", status.scene);
    field!("screen", status.screen);
    field!("effective_view", status.effective_view);
    field!("return_screen", status.return_screen);
    field!("output_route", status.output_route);
    field!("crt_font_experiment", status.crt_font_experiment);
    field!("framebuffer_width", status.framebuffer_width);
    field!("framebuffer_height", status.framebuffer_height);
    field!("frames", status.frames);
    field!("idle", status.idle);
    field!("idle_loops", status.idle_loops);
    field!("status_sequence", status.status_sequence);
    field!("state_revision", status.state_revision);
    field!("presented_state_revision", status.presented_state_revision);
    field!("action_sequence", status.action_sequence);
    field!(
        "presented_action_sequence",
        status.presented_action_sequence
    );
    field!("menu_id", status.menu_id);
    field!("selected_item_id", status.selected_item_id);
    field!("active_collection_id", status.active_collection_id);
    field!("selected_system_id", status.selected_system_id);
    field!("selected_game_id", status.selected_game_id);
    field!("selected_game_title", status.selected_game_title);
    field!("preview_asset_key", status.preview_asset_key);
    field!("catalog_generation", status.catalog_generation);
    field!("fps_estimate", (status.fps_estimate * 10.0).round() / 10.0);
    field!("rolling_fps", (status.rolling_fps * 10.0).round() / 10.0);
    field!("rolling_prepare_us", status.rolling_prepare_us);
    field!("rolling_render_us", status.rolling_render_us);
    field!("rolling_custom_draw_us", status.rolling_custom_draw_us);
    field!("rolling_vsync_us", status.rolling_vsync_us);
    field!("rolling_present_us", status.rolling_present_us);
    field!("rolling_rows", status.rolling_rows);
    field!("last_frame_ms_ago", status.last_frame_ms_ago);
    field!("vsync_source", status.vsync_source);
    field!("vsync_period_us", status.vsync_period_us);
    field!("present_backend", status.present_backend);
    field!("present_status", status.present_status);
    field!("latch_failure_state", status.latch_failure_state);
    field!("latch_failure_stage", status.latch_failure_stage);
    field!("latch_failure_reason", status.latch_failure_reason);
    field!("latch_failure_detail", status.latch_failure_detail);
    field!("display_frozen", status.display_frozen);
    field!("present_buffer", status.present_buffer);
    field!("latch_publish_us", status.latch_publish_us);
    field!("latch_sequence", status.latch_sequence);
    field!("latch_flip_count", status.latch_flip_count);
    field!("latch_drop_count", status.latch_drop_count);
    field!("startup_intro", status.startup_intro);
    field!("catalog_ready", status.catalog_ready);
    field!("catalog_games", status.catalog_games);
    field!("catalog_systems", status.catalog_systems);
    field!("catalog_refresh_done", status.catalog_refresh_done);
    field!("catalog_refresh_policy", status.catalog_refresh_policy);
    field!("catalog_worker_enabled", status.catalog_worker_enabled);
    field!(
        "screensaver_profile_state",
        status.screensaver_profile_state
    );
    field!("catalog_scan_visible", status.catalog_scan_visible);
    field!("catalog_scan_message", status.catalog_scan_message);
    field!("catalog_scan_title", status.catalog_scan_title);
    field!("catalog_scan_detail", status.catalog_scan_detail);
    field!("catalog_scan_percent", status.catalog_scan_percent);
    field!(
        "catalog_background_scan_visible",
        status.catalog_background_scan_visible
    );
    field!("confirm_visible", status.confirm_visible);
    field!("confirm_title", status.confirm_title);
    field!("confirm_message", status.confirm_message);
    field!("confirm_selected", status.confirm_selected);
    field!("confirm_left_label", status.confirm_left_label);
    field!("confirm_right_label", status.confirm_right_label);
    let catalog_failure_code = match status.confirm_title.as_str() {
        "Catalog update required" => Some("projection_upgrade_required"),
        "Catalog repair required" => Some("catalog_repair_required"),
        "Catalog unavailable" => Some("catalog_load_failed"),
        "Catalog rebuild failed" => Some("catalog_persistence_failed"),
        _ => None,
    };
    let catalog_failure = catalog_failure_code.map(|code| CatalogFailureStatus {
        code,
        detail: &status.confirm_message,
        usable_catalog: status.catalog_ready,
        report_path: "diagnostics/catalog/latest.json",
        recovery: CatalogRecoveryStatus {
            left: &status.confirm_left_label,
            right: &status.confirm_right_label,
            selected: status.confirm_selected,
        },
    });
    field!("catalog_failure", catalog_failure);
    write_launcher_status_json_tail(buffer, status, counters, &mut first)?;
    buffer.extend_from_slice(b"}\n");
    Ok(())
}

fn write_launcher_status_json_tail(
    buffer: &mut Vec<u8>,
    status: &OwnedLauncherStatus,
    counters: &RuntimeStatusPublisherCounters,
    first: &mut bool,
) -> Result<(), serde_json::Error> {
    macro_rules! field {
        ($name:literal, $value:expr) => {{
            if !*first {
                buffer.push(b',');
            }
            *first = false;
            serde_json::to_writer(&mut *buffer, $name)?;
            buffer.push(b':');
            serde_json::to_writer(&mut *buffer, &$value)?;
        }};
    }
    field!("arcade_selected", status.arcade_selected);
    field!(
        "selected_game_has_preview",
        status.selected_game_has_preview
    );
    field!(
        "arcade_visual_index",
        (status.arcade_visual_index * 1000.0).round() / 1000.0
    );
    field!("arcade_scroll_y", status.arcade_scroll_y);
    field!("arcade_drawer_open", status.arcade_drawer_open);
    field!("arcade_drawer_level", status.arcade_drawer_level);
    field!("arcade_drawer_selected", status.arcade_drawer_selected);
    field!(
        "arcade_drawer_requested_hash",
        status.arcade_drawer_requested_hash
    );
    field!(
        "arcade_drawer_rendered_hash",
        status.arcade_drawer_rendered_hash
    );
    field!("arcade_search_active", status.arcade_search_active);
    field!("arcade_search_status", status.arcade_search_status);
    field!("arcade_search_query", status.arcade_search_query);
    field!("arcade_search_results", status.arcade_search_results);
    field!("preview_cache_state", status.preview_cache_state);
    field!(
        "preview_presentation_state",
        status.preview_presentation_state
    );
    field!(
        "preview_presentation_generation",
        status.preview_presentation_generation
    );
    field!("return_source", status.return_source);
    field!("return_phase", status.return_phase);
    field!("return_fallback_reason", status.return_fallback_reason);
    field!(
        "process_start_monotonic_us",
        status.process_start_monotonic_us
    );
    field!(
        "exact_context_monotonic_us",
        status.exact_context_monotonic_us
    );
    field!(
        "preview_ready_monotonic_us",
        status.preview_ready_monotonic_us
    );
    field!(
        "first_correct_present_monotonic_us",
        status.first_correct_present_monotonic_us
    );
    field!(
        "preview_transition_effect",
        status.preview_transition_effect
    );
    field!(
        "preview_transition_progress",
        (status.preview_transition_progress * 1000.0).round() / 1000.0
    );
    field!("screensaver_active_cards", status.screensaver_active_cards);
    field!("composition_state", status.composition_state);
    field!(
        "direct_layer_retirement_state",
        status.direct_layer_retirement_state
    );
    field!(
        "direct_layer_retirement_generation",
        status.direct_layer_retirement_generation
    );
    field!(
        "direct_layer_retirement_obligations",
        status.direct_layer_retirement_obligations
    );
    field!(
        "direct_layer_retirement_receipt",
        status.direct_layer_retirement_receipt
    );
    field!(
        "direct_layer_retirement_receipt_sequence",
        status.direct_layer_retirement_receipt_sequence
    );
    field!(
        "direct_layer_retirement_receipt_slot",
        status.direct_layer_retirement_receipt_slot
    );
    field!(
        "direct_layer_retirement_receipt_route_epoch",
        status.direct_layer_retirement_receipt_route_epoch
    );
    field!(
        "composition_recovery_count",
        status.composition_recovery_count
    );
    field!(
        "last_composition_invariant_kind",
        status.last_composition_invariant_kind
    );
    field!(
        "last_composition_invariant_detail",
        status.last_composition_invariant_detail
    );
    field!("bench_scenario", status.bench_scenario);
    field!("start_screen", status.start_screen);
    field!("lock_screen", status.lock_screen);
    field!("route_reassert_count", status.route_reassert_count);
    field!(
        "last_route_reassert_frame",
        status.last_route_reassert_frame
    );
    field!("last_route_reassert_ok", status.last_route_reassert_ok);
    field!(
        "last_route_reassert_error",
        status.last_route_reassert_error
    );
    field!("launch_state", status.launch_state);
    field!("loading_title", status.loading_title);
    field!("input_pad_count", status.input_pad_count);
    field!("active_pad_index", status.active_pad_index);
    field!("active_pad_name", status.active_pad_name);
    field!("active_pad_path", status.active_pad_path);
    field!("last_raw_event", status.last_raw_event);
    field!("last_input_ms_ago", status.last_input_ms_ago);
    field!("startup_mode", status.startup_mode);
    field!("startup_reveal_state", status.startup_reveal_state);
    field!("revealed", status.revealed);
    field!("input_enabled", status.input_enabled);
    field!("reveal_ms", status.reveal_ms);
    field!("input_enabled_ms", status.input_enabled_ms);
    field!("frame_budget", status.frame_budget);
    field!("rss_kb", current_rss_kb());
    field!("rss_hwm_kb", current_rss_hwm_kb());
    field!("status_publish_mode", "async");
    field!(
        "status_submitted_sequence",
        counters
            .submitted_sequence
            .load(Ordering::Acquire)
            .max(status.status_sequence)
    );
    field!("status_written_sequence", status.status_sequence);
    field!(
        "status_replaced_count",
        counters.replaced_count.load(Ordering::Relaxed)
    );
    field!(
        "status_worker_write_us",
        counters.last_worker_duration_us.load(Ordering::Relaxed)
    );
    field!(
        "status_worker_errors",
        counters.worker_errors.load(Ordering::Relaxed)
    );
    field!("status_worker_active", true);
    Ok(())
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

#[cfg(test)]
fn launcher_status_value(
    status: impl Into<OwnedLauncherStatus>,
    ts_unix_ms: u128,
    pid: u32,
) -> Value {
    let status = status.into();
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
    insert!(
        "build",
        json!({
            "package_version": status.build_package_version,
            "version": status.build_version,
            "build_number": status.build_number,
            "source_revision": status.build_source_revision,
            "source_dirty": match status.build_source_dirty.as_str() {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            },
            "build_time": status.build_time,
            "arch": status.build_arch,
        })
    );
    insert!("scene", status.scene);
    insert!("screen", status.screen);
    insert!("effective_view", status.effective_view);
    insert!("return_screen", status.return_screen);
    insert!("menu_id", status.menu_id);
    insert!("selected_item_id", status.selected_item_id);
    insert!("active_collection_id", status.active_collection_id);
    insert!("selected_system_id", status.selected_system_id);
    insert!("selected_game_id", status.selected_game_id);
    insert!("selected_game_title", status.selected_game_title);
    insert!("preview_asset_key", status.preview_asset_key);
    insert!("catalog_generation", status.catalog_generation);
    insert!("output_route", status.output_route);
    insert!("crt_font_experiment", status.crt_font_experiment);
    insert!("framebuffer_width", status.framebuffer_width);
    insert!("framebuffer_height", status.framebuffer_height);
    insert!("frames", status.frames);
    insert!("idle", status.idle);
    insert!("idle_loops", status.idle_loops);
    insert!("status_sequence", status.status_sequence);
    insert!("state_revision", status.state_revision);
    insert!("presented_state_revision", status.presented_state_revision);
    insert!("action_sequence", status.action_sequence);
    insert!(
        "presented_action_sequence",
        status.presented_action_sequence
    );
    insert!("fps_estimate", (status.fps_estimate * 10.0).round() / 10.0);
    insert!("rolling_fps", (status.rolling_fps * 10.0).round() / 10.0);
    insert!("rolling_prepare_us", status.rolling_prepare_us);
    insert!("rolling_render_us", status.rolling_render_us);
    insert!("rolling_custom_draw_us", status.rolling_custom_draw_us);
    insert!("rolling_vsync_us", status.rolling_vsync_us);
    insert!("rolling_present_us", status.rolling_present_us);
    insert!("rolling_rows", status.rolling_rows);
    insert!("last_frame_ms_ago", status.last_frame_ms_ago);
    insert!("vsync_source", status.vsync_source);
    insert!("vsync_period_us", status.vsync_period_us);
    insert!("present_backend", status.present_backend);
    insert!("present_status", status.present_status);
    insert!("latch_failure_state", status.latch_failure_state);
    insert!("latch_failure_stage", status.latch_failure_stage);
    insert!("latch_failure_reason", status.latch_failure_reason);
    insert!("latch_failure_detail", status.latch_failure_detail);
    insert!("display_frozen", status.display_frozen);
    insert!("present_buffer", status.present_buffer);
    insert!("latch_publish_us", status.latch_publish_us);
    insert!("latch_sequence", status.latch_sequence);
    insert!("latch_flip_count", status.latch_flip_count);
    insert!("latch_drop_count", status.latch_drop_count);
    insert!("startup_intro", status.startup_intro);
    insert!("catalog_ready", status.catalog_ready);
    insert!("catalog_games", status.catalog_games);
    insert!("catalog_systems", status.catalog_systems);
    insert!("catalog_refresh_done", status.catalog_refresh_done);
    insert!("catalog_refresh_policy", status.catalog_refresh_policy);
    insert!("catalog_worker_enabled", status.catalog_worker_enabled);
    insert!(
        "screensaver_profile_state",
        status.screensaver_profile_state
    );
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
    insert!("confirm_title", status.confirm_title.clone());
    insert!("confirm_message", status.confirm_message.clone());
    insert!("confirm_selected", status.confirm_selected);
    insert!("confirm_left_label", status.confirm_left_label.clone());
    insert!("confirm_right_label", status.confirm_right_label.clone());
    let catalog_failure_code = match status.confirm_title.as_str() {
        "Catalog update required" => Some("projection_upgrade_required"),
        "Catalog repair required" => Some("catalog_repair_required"),
        "Catalog unavailable" => Some("catalog_load_failed"),
        "Catalog rebuild failed" => Some("catalog_persistence_failed"),
        _ => None,
    };
    map.insert(
        "catalog_failure".to_string(),
        catalog_failure_code.map_or(Value::Null, |code| {
            json!({
                "code": code,
                "detail": status.confirm_message,
                "usable_catalog": status.catalog_ready,
                "report_path": "diagnostics/catalog/latest.json",
                "recovery": {
                    "left": status.confirm_left_label,
                    "right": status.confirm_right_label,
                    "selected": status.confirm_selected,
                },
            })
        }),
    );
    insert!("arcade_selected", status.arcade_selected);
    insert!(
        "selected_game_has_preview",
        status.selected_game_has_preview
    );
    insert!(
        "arcade_visual_index",
        (status.arcade_visual_index * 1000.0).round() / 1000.0
    );
    insert!("arcade_scroll_y", status.arcade_scroll_y);
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
    insert!("arcade_search_active", status.arcade_search_active);
    insert!("arcade_search_status", status.arcade_search_status);
    insert!("arcade_search_query", status.arcade_search_query);
    insert!("arcade_search_results", status.arcade_search_results);
    insert!("preview_cache_state", status.preview_cache_state);
    insert!(
        "preview_presentation_state",
        status.preview_presentation_state
    );
    insert!(
        "preview_presentation_generation",
        status.preview_presentation_generation
    );
    insert!("return_source", status.return_source);
    insert!("return_phase", status.return_phase);
    insert!("return_fallback_reason", status.return_fallback_reason);
    insert!(
        "process_start_monotonic_us",
        status.process_start_monotonic_us
    );
    insert!(
        "exact_context_monotonic_us",
        status.exact_context_monotonic_us
    );
    insert!(
        "preview_ready_monotonic_us",
        status.preview_ready_monotonic_us
    );
    insert!(
        "first_correct_present_monotonic_us",
        status.first_correct_present_monotonic_us
    );
    insert!(
        "preview_transition_effect",
        status.preview_transition_effect
    );
    insert!(
        "preview_transition_progress",
        (status.preview_transition_progress * 1000.0).round() / 1000.0
    );
    insert!("screensaver_active_cards", status.screensaver_active_cards);
    insert!("composition_state", status.composition_state);
    insert!(
        "direct_layer_retirement_state",
        status.direct_layer_retirement_state
    );
    insert!(
        "direct_layer_retirement_generation",
        status.direct_layer_retirement_generation
    );
    insert!(
        "direct_layer_retirement_obligations",
        status.direct_layer_retirement_obligations
    );
    insert!(
        "direct_layer_retirement_receipt",
        status.direct_layer_retirement_receipt
    );
    insert!(
        "direct_layer_retirement_receipt_sequence",
        status.direct_layer_retirement_receipt_sequence
    );
    insert!(
        "direct_layer_retirement_receipt_slot",
        status.direct_layer_retirement_receipt_slot
    );
    insert!(
        "direct_layer_retirement_receipt_route_epoch",
        status.direct_layer_retirement_receipt_route_epoch
    );
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

#[cfg(test)]
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

#[cfg(test)]
fn frame_budget_recent_frame_value(frame: &FrameBudgetRecentFrame) -> Value {
    let mut object = serde_json::Map::new();
    macro_rules! field {
        ($name:literal, $value:expr) => {
            object.insert($name.into(), json!($value));
        };
    }
    field!("frame", frame.frame);
    field!("screensaver_active", frame.screensaver_active);
    field!("screensaver_renderer", frame.screensaver_renderer);
    field!(
        "navigation_transition_edge",
        frame.navigation_transition_edge
    );
    field!(
        "navigation_transition_direction",
        frame.navigation_transition_direction
    );
    field!("navigation_transition_us", frame.navigation_transition_us);
    field!(
        "navigation_transition_overlay_us",
        frame.navigation_transition_overlay_us
    );
    field!(
        "navigation_snapshot_locked",
        frame.navigation_snapshot_locked
    );
    field!(
        "navigation_slint_render_called",
        frame.navigation_slint_render_called
    );
    field!(
        "navigation_status_quiesce_wait_us",
        frame.navigation_status_quiesce_wait_us
    );
    field!(
        "navigation_status_quiesce_timeout",
        frame.navigation_status_quiesce_timeout
    );
    field!(
        "orientation_transition_active",
        frame.orientation_transition_active
    );
    field!(
        "orientation_transition_leg",
        frame.orientation_transition_leg
    );
    field!(
        "orientation_transition_effect",
        frame.orientation_transition_effect
    );
    field!(
        "orientation_transition_from",
        frame.orientation_transition_from
    );
    field!("orientation_transition_to", frame.orientation_transition_to);
    field!("orientation_begin_us", frame.orientation_begin_us);
    field!(
        "orientation_source_snapshot_us",
        frame.orientation_source_snapshot_us
    );
    field!("orientation_layout_us", frame.orientation_layout_us);
    field!(
        "orientation_controlled_slint_raster_us",
        frame.orientation_controlled_slint_raster_us
    );
    field!(
        "orientation_transition_destination_capture_us",
        frame.orientation_transition_destination_capture_us
    );
    field!(
        "orientation_damage_rotation_us",
        frame.orientation_damage_rotation_us
    );
    field!(
        "orientation_damage_build_us",
        frame.orientation_damage_build_us
    );
    field!(
        "orientation_source_snapshot_bytes",
        frame.orientation_source_snapshot_bytes
    );
    field!(
        "orientation_destination_snapshot_bytes",
        frame.orientation_destination_snapshot_bytes
    );
    field!(
        "orientation_effect_read_bytes",
        frame.orientation_effect_read_bytes
    );
    field!(
        "orientation_effect_write_bytes",
        frame.orientation_effect_write_bytes
    );
    field!(
        "orientation_damage_rects_before",
        frame.orientation_damage_rects_before
    );
    field!(
        "orientation_damage_rects_after",
        frame.orientation_damage_rects_after
    );
    field!(
        "orientation_transition_fill_us",
        frame.orientation_transition_fill_us
    );
    field!(
        "orientation_transition_map_us",
        frame.orientation_transition_map_us
    );
    field!(
        "orientation_transition_crossfade_us",
        frame.orientation_transition_crossfade_us
    );
    field!(
        "orientation_transition_cache_restore_us",
        frame.orientation_transition_cache_restore_us
    );
    field!(
        "orientation_transition_total_us",
        frame.orientation_transition_total_us
    );
    field!(
        "orientation_transition_mapped_pixels",
        frame.orientation_transition_mapped_pixels
    );
    field!(
        "orientation_transition_blended_pixels",
        frame.orientation_transition_blended_pixels
    );
    field!(
        "orientation_transition_progress_ppm",
        frame.orientation_transition_progress_ppm
    );
    field!("wall_us", frame.wall_us);
    field!("prepare_us", frame.prepare_us);
    field!("slint_timer_dispatch_us", frame.slint_timer_dispatch_us);
    field!("navigation_commit_us", frame.navigation_commit_us);
    field!("bridge_sync_us", frame.bridge_sync_us);
    field!("unattributed_prepare_us", frame.unattributed_prepare_us);
    field!("render_us", frame.render_us);
    field!("custom_draw_us", frame.custom_draw_us);
    field!("vsync_us", frame.vsync_us);
    field!("present_us", frame.present_us);
    field!("cpu_prepare_us", frame.cpu_prepare_us);
    field!("cpu_render_us", frame.cpu_render_us);
    field!("cpu_custom_draw_us", frame.cpu_custom_draw_us);
    field!("cpu_vsync_us", frame.cpu_vsync_us);
    field!("cpu_frame_tail_us", frame.cpu_frame_tail_us);
    field!("process_cpu_us", frame.process_cpu_us);
    field!("completion_monotonic_us", frame.completion_monotonic_us);
    field!("vsync_source", frame.vsync_source);
    field!("vsync_period_us", frame.vsync_period_us);
    field!("vsync_miss_streak", frame.vsync_miss_streak);
    field!("vsync_stale_hits", frame.vsync_stale_hits);
    field!("vsync_wait_start_age_us", frame.vsync_wait_start_age_us);
    field!("vsync_accepted_hit_age_us", frame.vsync_accepted_hit_age_us);
    field!("frame_start_phase_us", frame.frame_start_phase_us);
    field!("present_phase_us", frame.present_phase_us);
    field!("main_present_status", frame.main_present_status);
    field!("main_present_copy_path", frame.main_present_copy_path);
    field!("main_present_request_us", frame.main_present_request_us);
    field!("main_present_sequence", frame.main_present_sequence);
    field!(
        "main_present_post_active_sequence",
        frame.main_present_post_active_sequence
    );
    field!(
        "main_present_post_pending_sequence",
        frame.main_present_post_pending_sequence
    );
    field!("main_present_post_pending", frame.main_present_post_pending);
    field!(
        "main_present_active_sequence",
        frame.main_present_active_sequence
    );
    field!("main_present_pending", frame.main_present_pending);
    field!(
        "main_present_completion_poll_count",
        frame.main_present_completion_poll_count
    );
    field!(
        "main_present_completion_poll_wall_us",
        frame.main_present_completion_poll_wall_us
    );
    field!(
        "main_present_completion_poll_cpu_us",
        frame.main_present_completion_poll_cpu_us
    );
    field!("main_present_flip_count", frame.main_present_flip_count);
    field!("main_present_drop_count", frame.main_present_drop_count);
    field!("status_write_due", frame.status_write_due);
    field!("runtime_status_write_us", frame.runtime_status_write_us);
    field!("status_publish_mode", frame.status_publish_mode);
    field!("status_enqueue_us", frame.status_enqueue_us);
    field!("status_worker_write_us", frame.status_worker_write_us);
    field!("status_replaced_count", frame.status_replaced_count);
    field!("status_submitted_sequence", frame.status_submitted_sequence);
    field!("status_written_sequence", frame.status_written_sequence);
    field!("status_worker_errors", frame.status_worker_errors);
    field!("status_worker_active", frame.status_worker_active);
    field!("clock_update_due", frame.clock_update_due);
    field!("clock_update_us", frame.clock_update_us);
    field!(
        "screensaver_archive_poll_us",
        frame.screensaver_archive_poll_us
    );
    field!("screensaver_card_adopt_us", frame.screensaver_card_adopt_us);
    field!(
        "screensaver_parade_advance_us",
        frame.screensaver_parade_advance_us
    );
    field!("screensaver_background_us", frame.screensaver_background_us);
    field!("screensaver_draw_order_us", frame.screensaver_draw_order_us);
    field!("screensaver_tile_blit_us", frame.screensaver_tile_blit_us);
    field!(
        "screensaver_raster_held_cards",
        frame.screensaver_raster_held_cards
    );
    field!(
        "screensaver_raster_moved_cards",
        frame.screensaver_raster_moved_cards
    );
    field!(
        "screensaver_raster_hold_layer_mask",
        frame.screensaver_raster_hold_layer_mask
    );
    field!(
        "screensaver_raster_visible_layer_mask",
        frame.screensaver_raster_visible_layer_mask
    );
    field!(
        "screensaver_phase_bank_bytes",
        frame.screensaver_phase_bank_bytes
    );
    field!("frame_production_class", frame.frame_production_class);
    field!("frame_production_sequence", frame.frame_production_sequence);
    field!(
        "frame_production_render_start_phase_us",
        frame.frame_production_render_start_phase_us
    );
    field!(
        "frame_production_ready_depth",
        frame.frame_production_ready_depth
    );
    field!(
        "frame_production_ready_age_us",
        frame.frame_production_ready_age_us
    );
    field!(
        "frame_production_render_wall_us",
        frame.frame_production_render_wall_us
    );
    field!(
        "frame_production_starvation_count",
        frame.frame_production_starvation_count
    );
    field!(
        "frame_production_cancelled",
        frame.frame_production_cancelled
    );
    Value::Object(object)
}

#[cfg(test)]
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
    object.insert(
        "slint_timer_dispatch_us".into(),
        json!(frame.slint_timer_dispatch_us),
    );
    object.insert(
        "navigation_commit_us".into(),
        json!(frame.navigation_commit_us),
    );
    object.insert("bridge_sync_us".into(), json!(frame.bridge_sync_us));
    object.insert(
        "unattributed_prepare_us".into(),
        json!(frame.unattributed_prepare_us),
    );
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
                build_package_version: "0.1.0",
                build_version: "0.2.2429",
                build_number: "2429",
                build_source_revision: "deadbeef",
                build_source_dirty: "0",
                build_time: "28.7.2026 12:00",
                build_arch: "arm",
                scene: "launcher",
                screen: "home",
                effective_view: "home",
                return_screen: "home",
                menu_id: "menu:root",
                selected_item_id: "collection:arcade",
                active_collection_id: "collection:arcade",
                selected_system_id: "arcade",
                selected_game_id: "/media/fat/_Arcade/1942.mra",
                selected_game_title: "1942",
                preview_asset_key: "1942",
                catalog_generation: "generation-4",
                output_route: "crt-576p50",
                crt_font_experiment: "baseline",
                frames: 42,
                idle: true,
                idle_loops: 12,
                status_sequence: 9,
                state_revision: 12,
                presented_state_revision: 12,
                action_sequence: 4,
                presented_action_sequence: 4,
                fps_estimate: 59.94,
                framebuffer_width: 640,
                framebuffer_height: 576,
                rolling_fps: 60.0,
                rolling_prepare_us: 1,
                rolling_render_us: 2,
                rolling_custom_draw_us: 3,
                rolling_vsync_us: 4,
                rolling_present_us: 5,
                rolling_rows: 6,
                last_frame_ms_ago: 7,
                vsync_source: "vsync",
                vsync_period_us: 19_829,
                present_backend: "fpga-vblank-latch-hidden",
                present_status: "ok",
                latch_failure_state: "runtime-fault",
                latch_failure_stage: "post-verification",
                latch_failure_reason: "posted-sequence-unverified",
                latch_failure_detail: "posted=219 final_active=218",
                present_buffer: 2,
                latch_publish_us: 3,
                latch_sequence: 41,
                latch_flip_count: 40,
                latch_drop_count: 0,
                startup_intro: None,
                display_frozen: true,
                catalog_ready: true,
                catalog_games: 9014,
                catalog_systems: 13,
                catalog_refresh_done: false,
                catalog_refresh_policy: "off",
                catalog_worker_enabled: false,
                selected_game_has_preview: true,
                screensaver_profile_state: "active",
                catalog_scan_visible: false,
                catalog_scan_message: "Scanning for games",
                catalog_scan_title: "",
                catalog_scan_detail: "",
                catalog_scan_percent: -1,
                catalog_background_scan_visible: false,
                confirm_visible: true,
                confirm_title: "Catalog rebuild failed",
                confirm_message: "Could not publish the new catalog.",
                confirm_selected: 0,
                confirm_left_label: "Continue",
                confirm_right_label: "Full rebuild",
                arcade_selected: 3,
                arcade_visual_index: 3.25,
                arcade_scroll_y: 72,
                arcade_drawer_open: true,
                arcade_drawer_level: "Decades",
                arcade_drawer_selected: 0,
                arcade_drawer_requested_hash: 123,
                arcade_drawer_rendered_hash: 123,
                arcade_search_active: true,
                arcade_search_status: "ready",
                arcade_search_query: "A",
                arcade_search_results: 42,
                preview_cache_state: "exact",
                preview_presentation_state: "visible",
                preview_presentation_generation: 7,
                preview_transition_effect: "fade",
                preview_transition_progress: 0.5,
                screensaver_active_cards: 73,
                composition_state: "mixed-arcade",
                composition_recovery_count: 0,
                direct_layer_retirement_state: "idle",
                direct_layer_retirement_generation: 0,
                direct_layer_retirement_obligations: "none",
                direct_layer_retirement_receipt: "",
                direct_layer_retirement_receipt_sequence: 0,
                direct_layer_retirement_receipt_slot: 0,
                direct_layer_retirement_receipt_route_epoch: 0,
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
                return_source: "return-capsule",
                return_phase: "complete",
                return_fallback_reason: "",
                revealed: true,
                input_enabled: true,
                reveal_ms: 37,
                input_enabled_ms: 37,
                process_start_monotonic_us: 1_000,
                exact_context_monotonic_us: 1_100,
                preview_ready_monotonic_us: 1_200,
                first_correct_present_monotonic_us: 1_300,
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
                        screensaver_active: true,
                        screensaver_renderer: "parade",
                        orientation_transition_active: true,
                        orientation_transition_leg: 2,
                        orientation_transition_from: "monitor-clockwise",
                        orientation_transition_to: "monitor-counterclockwise",
                        orientation_begin_us: 101,
                        orientation_source_snapshot_us: 102,
                        orientation_layout_us: 103,
                        orientation_controlled_slint_raster_us: 104,
                        orientation_transition_destination_capture_us: 111,
                        orientation_damage_rotation_us: 105,
                        orientation_damage_build_us: 106,
                        orientation_source_snapshot_bytes: 1_843_200,
                        orientation_destination_snapshot_bytes: 1_843_200,
                        orientation_effect_read_bytes: 640,
                        orientation_effect_write_bytes: 640,
                        orientation_damage_rects_before: 9,
                        orientation_damage_rects_after: 7,
                        orientation_transition_fill_us: 222,
                        orientation_transition_map_us: 3_333,
                        orientation_transition_crossfade_us: 444,
                        orientation_transition_cache_restore_us: 555,
                        orientation_transition_total_us: 4_665,
                        orientation_transition_mapped_pixels: 700_000,
                        orientation_transition_blended_pixels: 921_600,
                        orientation_transition_progress_ppm: 800_000,
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
                        cpu_frame_tail_us: 5,
                        process_cpu_us: 75,
                        completion_monotonic_us: 123_456,
                        vsync_source: "vsync",
                        vsync_period_us: 16_667,
                        vsync_miss_streak: 1,
                        frame_start_phase_us: 2_500,
                        present_phase_us: 9_500,
                        main_present_status: "ok",
                        main_present_request_us: 500,
                        main_present_sequence: 17,
                        main_present_post_active_sequence: 16,
                        main_present_post_pending_sequence: 17,
                        main_present_post_pending: true,
                        main_present_active_sequence: 17,
                        main_present_flip_count: 18,
                        status_write_due: true,
                        runtime_status_write_us: 321,
                        clock_update_due: true,
                        clock_update_us: 45,
                        screensaver_raster_held_cards: 2,
                        screensaver_raster_moved_cards: 8,
                        screensaver_raster_hold_layer_mask: 1,
                        screensaver_raster_visible_layer_mask: 3,
                        frame_production_class: "prepared",
                        frame_production_sequence: 99,
                        frame_production_render_start_phase_us: 3_250,
                        frame_production_ready_depth: 2,
                        frame_production_ready_age_us: 400,
                        frame_production_render_wall_us: 4_500,
                        ..FrameBudgetRecentFrame::default()
                    }],
                    slow_frames: vec![FrameBudgetSlowFrame {
                        frame: 41,
                        severity: "cadence-overrun",
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
                        slint_timer_dispatch_us: 0,
                        navigation_commit_us: 0,
                        bridge_sync_us: 0,
                        unattributed_prepare_us: 0,
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
            value["frame_budget"]["recent_frames"][0]["screensaver_active"],
            true
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["vsync_period_us"],
            16_667
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["process_cpu_us"],
            75
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["main_present_sequence"],
            17
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["main_present_post_pending_sequence"],
            17
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["main_present_post_pending"],
            true
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["runtime_status_write_us"],
            321
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["screensaver_raster_held_cards"],
            2
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["screensaver_renderer"],
            "parade"
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["frame_production_class"],
            "prepared"
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["frame_production_sequence"],
            99
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["frame_production_render_start_phase_us"],
            3_250
        );
        assert!(
            value["frame_budget"]["recent_frames"][0]
                .get("screensaver_render_ahead_sequence")
                .is_none()
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["orientation_transition_leg"],
            2
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["orientation_transition_total_us"],
            4_665
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["orientation_controlled_slint_raster_us"],
            104
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["orientation_damage_rotation_us"],
            105
        );
        assert_eq!(
            value["frame_budget"]["recent_frames"][0]["orientation_source_snapshot_bytes"],
            1_843_200
        );
        assert_eq!(value["frame_budget"]["slow_frames"][0]["frame"], 41);
        assert_eq!(
            value["frame_budget"]["slow_frames"][0]["severity"],
            "cadence-overrun"
        );
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
        assert_eq!(value["build"]["version"], "0.2.2429");
        assert_eq!(value["build"]["build_number"], "2429");
        assert_eq!(value["build"]["source_revision"], "deadbeef");
        assert_eq!(value["build"]["source_dirty"], false);
        assert_eq!(value["build"]["arch"], "arm");
        assert_eq!(value["scene"], "launcher");
        assert_eq!(value["screen"], "home");
        assert_eq!(value["output_route"], "crt-576p50");
        assert_eq!(value["framebuffer_width"], 640);
        assert_eq!(value["framebuffer_height"], 576);
        assert_eq!(value["vsync_period_us"], 19_829);
        assert_eq!(value["present_backend"], "fpga-vblank-latch-hidden");
        assert_eq!(value["latch_failure_state"], "runtime-fault");
        assert_eq!(value["latch_failure_reason"], "posted-sequence-unverified");
        assert_eq!(value["display_frozen"], true);
        assert_eq!(value["latch_publish_us"], 3);
        assert_eq!(value["latch_sequence"], 41);
        assert_eq!(value["latch_flip_count"], 40);
        assert_eq!(value["latch_drop_count"], 0);
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
        assert_eq!(value["confirm_title"], "Catalog rebuild failed");
        assert_eq!(value["confirm_selected"], 0);
        assert_eq!(value["confirm_left_label"], "Continue");
        assert_eq!(value["confirm_right_label"], "Full rebuild");
        assert_eq!(
            value["catalog_failure"]["code"],
            "catalog_persistence_failed"
        );
        assert_eq!(value["catalog_failure"]["usable_catalog"], true);
        assert_eq!(value["arcade_selected"], 3);
        assert_eq!(value["arcade_visual_index"], 3.25);
        assert_eq!(value["arcade_drawer_open"], true);
        assert_eq!(value["arcade_drawer_level"], "Decades");
        assert_eq!(value["arcade_drawer_selected"], 0);
        assert_eq!(value["arcade_drawer_requested_hash"], 123);
        assert_eq!(value["arcade_drawer_rendered_hash"], 123);
        assert_eq!(value["arcade_search_active"], true);
        assert_eq!(value["arcade_search_status"], "ready");
        assert_eq!(value["arcade_search_query"], "A");
        assert_eq!(value["arcade_search_results"], 42);
        assert_eq!(value["preview_cache_state"], "exact");
        assert_eq!(value["preview_presentation_state"], "visible");
        assert_eq!(value["preview_presentation_generation"], 7);
        assert_eq!(value["preview_transition_effect"], "fade");
        assert_eq!(value["preview_transition_progress"], 0.5);
        assert_eq!(value["screensaver_active_cards"], 73);
        assert_eq!(value["composition_state"], "mixed-arcade");
        assert_eq!(value["composition_recovery_count"], 0);
        assert_eq!(value["direct_layer_retirement_state"], "idle");
        assert_eq!(value["direct_layer_retirement_generation"], 0);
        assert_eq!(value["direct_layer_retirement_obligations"], "none");
        assert_eq!(value["direct_layer_retirement_receipt"], "");
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

    fn publisher_status(
        status_sequence: u64,
        screen: &'static str,
        screensaver_profile_state: &'static str,
    ) -> LauncherStatus<'static> {
        LauncherStatus {
            build_package_version: "0.1.0",
            build_version: "0.2.2429",
            build_number: "2429",
            build_source_revision: "deadbeef",
            build_source_dirty: "0",
            build_time: "28.7.2026 12:00",
            build_arch: "arm",
            scene: "launcher",
            screen,
            effective_view: screen,
            return_screen: screen,
            menu_id: "menu:root",
            selected_item_id: "collection:arcade",
            active_collection_id: "collection:arcade",
            selected_system_id: "arcade",
            selected_game_id: "/media/fat/_Arcade/1942.mra",
            selected_game_title: "1942",
            preview_asset_key: "1942",
            catalog_generation: "generation-4",
            output_route: "crt-288p50",
            crt_font_experiment: "baseline",
            frames: 7,
            idle: false,
            idle_loops: 0,
            status_sequence,
            state_revision: status_sequence,
            presented_state_revision: status_sequence,
            action_sequence: status_sequence,
            presented_action_sequence: status_sequence,
            fps_estimate: 60.04,
            framebuffer_width: 640,
            framebuffer_height: 288,
            rolling_fps: 59.9,
            rolling_prepare_us: 11,
            rolling_render_us: 22,
            rolling_custom_draw_us: 33,
            rolling_vsync_us: 44,
            rolling_present_us: 55,
            rolling_rows: 66,
            last_frame_ms_ago: 1,
            vsync_source: "fallback",
            vsync_period_us: 19_830,
            present_backend: "fpga-vblank-latch-hidden",
            present_status: "ok",
            latch_failure_state: "",
            latch_failure_stage: "",
            latch_failure_reason: "",
            latch_failure_detail: "",
            present_buffer: 1,
            latch_publish_us: 4,
            latch_sequence: 7,
            latch_flip_count: 6,
            latch_drop_count: 0,
            startup_intro: None,
            display_frozen: false,
            catalog_ready: false,
            catalog_games: 12,
            catalog_systems: 2,
            catalog_refresh_done: true,
            catalog_refresh_policy: "default",
            catalog_worker_enabled: true,
            selected_game_has_preview: false,
            screensaver_profile_state,
            catalog_scan_visible: true,
            catalog_scan_message: "Updating Library",
            catalog_scan_title: "Indexing library",
            catalog_scan_detail: "Games found: 12",
            catalog_scan_percent: -1,
            catalog_background_scan_visible: true,
            confirm_visible: false,
            confirm_title: "",
            confirm_message: "",
            confirm_selected: 0,
            confirm_left_label: "",
            confirm_right_label: "",
            arcade_selected: 0,
            arcade_visual_index: 0.0,
            arcade_scroll_y: 0,
            arcade_drawer_open: false,
            arcade_drawer_level: "Filters",
            arcade_drawer_selected: 0,
            arcade_drawer_requested_hash: 0,
            arcade_drawer_rendered_hash: 0,
            arcade_search_active: false,
            arcade_search_status: "idle",
            arcade_search_query: "",
            arcade_search_results: 0,
            preview_cache_state: "placeholder",
            preview_presentation_state: "detached",
            preview_presentation_generation: 0,
            preview_transition_effect: "fade",
            preview_transition_progress: 1.0,
            screensaver_active_cards: 0,
            composition_state: "full-slint",
            composition_recovery_count: 0,
            direct_layer_retirement_state: "idle",
            direct_layer_retirement_generation: 0,
            direct_layer_retirement_obligations: "none",
            direct_layer_retirement_receipt: "",
            direct_layer_retirement_receipt_sequence: 0,
            direct_layer_retirement_receipt_slot: 0,
            direct_layer_retirement_receipt_route_epoch: 0,
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
            return_source: "none",
            return_phase: "none",
            return_fallback_reason: "",
            revealed: false,
            input_enabled: false,
            reveal_ms: 0,
            input_enabled_ms: 0,
            process_start_monotonic_us: 0,
            exact_context_monotonic_us: 0,
            preview_ready_monotonic_us: 0,
            first_correct_present_monotonic_us: 0,
            frame_budget: FrameBudgetStatus::default(),
        }
    }

    #[test]
    fn streamed_launcher_status_is_value_equivalent_to_legacy_document() {
        let counters = RuntimeStatusPublisherCounters::default();
        let mut bytes = Vec::new();
        let owned = OwnedLauncherStatus::from(publisher_status(1, "arcade", "disabled"));
        write_launcher_status_json(&mut bytes, &owned, 123, 99, &counters).unwrap();
        let mut streamed: Value = serde_json::from_slice(&bytes).unwrap();
        let mut expected =
            launcher_status_value(publisher_status(1, "arcade", "disabled"), 123, 99);
        let map = expected.as_object_mut().unwrap();
        map.insert("status_publish_mode".into(), json!("async"));
        map.insert("status_submitted_sequence".into(), json!(1));
        map.insert("status_written_sequence".into(), json!(1));
        map.insert("status_replaced_count".into(), json!(0));
        map.insert("status_worker_write_us".into(), json!(0));
        map.insert("status_worker_errors".into(), json!(0));
        map.insert("status_worker_active".into(), json!(true));
        for field in ["rss_kb", "rss_hwm_kb"] {
            assert!(streamed.get(field).is_some_and(Value::is_u64));
            assert!(expected.get(field).is_some_and(Value::is_u64));
            streamed.as_object_mut().unwrap().remove(field);
            expected.as_object_mut().unwrap().remove(field);
        }
        assert_eq!(streamed, expected);
    }

    #[test]
    fn runtime_status_publisher_replaces_status_file_atomically() {
        let path = std::env::temp_dir().join(unique_name("runtime-status.json"));
        let publisher = RuntimeStatusPublisher::new_for_test(path.clone(), Duration::ZERO);
        publisher.submit(publisher_status(1, "arcade", "disabled"));
        drop(publisher);

        let text = fs::read_to_string(&path).expect("status json should be written");
        let _ = fs::remove_file(&path);
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
        assert!(!std::path::Path::new(&format!("{}.tmp", path.display())).exists());
    }

    fn wait_for_publisher(
        publisher: &RuntimeStatusPublisher,
        predicate: impl Fn(RuntimeStatusPublisherMetrics) -> bool,
    ) -> RuntimeStatusPublisherMetrics {
        let started = Instant::now();
        loop {
            let metrics = publisher.metrics();
            if predicate(metrics) || started.elapsed() > Duration::from_secs(2) {
                return metrics;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn runtime_status_publisher_coalesces_stale_snapshots_behind_slow_worker() {
        let path = std::env::temp_dir().join(unique_name("runtime-status-coalesce.json"));
        let publisher =
            RuntimeStatusPublisher::new_for_test(path.clone(), Duration::from_millis(50));
        publisher.submit(publisher_status(1, "home", "active"));
        publisher.submit(publisher_status(2, "settings", "active"));
        publisher.submit(publisher_status(3, "arcade", "complete"));
        let metrics = wait_for_publisher(&publisher, |metrics| metrics.written_sequence == 3);
        assert_eq!(metrics.submitted_sequence, 3);
        assert_eq!(metrics.written_sequence, 3);
        assert!(metrics.replaced_count >= 1);
        drop(publisher);
        let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(value["status_sequence"], 3);
        assert_eq!(value["screen"], "arcade");
        assert_eq!(value["screensaver_profile_state"], "complete");
    }

    #[test]
    fn runtime_status_publisher_reports_write_failure() {
        let path = std::env::temp_dir().join(unique_name("runtime-status-failure"));
        fs::create_dir_all(&path).unwrap();
        let publisher = RuntimeStatusPublisher::new_for_test(path.clone(), Duration::ZERO);
        publisher.submit(publisher_status(4, "home", "active"));
        let metrics = wait_for_publisher(&publisher, |metrics| metrics.worker_errors > 0);
        assert_eq!(metrics.written_sequence, 0);
        assert!(metrics.worker_errors > 0);
        drop(publisher);
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn runtime_status_publisher_drop_flushes_final_completion_and_joins() {
        let path = std::env::temp_dir().join(unique_name("runtime-status-final.json"));
        let publisher =
            RuntimeStatusPublisher::new_for_test(path.clone(), Duration::from_millis(20));
        publisher.submit(publisher_status(9, "settings", "complete"));
        drop(publisher);
        let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(value["status_sequence"], 9);
        assert_eq!(value["status_written_sequence"], 9);
        assert_eq!(value["screensaver_profile_state"], "complete");
    }

    #[test]
    fn runtime_status_submission_never_waits_for_worker_serialization() {
        let path = std::env::temp_dir().join(unique_name("runtime-status-submit.json"));
        let publisher =
            RuntimeStatusPublisher::new_for_test(path.clone(), Duration::from_millis(100));
        let started = Instant::now();
        publisher.submit(publisher_status(1, "home", "active"));
        assert!(started.elapsed() < Duration::from_millis(50));
        drop(publisher);
        let _ = fs::remove_file(path);
    }
}
