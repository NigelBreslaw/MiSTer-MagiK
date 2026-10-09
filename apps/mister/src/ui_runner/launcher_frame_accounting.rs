// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::launcher_compositor::{
    LauncherPresentBackend, LauncherPresentResult, LauncherPresentStatus,
};
use super::launcher_loop::{LaunchReturnSession, LauncherStatusTextSnapshot};
#[cfg(feature = "tooling")]
use super::launcher_pacing::FrameProductionClass;
use super::launcher_pacing::{FrameProductionTrace, LauncherPacingTrace};
use super::launcher_screensaver::ScreensaverRenderTrace;
use super::*;
use crate::launcher_presentation::SelectionFeedbackStamp;
use mister_magik_fb::latch_readiness::LatchFailure;

const FRAME_BUDGET_US: u64 = 16_667;
const FRAME_CADENCE_WARNING_US: u64 = 16_000;
const FRAME_BUDGET_20MS_US: u64 = 20_000;
const FRAME_BUDGET_33MS_US: u64 = 33_334;
const FRAME_ANALYTICS_LEASE_PATH: &str = "/tmp/mister-magik/realtime-frame-analytics";
const FRAME_ANALYTICS_LEASE_MAX_AGE: Duration = Duration::from_secs(3);
const FRAME_ANALYTICS_SAMPLE_CAP: usize = 75;
const FRAME_SLOW_SAMPLE_CAP: usize = 32;

pub(super) struct LauncherFrameAccounting {
    output_route: &'static str,
    crt_font_experiment: &'static str,
    framebuffer_width: usize,
    framebuffer_height: usize,
    fps_log_enabled: bool,
    fps_window_start: Instant,
    fps_frames: u64,
    prepare_us: u128,
    render_us: u128,
    custom_draw_us: u128,
    vsync_us: u128,
    copy_us: u128,
    cached_present_us: u128,
    hidden_compose_us: u128,
    direct_preview_present_us: u128,
    arcade_list_present_us: u128,
    rows: u128,
    #[cfg(feature = "profile")]
    boot_frame_profile: Option<boot_analytics::LauncherFrameWriter>,
    runtime_status_publisher: runtime_status::RuntimeStatusPublisher,
    last_status_write: Instant,
    status_sequence: u64,
    last_media_receipt: String,
    profile_completion_submitted: bool,
    first_copy_logged: bool,
    first_frame_logged: bool,
    first_visible_copy_done: bool,
    stable_frame_logged: bool,
    last_rendered_frame_at: Instant,
    idle_loops_since_status: u64,
    last_rolling_fps: f64,
    last_rolling_prepare_us: u64,
    last_rolling_render_us: u64,
    last_rolling_custom_draw_us: u64,
    last_rolling_vsync_us: u64,
    last_rolling_present_us: u64,
    last_rolling_rows: u64,
    last_vsync_source: &'static str,
    last_vsync_period_us: u64,
    last_present_backend: &'static str,
    last_present_status: &'static str,
    effective_view: &'static str,
    latch_failure_state: String,
    latch_failure_stage: String,
    latch_failure_reason: String,
    latch_failure_detail: String,
    display_frozen: bool,
    last_present_buffer: u8,
    last_latch_publish_us: u64,
    last_latch_sequence: u16,
    last_latch_flip_count: u16,
    last_latch_drop_count: u16,
    startup_intro: Option<runtime_status::StartupIntroCadenceStatus>,
    catalog_generation: String,
    frame_budget_total: FrameBudgetAccumulator,
    frame_budget_window: FrameBudgetAccumulator,
    last_frame_budget_status: runtime_status::FrameBudgetStatus,
    frame_analytics_mode: FrameAnalyticsMode,
    frame_analytics_samples: Vec<runtime_status::FrameBudgetRecentFrame>,
    slow_frame_samples: Vec<runtime_status::FrameBudgetSlowFrame>,
}

pub(super) struct LauncherPresentedFrame {
    pub(super) frames: u64,
    pub(super) selection_feedback: SelectionFeedbackStamp,
    pub(super) selected: usize,
    pub(super) visual_index: f32,
    pub(super) startup_start: Instant,
    pub(super) startup_monotonic_us: u64,
    pub(super) run_start: Instant,
    pub(super) loop_start: Instant,
    pub(super) frame_t0: Instant,
    pub(super) frame_t1: Instant,
    pub(super) frame_t2: Instant,
    pub(super) frame_t3: Instant,
    pub(super) frame_t4: Instant,
    pub(super) pre_render_wait_us: u128,
    pub(super) post_present_wait_us: u128,
    pub(super) custom_draw_start: Instant,
    pub(super) custom_draw_done: Instant,
    pub(super) custom_draw_trace: LauncherCustomDrawTrace,
    pub(super) prepare_trace: LauncherPrepareTrace,
    pub(super) prepare_us: u128,
    pub(super) dirty_rect: Option<DirtyRect>,
    pub(super) copied_rows: u32,
    pub(super) direct_preview_rows: u32,
    pub(super) present_bytes: usize,
    pub(super) wasted_present_bytes: usize,
    pub(super) fb_present_us_override: Option<u128>,
    pub(super) vsync_us_override: Option<u128>,
    pub(super) cached_present_us: u128,
    pub(super) hidden_compose_us: u128,
    pub(super) hidden_preview_compose_us: u128,
    pub(super) hidden_arcade_compose_us: u128,
    pub(super) direct_preview_present_us: u128,
    pub(super) arcade_list_present_us: u128,
    pub(super) main_present_backend: LauncherPresentBackend,
    pub(super) main_present_status: LauncherPresentStatus,
    pub(super) main_present_buffer: u8,
    pub(super) main_present_hidden_copy_us: u128,
    pub(super) main_present_hidden_publish_us: u128,
    #[cfg_attr(not(feature = "tooling"), allow(dead_code))]
    pub(super) main_present_hidden_copied_bytes: usize,
    pub(super) main_present_hidden_invalid_bytes: usize,
    pub(super) main_present_hidden_rect_count: u32,
    pub(super) main_present_hidden_catchup_bytes: usize,
    pub(super) main_present_hidden_full_copy: bool,
    pub(super) main_present_copy_path: &'static str,
    pub(super) main_present_request_us: u128,
    pub(super) main_present_set_vga_fb_us: u128,
    pub(super) main_present_wait_us: u64,
    pub(super) main_present_sequence: u16,
    pub(super) main_present_post_active_sequence: u16,
    pub(super) main_present_post_pending_sequence: u16,
    pub(super) main_present_post_pending: bool,
    pub(super) main_present_active_sequence: u16,
    pub(super) main_present_pending: bool,
    pub(super) main_present_completion_poll_count: u16,
    pub(super) main_present_completion_poll_wall_us: u64,
    pub(super) main_present_completion_poll_cpu_us: u64,
    pub(super) main_present_flip_count: u16,
    pub(super) main_present_drop_count: u16,
    pub(super) main_present_receipt_crc: u16,
    pub(super) vsync_source: Option<VsyncPaceSource>,
    pub(super) vsync_period_us: u64,
    pub(super) vsync_miss_streak: u32,
    pub(super) vsync_stale_hits: u32,
    pub(super) vsync_wait_start_age_us: u64,
    pub(super) vsync_accepted_hit_age_us: u64,
    pub(super) frame_start_phase_us: u64,
    pub(super) present_phase_us: u128,
    pub(super) redraw_pending: bool,
    pub(super) wake_reasons_bits: u64,
    pub(super) preview_cache_state: &'static str,
    pub(super) preview_transition: PreviewTransitionTrace,
    pub(super) composition_status: UiCompositionStatus,
    pub(super) screensaver_active: bool,
    pub(super) screensaver_active_cards: usize,
    pub(super) frame_production_trace: FrameProductionTrace,
    pub(super) screensaver_render_trace: ScreensaverRenderTrace,
    pub(super) status_write_due: bool,
    pub(super) status_string_copy_bytes: usize,
    pub(super) clock_update_due: bool,
    pub(super) clock_update_us: u128,
    pub(super) cpu_loop_start: FrameAnalyticsCpuStamp,
    pub(super) cpu_t0: FrameAnalyticsCpuStamp,
    pub(super) cpu_t1: FrameAnalyticsCpuStamp,
    pub(super) cpu_t2: FrameAnalyticsCpuStamp,
    pub(super) cpu_custom_draw_start: FrameAnalyticsCpuStamp,
    pub(super) cpu_custom_draw_done: FrameAnalyticsCpuStamp,
    pub(super) cpu_t3: FrameAnalyticsCpuStamp,
    pub(super) cpu_t4: FrameAnalyticsCpuStamp,
}

pub(super) struct LauncherFrameSnapshotBuilder {
    pub(super) identity: LauncherFrameIdentity,
    pub(super) timing: LauncherFrameTiming,
    pub(super) render: LauncherFrameRenderData,
    pub(super) pacing: LauncherPacingTrace,
    pub(super) presentation: LauncherPresentResult,
    pub(super) status: LauncherFrameStatusData,
    pub(super) cpu: LauncherFrameCpuTrace,
}

pub(super) struct LauncherFrameIdentity {
    pub(super) frames: u64,
    pub(super) selection_feedback: SelectionFeedbackStamp,
    pub(super) selected: usize,
    pub(super) visual_index: f32,
}

pub(super) struct LauncherFrameTiming {
    pub(super) startup_start: Instant,
    pub(super) startup_monotonic_us: u64,
    pub(super) run_start: Instant,
    pub(super) loop_start: Instant,
    pub(super) frame_t0: Instant,
    pub(super) frame_t1: Instant,
    pub(super) frame_t2: Instant,
    pub(super) frame_t3: Instant,
    pub(super) frame_t4: Instant,
    pub(super) pre_render_wait_us: u128,
    pub(super) post_present_wait_us: u128,
    pub(super) custom_draw_start: Instant,
    pub(super) custom_draw_done: Instant,
    pub(super) prepare_us: u128,
    pub(super) redraw_pending: bool,
    pub(super) wake_reasons_bits: u64,
}

pub(super) struct LauncherFrameRenderData {
    pub(super) custom_draw_trace: LauncherCustomDrawTrace,
    pub(super) prepare_trace: LauncherPrepareTrace,
    pub(super) dirty_rect: Option<DirtyRect>,
    pub(super) preview_cache_state: &'static str,
    pub(super) preview_transition: PreviewTransitionTrace,
    pub(super) composition_status: UiCompositionStatus,
    pub(super) screensaver_active: bool,
    pub(super) screensaver_active_cards: usize,
    pub(super) frame_production_trace: FrameProductionTrace,
    pub(super) screensaver_render_trace: ScreensaverRenderTrace,
}

pub(super) struct LauncherFrameStatusData {
    pub(super) status_write_due: bool,
    pub(super) status_string_copy_bytes: usize,
    pub(super) clock_update_due: bool,
    pub(super) clock_update_us: u128,
}

pub(super) struct LauncherFrameCpuTrace {
    pub(super) loop_start: FrameAnalyticsCpuStamp,
    pub(super) t0: FrameAnalyticsCpuStamp,
    pub(super) t1: FrameAnalyticsCpuStamp,
    pub(super) t2: FrameAnalyticsCpuStamp,
    pub(super) custom_draw_start: FrameAnalyticsCpuStamp,
    pub(super) custom_draw_done: FrameAnalyticsCpuStamp,
    pub(super) t3: FrameAnalyticsCpuStamp,
    pub(super) t4: FrameAnalyticsCpuStamp,
}

pub(super) struct LauncherFrameFinishTraceTiming {
    pub(super) runtime_status_write_us: u128,
    pub(super) frame_finish_us: u128,
}

impl LauncherFrameSnapshotBuilder {
    pub(super) fn build(self) -> LauncherPresentedFrame {
        LauncherPresentedFrame {
            frames: self.identity.frames,
            selection_feedback: self.identity.selection_feedback,
            selected: self.identity.selected,
            visual_index: self.identity.visual_index,
            startup_start: self.timing.startup_start,
            startup_monotonic_us: self.timing.startup_monotonic_us,
            run_start: self.timing.run_start,
            loop_start: self.timing.loop_start,
            frame_t0: self.timing.frame_t0,
            frame_t1: self.timing.frame_t1,
            frame_t2: self.timing.frame_t2,
            frame_t3: self.timing.frame_t3,
            frame_t4: self.timing.frame_t4,
            pre_render_wait_us: self.timing.pre_render_wait_us,
            post_present_wait_us: self.timing.post_present_wait_us,
            custom_draw_start: self.timing.custom_draw_start,
            custom_draw_done: self.timing.custom_draw_done,
            custom_draw_trace: self.render.custom_draw_trace,
            prepare_trace: self.render.prepare_trace,
            prepare_us: self.timing.prepare_us,
            dirty_rect: self.render.dirty_rect,
            copied_rows: self.presentation.copied_rows,
            direct_preview_rows: self.presentation.direct_preview_rows,
            present_bytes: self.presentation.present_bytes,
            wasted_present_bytes: self.presentation.wasted_present_bytes,
            fb_present_us_override: self.presentation.fb_present_us_override,
            vsync_us_override: self.presentation.vsync_us_override,
            cached_present_us: self.presentation.cached_present_us,
            hidden_compose_us: self.presentation.hidden_compose_us,
            hidden_preview_compose_us: self.presentation.hidden_preview_compose_us,
            hidden_arcade_compose_us: self.presentation.hidden_arcade_compose_us,
            direct_preview_present_us: self.presentation.direct_preview_present_us,
            arcade_list_present_us: self.presentation.arcade_list_present_us,
            main_present_backend: self.presentation.main_present_backend,
            main_present_status: self.presentation.main_present_status,
            main_present_buffer: self.presentation.main_present_buffer,
            main_present_hidden_copy_us: self.presentation.main_present_hidden_copy_us,
            main_present_hidden_publish_us: self.presentation.main_present_hidden_publish_us,
            main_present_hidden_copied_bytes: self.presentation.main_present_hidden_copied_bytes,
            main_present_hidden_invalid_bytes: self.presentation.main_present_hidden_invalid_bytes,
            main_present_hidden_rect_count: self.presentation.main_present_hidden_rect_count,
            main_present_hidden_catchup_bytes: self.presentation.main_present_hidden_catchup_bytes,
            main_present_hidden_full_copy: self.presentation.main_present_hidden_full_copy,
            main_present_copy_path: self.presentation.main_present_copy_path,
            main_present_request_us: self.presentation.main_present_request_us,
            main_present_set_vga_fb_us: self.presentation.main_present_set_vga_fb_us,
            main_present_wait_us: self.presentation.main_present_wait_us,
            main_present_sequence: self.presentation.main_present_sequence,
            main_present_post_active_sequence: self.presentation.main_present_post_active_sequence,
            main_present_post_pending_sequence: self
                .presentation
                .main_present_post_pending_sequence,
            main_present_post_pending: self.presentation.main_present_post_pending,
            main_present_active_sequence: self.presentation.main_present_sequence,
            main_present_pending: false,
            main_present_completion_poll_count: 0,
            main_present_completion_poll_wall_us: 0,
            main_present_completion_poll_cpu_us: 0,
            main_present_flip_count: self.presentation.main_present_flip_count,
            main_present_drop_count: self.presentation.main_present_drop_count,
            main_present_receipt_crc: self.presentation.main_present_receipt_crc,
            vsync_source: self.pacing.vsync_source,
            vsync_period_us: self.pacing.vsync_period_us,
            vsync_miss_streak: self.pacing.vsync_miss_streak,
            vsync_stale_hits: self.pacing.vsync_stale_hits,
            vsync_wait_start_age_us: self.pacing.vsync_wait_start_age_us,
            vsync_accepted_hit_age_us: self.pacing.vsync_accepted_hit_age_us,
            frame_start_phase_us: self.pacing.frame_start_phase_us,
            present_phase_us: self.pacing.present_phase_us,
            redraw_pending: self.timing.redraw_pending,
            wake_reasons_bits: self.timing.wake_reasons_bits,
            preview_cache_state: self.render.preview_cache_state,
            preview_transition: self.render.preview_transition,
            composition_status: self.render.composition_status,
            screensaver_active: self.render.screensaver_active,
            screensaver_active_cards: self.render.screensaver_active_cards,
            frame_production_trace: self.render.frame_production_trace,
            screensaver_render_trace: self.render.screensaver_render_trace,
            status_write_due: self.status.status_write_due,
            status_string_copy_bytes: self.status.status_string_copy_bytes,
            clock_update_due: self.status.clock_update_due,
            clock_update_us: self.status.clock_update_us,
            cpu_loop_start: self.cpu.loop_start,
            cpu_t0: self.cpu.t0,
            cpu_t1: self.cpu.t1,
            cpu_t2: self.cpu.t2,
            cpu_custom_draw_start: self.cpu.custom_draw_start,
            cpu_custom_draw_done: self.cpu.custom_draw_done,
            cpu_t3: self.cpu.t3,
            cpu_t4: self.cpu.t4,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum FrameAnalyticsMode {
    #[default]
    Off,
    Wall,
    Thread,
    Process,
}

impl FrameAnalyticsMode {
    fn from_lease_text(text: &str) -> Option<Self> {
        match text.trim() {
            "off" => Some(Self::Off),
            "wall" => Some(Self::Wall),
            "thread" => Some(Self::Thread),
            "process" | "1" | "true" => Some(Self::Process),
            _ => None,
        }
    }

    pub(super) fn records_wall(self) -> bool {
        !matches!(self, Self::Off)
    }

    fn records_thread_cpu(self) -> bool {
        matches!(self, Self::Thread | Self::Process)
    }

    fn records_process_cpu(self) -> bool {
        matches!(self, Self::Process)
    }
}

fn fresh_frame_analytics_mode(
    previous: FrameAnalyticsMode,
    lease_is_fresh: bool,
    lease_text: Result<&str, &std::io::Error>,
) -> FrameAnalyticsMode {
    if !lease_is_fresh {
        return FrameAnalyticsMode::Off;
    }
    lease_text
        .ok()
        .and_then(FrameAnalyticsMode::from_lease_text)
        .unwrap_or(previous)
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FrameAnalyticsCpuStamp {
    thread_us: u64,
    process_us: u64,
}

impl FrameAnalyticsCpuStamp {
    pub(super) fn capture(mode: FrameAnalyticsMode) -> Self {
        if matches!(mode, FrameAnalyticsMode::Off | FrameAnalyticsMode::Wall) {
            return Self::default();
        }
        Self {
            thread_us: mode
                .records_thread_cpu()
                .then(cpu_thread_us)
                .flatten()
                .unwrap_or(0),
            process_us: mode
                .records_process_cpu()
                .then(cpu_process_us)
                .flatten()
                .unwrap_or(0),
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct LauncherPrepareTrace {
    pub(super) slint_timer_dispatch_us: u128,
    pub(super) navigation_commit_us: u128,
    pub(super) bridge_sync_us: u128,
    pub(super) bridge_model_projection_us: u128,
    pub(super) bridge_model_replacements: u64,
    pub(super) bridge_row_mutations: u64,
    pub(super) bridge_row_allocations: u64,
    pub(super) bridge_shared_string_constructions: u64,
    pub(super) bridge_model_allocation_us: u64,
    pub(super) catalog_worker_us: u128,
    pub(super) catalog_message_count: u32,
    pub(super) catalog_backlog: u32,
    pub(super) catalog_ready_deferred: bool,
    pub(super) catalog_ready_deferred_age_us: u128,
    pub(super) media_worker_us: u128,
    pub(super) media_gate_us: u128,
    pub(super) preview_schedule_us: u128,
    pub(super) preview_apply_us: u128,
    pub(super) preview_worker_drained: u32,
    pub(super) preview_ready_processed: u32,
    pub(super) preview_selected_processed: u32,
    pub(super) preview_prefetch_processed: u32,
    pub(super) preview_stale_results: u32,
    pub(super) preview_cache_inserts: u32,
    pub(super) preview_cache_evictions: u32,
    pub(super) preview_failed_results: u32,
    pub(super) preview_backlog: u32,
    pub(super) status_string_copy_us: u128,
}

impl LauncherFrameAccounting {
    pub(super) fn finish_preview_scroll_trace(&mut self) {}
}

#[derive(Clone, Copy, Default)]
pub(super) struct LauncherCustomDrawTrace {
    pub(super) arcade_bbox_invalidation: bool,
    pub(super) arcade_rect_invalidation: bool,
    pub(super) arcade_false_positive_invalidation: bool,
    pub(super) preview_bbox_invalidation: bool,
    pub(super) preview_rect_invalidation: bool,
    pub(super) preview_false_positive_invalidation: bool,
    pub(super) arcade_list_update_us: u128,
    pub(super) persistent_arcade_composition:
        crate::arcade_list_renderer::PersistentArcadeCompositionTrace,
    pub(super) portrait_arcade_list_pixels: u64,
    pub(super) portrait_arcade_list_bytes: u64,
    pub(super) portrait_preview_rotation_pixels: u64,
    pub(super) portrait_preview_blend_pixels: u64,
    pub(super) portrait_preview_worker_queue_replacements: u64,
    pub(super) portrait_preview_worker_result_replacements: u64,
    pub(super) portrait_preview_worker_stale_results: u64,
    pub(super) portrait_preview_worker_age_us: u64,
    pub(super) portrait_preview_worker_generation_lag: u64,
    pub(super) portrait_preview_worker_affinity_status: &'static str,
    pub(super) portrait_preview_worker_errors: u64,
    pub(super) portrait_preview_worker_adoption_failures: u64,
    pub(super) portrait_preview_worker_alive: bool,
    pub(super) crt_backdrop_prepare_us: u64,
    pub(super) crt_backdrop_prepare_pixels: u32,
    pub(super) crt_backdrop_blend_us: u64,
    pub(super) crt_backdrop_blend_pixels: u32,
    pub(super) crt_backdrop_copy_us: u64,
    pub(super) crt_backdrop_copy_pixels: u32,
    pub(super) crt_backdrop_list_overlay_us: u64,
    pub(super) crt_backdrop_list_overlay_pixels: u32,
    pub(super) crt_backdrop_list_restore_pixels: u32,
    pub(super) crt_backdrop_list_foreground_pixels: u32,
    pub(super) crt_backdrop_alpha_bucket: u8,
    pub(super) crt_backdrop_active: bool,
    pub(super) crt_backdrop_selected: usize,
    pub(super) crt_backdrop_transition_id: u64,
    pub(super) crt_backdrop_cache_state: &'static str,
    pub(super) effect_label_us: u128,
    pub(super) navigation_transition_base_copy_us: u128,
    pub(super) navigation_transition_settings_blit_us: u128,
    pub(super) navigation_transition_card_scale_us: u128,
    pub(super) navigation_transition_destination_reveal_us: u128,
    pub(super) navigation_transition_overlay_us: u128,
    pub(super) navigation_transition_edge: &'static str,
    pub(super) navigation_transition_route: &'static str,
    pub(super) navigation_transition_direction: &'static str,
    pub(super) navigation_transition_renderer: &'static str,
    pub(super) navigation_transition_orientation: &'static str,
    pub(super) navigation_snapshot_locked: bool,
    pub(super) navigation_slint_render_called: bool,
    pub(super) navigation_status_quiesce_wait_us: u64,
    pub(super) navigation_status_quiesce_timeout: bool,
    pub(super) orientation_transition_active: bool,
    pub(super) orientation_transition_effect: &'static str,
    pub(super) orientation_transition_from: &'static str,
    pub(super) orientation_transition_to: &'static str,
    pub(super) orientation_begin_us: u128,
    pub(super) orientation_source_snapshot_us: u128,
    pub(super) orientation_layout_us: u128,
    pub(super) orientation_source_snapshot_bytes: u64,
    pub(super) orientation_controlled_slint_raster_us: u128,
    pub(super) orientation_transition_destination_capture_us: u128,
    pub(super) orientation_destination_snapshot_bytes: u64,
    pub(super) orientation_damage_rotation_us: u128,
    pub(super) orientation_damage_build_us: u128,
    pub(super) orientation_damage_rects_before: u32,
    pub(super) orientation_damage_rects_after: u32,
    pub(super) orientation_effect_read_bytes: u64,
    pub(super) orientation_effect_write_bytes: u64,
    pub(super) orientation_transition_cache_restore_us: u128,
    pub(super) orientation_transition_total_us: u128,
    pub(super) orientation_transition_stats: OrientationTransitionRenderStats,
}

#[derive(Clone, Copy, Default)]
struct FrameBudgetAccumulator {
    frames: u64,
    over_budget: u64,
    over_20ms: u64,
    over_33ms: u64,
    max_wall_us: u64,
    latest_over_budget_frame: u64,
    latest_over_budget_wall_us: u64,
    max_vsync_miss_streak: u64,
    vsync: u64,
    fallback: u64,
    timeout: u64,
    error: u64,
    prepare_us: u128,
    render_us: u128,
    custom_draw_us: u128,
    vsync_us: u128,
    present_us: u128,
}

impl FrameBudgetAccumulator {
    fn record(&mut self, sample: FrameBudgetSample) {
        self.frames = self.frames.saturating_add(1);
        self.max_wall_us = self.max_wall_us.max(sample.wall_us);
        self.max_vsync_miss_streak = self
            .max_vsync_miss_streak
            .max(u64::from(sample.vsync_miss_streak));
        if sample.wall_us > FRAME_BUDGET_US {
            self.over_budget = self.over_budget.saturating_add(1);
            self.latest_over_budget_frame = sample.frame;
            self.latest_over_budget_wall_us = sample.wall_us;
        }
        if sample.wall_us > FRAME_BUDGET_20MS_US {
            self.over_20ms = self.over_20ms.saturating_add(1);
        }
        if sample.wall_us > FRAME_BUDGET_33MS_US {
            self.over_33ms = self.over_33ms.saturating_add(1);
        }
        match sample.vsync_source {
            Some(VsyncPaceSource::Vsync) => self.vsync = self.vsync.saturating_add(1),
            Some(VsyncPaceSource::Fallback) => self.fallback = self.fallback.saturating_add(1),
            Some(VsyncPaceSource::Timeout) => self.timeout = self.timeout.saturating_add(1),
            Some(VsyncPaceSource::Error) => self.error = self.error.saturating_add(1),
            None => {}
        }
        self.prepare_us = self.prepare_us.saturating_add(sample.prepare_us);
        self.render_us = self.render_us.saturating_add(sample.render_us);
        self.custom_draw_us = self.custom_draw_us.saturating_add(sample.custom_draw_us);
        self.vsync_us = self.vsync_us.saturating_add(sample.vsync_us);
        self.present_us = self.present_us.saturating_add(sample.present_us);
    }

    fn avg_us(sum: u128, frames: u64) -> u64 {
        if frames == 0 {
            0
        } else {
            (sum / u128::from(frames)) as u64
        }
    }
}

#[derive(Clone, Copy)]
struct FrameBudgetSample {
    frame: u64,
    wall_us: u64,
    prepare_us: u128,
    render_us: u128,
    custom_draw_us: u128,
    vsync_us: u128,
    present_us: u128,
    vsync_source: Option<VsyncPaceSource>,
    vsync_miss_streak: u32,
}

#[derive(Clone, Copy)]
pub(super) struct FrameStatusView<'a> {
    pub(super) nav: &'a LauncherNav,
    pub(super) pad: &'a PadPool,
    pub(super) catalog: &'a ArcadeCatalog,
    pub(super) catalog_ready: bool,
    pub(super) catalog_refresh_done: bool,
    pub(super) launching: bool,
    pub(super) loading_title: &'a str,
    pub(super) catalog_scan_visible: bool,
    pub(super) catalog_scan_percent: i32,
    pub(super) catalog_background_scan_visible: bool,
    pub(super) confirm_visible: bool,
    pub(super) confirm_selected: i32,
    pub(super) status_text: Option<&'a LauncherStatusTextSnapshot>,
    pub(super) route_reassert_count: u64,
    pub(super) last_route_reassert_frame: u64,
    pub(super) last_route_reassert_ok: bool,
    pub(super) last_route_reassert_error: &'a str,
    pub(super) startup_status: StartupRevealStatus,
    pub(super) return_session: &'a LaunchReturnSession,
}

impl<'a> FrameStatusView<'a> {
    /// The snapshot's status strings, or empty ones when no snapshot was taken this frame.
    pub(super) fn strings(&self) -> FrameStatusStrings<'a> {
        let text =
            |field: fn(&LauncherStatusTextSnapshot) -> &str| self.status_text.map_or("", field);
        FrameStatusStrings {
            catalog_scan_title: text(|t| t.catalog_scan_title.as_str()),
            catalog_scan_detail: text(|t| t.catalog_scan_detail.as_str()),
            catalog_scan_message: text(|t| t.catalog_scan_message.as_str()),
            confirm_title: text(|t| t.confirm_title.as_str()),
            confirm_message: text(|t| t.confirm_message.as_str()),
            confirm_left_label: text(|t| t.confirm_left_label.as_str()),
            confirm_right_label: text(|t| t.confirm_right_label.as_str()),
        }
    }
}

/// The status strings of a `FrameStatusView`.
pub(super) struct FrameStatusStrings<'a> {
    pub(super) catalog_scan_title: &'a str,
    pub(super) catalog_scan_detail: &'a str,
    pub(super) catalog_scan_message: &'a str,
    pub(super) confirm_title: &'a str,
    pub(super) confirm_message: &'a str,
    pub(super) confirm_left_label: &'a str,
    pub(super) confirm_right_label: &'a str,
}

impl LauncherFrameAccounting {
    pub(super) fn new(
        run_start: Instant,
        output_route: &'static str,
        crt_font_experiment: &'static str,
        framebuffer_width: usize,
        framebuffer_height: usize,
        profile_fps_log_enabled: bool,
    ) -> Self {
        Self {
            output_route,
            crt_font_experiment,
            framebuffer_width,
            framebuffer_height,
            fps_log_enabled: profile_fps_log_enabled,
            fps_window_start: run_start,
            fps_frames: 0,
            prepare_us: 0,
            render_us: 0,
            custom_draw_us: 0,
            vsync_us: 0,
            copy_us: 0,
            cached_present_us: 0,
            hidden_compose_us: 0,
            direct_preview_present_us: 0,
            arcade_list_present_us: 0,
            rows: 0,
            #[cfg(feature = "profile")]
            boot_frame_profile: boot_analytics::LauncherFrameWriter::from_env(),
            runtime_status_publisher: runtime_status::RuntimeStatusPublisher::new(),
            last_status_write: Instant::now() - Duration::from_secs(2),
            status_sequence: 0,
            last_media_receipt: String::new(),
            profile_completion_submitted: false,
            first_copy_logged: false,
            first_frame_logged: false,
            first_visible_copy_done: false,
            stable_frame_logged: false,
            last_rendered_frame_at: run_start,
            idle_loops_since_status: 0,
            last_rolling_fps: 0.0,
            last_rolling_prepare_us: 0,
            last_rolling_render_us: 0,
            last_rolling_custom_draw_us: 0,
            last_rolling_vsync_us: 0,
            last_rolling_present_us: 0,
            last_rolling_rows: 0,
            last_vsync_source: "none",
            last_vsync_period_us: 0,
            last_present_backend: "none",
            last_present_status: "none",
            effective_view: "home",
            latch_failure_state: String::new(),
            latch_failure_stage: String::new(),
            latch_failure_reason: String::new(),
            latch_failure_detail: String::new(),
            display_frozen: false,
            last_present_buffer: 0,
            last_latch_publish_us: 0,
            last_latch_sequence: 0,
            last_latch_flip_count: 0,
            last_latch_drop_count: 0,
            startup_intro: None,
            catalog_generation: String::new(),
            frame_budget_total: FrameBudgetAccumulator::default(),
            frame_budget_window: FrameBudgetAccumulator::default(),
            last_frame_budget_status: runtime_status::FrameBudgetStatus {
                budget_us: FRAME_BUDGET_US,
                ..runtime_status::FrameBudgetStatus::default()
            },
            frame_analytics_mode: FrameAnalyticsMode::Off,
            frame_analytics_samples: Vec::with_capacity(FRAME_ANALYTICS_SAMPLE_CAP),
            slow_frame_samples: Vec::with_capacity(FRAME_SLOW_SAMPLE_CAP),
        }
    }

    pub(super) fn first_visible_copy_done(&self) -> bool {
        self.first_visible_copy_done
    }

    pub(super) fn record_startup_intro_cadence(
        &mut self,
        cadence: runtime_status::StartupIntroCadenceStatus,
    ) {
        self.startup_intro = Some(cadence);
    }

    pub(super) fn record_latch_failure(&mut self, failure: &LatchFailure) {
        if !self.latch_failure_state.is_empty() {
            return;
        }
        self.latch_failure_state = failure.state.code().to_string();
        self.latch_failure_stage = failure.stage.code().to_string();
        self.latch_failure_reason = failure.reason_code().to_string();
        self.latch_failure_detail.clone_from(&failure.detail);
    }

    pub(super) fn set_display_frozen(&mut self, frozen: bool) {
        self.display_frozen = frozen;
    }

    pub(super) fn set_effective_view(&mut self, effective_view: &'static str) {
        self.effective_view = effective_view;
    }

    pub(super) fn set_catalog_generation(&mut self, generation: Option<&str>) {
        self.catalog_generation.clear();
        if let Some(generation) = generation {
            self.catalog_generation.push_str(generation);
        }
    }

    pub(super) fn preview_scroll_trace_enabled(&self) -> bool {
        false
    }

    pub(super) fn status_write_due(&self) -> bool {
        self.last_status_write.elapsed() >= Duration::from_secs(1)
            || (!self.profile_completion_submitted
                && cpu_profile::screensaver_profile_state() == "complete")
    }

    pub(super) fn runtime_status_worker_active(&self) -> bool {
        self.runtime_status_publisher.metrics().worker_active
    }

    pub(super) fn frame_analytics_mode(&self) -> FrameAnalyticsMode {
        self.frame_analytics_mode
    }

    pub(super) fn finish_frame(
        &mut self,
        frame: LauncherPresentedFrame,
        start: Instant,
        disp: &mut MappedRgb565Framebuffer,
        status: FrameStatusView<'_>,
        defer_preview_trace_flush: bool,
    ) {
        let timing = self.finish_frame_before_trace(&frame, status);
        self.record_finished_frame(
            &frame,
            start,
            disp,
            status.catalog_ready,
            timing.runtime_status_write_us,
        );
        self.write_finished_frame_trace(&frame, timing, defer_preview_trace_flush);
    }

    pub(super) fn finish_frame_before_trace(
        &mut self,
        frame: &LauncherPresentedFrame,
        status: FrameStatusView<'_>,
    ) -> LauncherFrameFinishTraceTiming {
        let frame_finish_start = Instant::now();
        let runtime_status_write_deferred = should_defer_runtime_status_write(frame);
        let status_write_now = frame.status_write_due && !runtime_status_write_deferred;
        if status_write_now {
            self.refresh_frame_analytics_mode();
        }
        let runtime_status_write_start = status_write_now.then(Instant::now);
        self.write_runtime_status(
            status_write_now,
            frame.frames,
            frame.run_start,
            status,
            frame.selected,
            frame.visual_index,
            frame.preview_cache_state,
            frame.preview_transition.effect.label(),
            frame.preview_transition.progress,
            frame.screensaver_active_cards,
            &frame.composition_status,
            None,
        );
        let runtime_status_write_us = runtime_status_write_start
            .map(|start| start.elapsed().as_micros())
            .unwrap_or(0);
        let frame_finish_us = frame_finish_start.elapsed().as_micros();
        LauncherFrameFinishTraceTiming {
            runtime_status_write_us,
            frame_finish_us,
        }
    }

    pub(super) fn record_finished_frame(
        &mut self,
        frame: &LauncherPresentedFrame,
        start: Instant,
        disp: &mut MappedRgb565Framebuffer,
        catalog_ready: bool,
        runtime_status_write_us: u128,
    ) {
        self.record_first_copy(frame, disp);
        self.accumulate_fps(frame);
        self.accumulate_frame_budget(frame, runtime_status_write_us);
        self.last_vsync_source = vsync_source_label(frame.vsync_source);
        self.last_vsync_period_us = frame.vsync_period_us;
        self.last_present_backend = frame.main_present_backend.trace_label();
        self.last_present_status = frame.main_present_status.trace_label();
        self.last_present_buffer = frame.main_present_buffer;
        self.last_latch_publish_us = u128_to_u64_saturating(frame.main_present_hidden_publish_us);
        self.last_latch_sequence = frame.main_present_sequence;
        self.last_latch_flip_count = frame.main_present_flip_count;
        self.last_latch_drop_count = frame.main_present_drop_count;
        self.record_stable_samples(frame.frames, disp);
        self.last_rendered_frame_at = frame.frame_t4;
        self.idle_loops_since_status = 0;
        #[cfg(feature = "profile")]
        self.record_boot_frame_profile(frame, disp);
        self.record_first_frame(frame, start, catalog_ready);
    }

    pub(super) fn write_finished_frame_trace(
        &mut self,
        frame: &LauncherPresentedFrame,
        timing: LauncherFrameFinishTraceTiming,
        defer_preview_trace_flush: bool,
    ) {
        {
            let _ = (frame, timing, defer_preview_trace_flush);
        }
    }

    fn refresh_frame_analytics_mode(&mut self) {
        let mode = fresh_frame_analytics_mode(
            self.frame_analytics_mode,
            std::fs::metadata(FRAME_ANALYTICS_LEASE_PATH)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age <= FRAME_ANALYTICS_LEASE_MAX_AGE),
            std::fs::read_to_string(FRAME_ANALYTICS_LEASE_PATH).as_deref(),
        );
        if mode != self.frame_analytics_mode {
            self.frame_analytics_mode = mode;
            self.frame_analytics_samples.clear();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_idle_loop(
        &mut self,
        frames: u64,
        run_start: Instant,
        now: Instant,
        status: FrameStatusView<'_>,
        arcade_selected: usize,
        arcade_visual_index: f32,
        preview_cache_state: &str,
        preview_transition_effect: &str,
        preview_transition_progress: f32,
        composition_status: &UiCompositionStatus,
    ) {
        self.idle_loops_since_status = self.idle_loops_since_status.saturating_add(1);
        let status_write_due = self.status_write_due();
        if status_write_due {
            self.refresh_frame_analytics_mode();
        }
        let last_frame_ms_ago = now
            .saturating_duration_since(self.last_rendered_frame_at)
            .as_millis()
            .min(u64::MAX as u128) as u64;
        self.write_runtime_status(
            status_write_due,
            frames,
            run_start,
            status,
            arcade_selected,
            arcade_visual_index,
            preview_cache_state,
            preview_transition_effect,
            preview_transition_progress,
            0,
            composition_status,
            Some((self.idle_loops_since_status, last_frame_ms_ago)),
        );
    }

    fn record_first_copy(
        &mut self,
        frame: &LauncherPresentedFrame,
        disp: &mut MappedRgb565Framebuffer,
    ) {
        if frame.copied_rows > 0 && !self.first_copy_logged {
            self.first_copy_logged = true;
            boot_analytics::event(
                if self.first_visible_copy_done {
                    "first_copy"
                } else {
                    "first_copy_immediate"
                },
                format!(
                    "frame={} rows={} dirty_rect={}",
                    frame.frames,
                    frame.copied_rows,
                    format_dirty_rect(frame.dirty_rect)
                ),
            );
            disp.record_visual_sample("after_first_copy");
        }
        if frame.copied_rows > 0 {
            self.first_visible_copy_done = true;
        }
    }

    fn accumulate_fps(&mut self, frame: &LauncherPresentedFrame) {
        self.fps_frames += 1;
        self.prepare_us += frame.prepare_us;
        self.render_us += (frame.frame_t2 - frame.frame_t1).as_micros();
        self.custom_draw_us += (frame.custom_draw_done - frame.custom_draw_start).as_micros();
        self.vsync_us += (frame.frame_t3 - frame.custom_draw_done).as_micros();
        self.copy_us += (frame.frame_t4 - frame.frame_t3).as_micros();
        self.cached_present_us += frame.cached_present_us;
        self.hidden_compose_us += frame.hidden_compose_us;
        self.direct_preview_present_us += frame.direct_preview_present_us;
        self.arcade_list_present_us += frame.arcade_list_present_us;
        self.rows += frame.copied_rows as u128;
        if self.fps_window_start.elapsed() >= Duration::from_secs(1) {
            let n = self.fps_frames.max(1) as u128;
            let elapsed = self.fps_window_start.elapsed().as_secs_f64();
            self.last_rolling_fps = if elapsed > 0.0 {
                self.fps_frames as f64 / elapsed
            } else {
                0.0
            };
            self.last_rolling_prepare_us = (self.prepare_us / n) as u64;
            self.last_rolling_render_us = (self.render_us / n) as u64;
            self.last_rolling_custom_draw_us = (self.custom_draw_us / n) as u64;
            self.last_rolling_vsync_us = (self.vsync_us / n) as u64;
            self.last_rolling_present_us = (self.copy_us / n) as u64;
            self.last_rolling_rows = (self.rows / n) as u64;
            if self.fps_log_enabled {
                crate::ui_logln!(
                    "launcher fps ~ {} prepare {}us slint-render {}us custom-draw {}us vsync-wait {}us fb-present {}us cached-present {}us hidden-compose {}us direct-preview-present {}us arcade-list-present {}us ({} rows avg)",
                    self.fps_frames,
                    self.prepare_us / n,
                    self.render_us / n,
                    self.custom_draw_us / n,
                    self.vsync_us / n,
                    self.copy_us / n,
                    self.cached_present_us / n,
                    self.hidden_compose_us / n,
                    self.direct_preview_present_us / n,
                    self.arcade_list_present_us / n,
                    self.rows / n
                );
            }
            self.fps_window_start = Instant::now();
            self.fps_frames = 0;
            self.prepare_us = 0;
            self.render_us = 0;
            self.custom_draw_us = 0;
            self.vsync_us = 0;
            self.copy_us = 0;
            self.cached_present_us = 0;
            self.hidden_compose_us = 0;
            self.direct_preview_present_us = 0;
            self.arcade_list_present_us = 0;
            self.rows = 0;
        }
    }

    fn accumulate_frame_budget(
        &mut self,
        frame: &LauncherPresentedFrame,
        runtime_status_write_us: u128,
    ) {
        let wall_us = u128_to_u64_saturating((frame.frame_t4 - frame.loop_start).as_micros());
        let prepare_us = u128_to_u64_saturating(frame.prepare_us);
        let render_us = u128_to_u64_saturating((frame.frame_t2 - frame.frame_t1).as_micros());
        let custom_draw_us =
            u128_to_u64_saturating((frame.custom_draw_done - frame.custom_draw_start).as_micros());
        let vsync_us = u128_to_u64_saturating(
            frame
                .vsync_us_override
                .unwrap_or_else(|| (frame.frame_t3 - frame.custom_draw_done).as_micros()),
        );
        let present_us = u128_to_u64_saturating(
            frame
                .fb_present_us_override
                .unwrap_or_else(|| (frame.frame_t4 - frame.frame_t3).as_micros()),
        );
        let sample = FrameBudgetSample {
            frame: frame.frames,
            wall_us,
            prepare_us: u128::from(prepare_us),
            render_us: u128::from(render_us),
            custom_draw_us: u128::from(custom_draw_us),
            vsync_us: u128::from(vsync_us),
            present_us: u128::from(present_us),
            vsync_source: frame.vsync_source,
            vsync_miss_streak: frame.vsync_miss_streak,
        };
        self.frame_budget_total.record(sample);
        self.frame_budget_window.record(sample);
        if wall_us >= FRAME_CADENCE_WARNING_US {
            self.push_slow_frame_sample(
                frame,
                wall_us,
                prepare_us,
                render_us,
                custom_draw_us,
                vsync_us,
                present_us,
            );
        }
        if self.frame_analytics_mode.records_wall() {
            self.push_frame_analytics_sample(
                frame,
                wall_us,
                prepare_us,
                render_us,
                custom_draw_us,
                vsync_us,
                present_us,
                runtime_status_write_us,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_frame_analytics_sample(
        &mut self,
        frame: &LauncherPresentedFrame,
        wall_us: u64,
        prepare_us: u64,
        render_us: u64,
        custom_draw_us: u64,
        vsync_us: u64,
        present_us: u64,
        runtime_status_write_us: u128,
    ) {
        let publisher = self.runtime_status_publisher.metrics();
        let attributed_prepare_us = u128_to_u64_saturating(
            frame
                .prepare_trace
                .slint_timer_dispatch_us
                .saturating_add(frame.prepare_trace.navigation_commit_us)
                .saturating_add(frame.prepare_trace.bridge_sync_us)
                .saturating_add(frame.prepare_trace.catalog_worker_us)
                .saturating_add(frame.prepare_trace.media_worker_us)
                .saturating_add(frame.prepare_trace.media_gate_us)
                .saturating_add(frame.prepare_trace.preview_schedule_us)
                .saturating_add(frame.prepare_trace.preview_apply_us)
                .saturating_add(frame.prepare_trace.status_string_copy_us),
        );
        if self.frame_analytics_samples.len() == FRAME_ANALYTICS_SAMPLE_CAP {
            self.frame_analytics_samples.remove(0);
        }
        self.frame_analytics_samples
            .push(runtime_status::FrameBudgetRecentFrame {
                frame: frame.frames,
                screensaver_active: frame.screensaver_active,
                screensaver_active_cards: frame.screensaver_active_cards,
                screensaver_renderer: frame.screensaver_render_trace.renderer,
                navigation_transition_edge: frame.custom_draw_trace.navigation_transition_edge,
                navigation_transition_route: frame.custom_draw_trace.navigation_transition_route,
                navigation_transition_direction: frame
                    .custom_draw_trace
                    .navigation_transition_direction,
                navigation_transition_renderer: frame
                    .custom_draw_trace
                    .navigation_transition_renderer,
                navigation_transition_orientation: frame
                    .custom_draw_trace
                    .navigation_transition_orientation,
                navigation_transition_us: u128_to_u64_saturating(
                    frame.custom_draw_trace.effect_label_us,
                ),
                navigation_transition_base_copy_us: u128_to_u64_saturating(
                    frame.custom_draw_trace.navigation_transition_base_copy_us,
                ),
                navigation_transition_settings_blit_us: u128_to_u64_saturating(
                    frame
                        .custom_draw_trace
                        .navigation_transition_settings_blit_us,
                ),
                navigation_transition_card_scale_us: u128_to_u64_saturating(
                    frame.custom_draw_trace.navigation_transition_card_scale_us,
                ),
                navigation_transition_destination_reveal_us: u128_to_u64_saturating(
                    frame
                        .custom_draw_trace
                        .navigation_transition_destination_reveal_us,
                ),
                navigation_transition_overlay_us: u128_to_u64_saturating(
                    frame.custom_draw_trace.navigation_transition_overlay_us,
                ),
                navigation_snapshot_locked: frame.custom_draw_trace.navigation_snapshot_locked,
                navigation_slint_render_called: frame
                    .custom_draw_trace
                    .navigation_slint_render_called,
                navigation_status_quiesce_wait_us: frame
                    .custom_draw_trace
                    .navigation_status_quiesce_wait_us,
                navigation_status_quiesce_timeout: frame
                    .custom_draw_trace
                    .navigation_status_quiesce_timeout,
                orientation_transition_active: frame
                    .custom_draw_trace
                    .orientation_transition_active,
                orientation_transition_effect: frame
                    .custom_draw_trace
                    .orientation_transition_effect,
                orientation_transition_from: frame.custom_draw_trace.orientation_transition_from,
                orientation_transition_to: frame.custom_draw_trace.orientation_transition_to,
                orientation_begin_us: u128_to_u64_saturating(
                    frame.custom_draw_trace.orientation_begin_us,
                ),
                orientation_source_snapshot_us: u128_to_u64_saturating(
                    frame.custom_draw_trace.orientation_source_snapshot_us,
                ),
                orientation_layout_us: u128_to_u64_saturating(
                    frame.custom_draw_trace.orientation_layout_us,
                ),
                orientation_controlled_slint_raster_us: u128_to_u64_saturating(
                    frame
                        .custom_draw_trace
                        .orientation_controlled_slint_raster_us,
                ),
                orientation_transition_destination_capture_us: u128_to_u64_saturating(
                    frame
                        .custom_draw_trace
                        .orientation_transition_destination_capture_us,
                ),
                orientation_damage_rotation_us: u128_to_u64_saturating(
                    frame.custom_draw_trace.orientation_damage_rotation_us,
                ),
                orientation_damage_build_us: u128_to_u64_saturating(
                    frame.custom_draw_trace.orientation_damage_build_us,
                ),
                orientation_source_snapshot_bytes: frame
                    .custom_draw_trace
                    .orientation_source_snapshot_bytes,
                orientation_destination_snapshot_bytes: frame
                    .custom_draw_trace
                    .orientation_destination_snapshot_bytes,
                orientation_effect_read_bytes: frame
                    .custom_draw_trace
                    .orientation_effect_read_bytes,
                orientation_effect_write_bytes: frame
                    .custom_draw_trace
                    .orientation_effect_write_bytes,
                orientation_damage_rects_before: frame
                    .custom_draw_trace
                    .orientation_damage_rects_before,
                orientation_damage_rects_after: frame
                    .custom_draw_trace
                    .orientation_damage_rects_after,
                orientation_transition_fill_us: frame
                    .custom_draw_trace
                    .orientation_transition_stats
                    .fill_us,
                orientation_transition_map_us: frame
                    .custom_draw_trace
                    .orientation_transition_stats
                    .map_us,
                orientation_transition_crossfade_us: frame
                    .custom_draw_trace
                    .orientation_transition_stats
                    .crossfade_us,
                orientation_transition_cache_restore_us: u128_to_u64_saturating(
                    frame
                        .custom_draw_trace
                        .orientation_transition_cache_restore_us,
                ),
                orientation_transition_total_us: u128_to_u64_saturating(
                    frame.custom_draw_trace.orientation_transition_total_us,
                ),
                orientation_transition_mapped_pixels: frame
                    .custom_draw_trace
                    .orientation_transition_stats
                    .mapped_pixels,
                orientation_transition_blended_pixels: frame
                    .custom_draw_trace
                    .orientation_transition_stats
                    .blended_pixels,
                orientation_transition_progress_ppm: frame
                    .custom_draw_trace
                    .orientation_transition_stats
                    .progress_ppm,
                crt_backdrop_prepare_us: frame.custom_draw_trace.crt_backdrop_prepare_us,
                crt_backdrop_prepare_pixels: frame.custom_draw_trace.crt_backdrop_prepare_pixels,
                crt_backdrop_blend_us: frame.custom_draw_trace.crt_backdrop_blend_us,
                crt_backdrop_blend_pixels: frame.custom_draw_trace.crt_backdrop_blend_pixels,
                wall_us,
                prepare_us,
                slint_timer_dispatch_us: u128_to_u64_saturating(
                    frame.prepare_trace.slint_timer_dispatch_us,
                ),
                navigation_commit_us: u128_to_u64_saturating(
                    frame.prepare_trace.navigation_commit_us,
                ),
                bridge_sync_us: u128_to_u64_saturating(frame.prepare_trace.bridge_sync_us),
                unattributed_prepare_us: prepare_us.saturating_sub(attributed_prepare_us),
                render_us,
                custom_draw_us,
                vsync_us,
                present_us,
                cpu_prepare_us: cpu_delta(frame.cpu_loop_start, frame.cpu_t0),
                cpu_render_us: cpu_delta(frame.cpu_t1, frame.cpu_t2),
                cpu_custom_draw_us: cpu_delta(
                    frame.cpu_custom_draw_start,
                    frame.cpu_custom_draw_done,
                ),
                cpu_vsync_us: cpu_delta(frame.cpu_custom_draw_done, frame.cpu_t3),
                cpu_frame_tail_us: cpu_delta(frame.cpu_t3, frame.cpu_t4),
                process_cpu_us: frame
                    .cpu_t4
                    .process_us
                    .saturating_sub(frame.cpu_loop_start.process_us),
                completion_monotonic_us: frame.startup_monotonic_us.saturating_add(
                    u128_to_u64_saturating(
                        frame
                            .frame_t4
                            .saturating_duration_since(frame.startup_start)
                            .as_micros(),
                    ),
                ),
                vsync_source: vsync_source_label(frame.vsync_source),
                vsync_period_us: frame.vsync_period_us,
                vsync_miss_streak: frame.vsync_miss_streak,
                vsync_stale_hits: frame.vsync_stale_hits,
                vsync_wait_start_age_us: frame.vsync_wait_start_age_us,
                vsync_accepted_hit_age_us: frame.vsync_accepted_hit_age_us,
                frame_start_phase_us: frame.frame_start_phase_us,
                present_phase_us: u128_to_u64_saturating(frame.present_phase_us),
                main_present_status: frame.main_present_status.trace_label(),
                main_present_copy_path: frame.main_present_copy_path,
                main_present_request_us: u128_to_u64_saturating(frame.main_present_request_us),
                main_present_sequence: frame.main_present_sequence,
                main_present_post_active_sequence: frame.main_present_post_active_sequence,
                main_present_post_pending_sequence: frame.main_present_post_pending_sequence,
                main_present_post_pending: frame.main_present_post_pending,
                main_present_active_sequence: frame.main_present_active_sequence,
                main_present_pending: frame.main_present_pending,
                main_present_completion_poll_count: frame.main_present_completion_poll_count,
                main_present_completion_poll_wall_us: frame.main_present_completion_poll_wall_us,
                main_present_completion_poll_cpu_us: frame.main_present_completion_poll_cpu_us,
                main_present_hidden_copy_us: u128_to_u64_saturating(
                    frame.main_present_hidden_copy_us,
                ),
                main_present_flip_count: frame.main_present_flip_count,
                main_present_drop_count: frame.main_present_drop_count,
                status_write_due: frame.status_write_due,
                runtime_status_write_us: u128_to_u64_saturating(runtime_status_write_us),
                status_publish_mode: "async",
                status_enqueue_us: u128_to_u64_saturating(runtime_status_write_us),
                status_worker_write_us: publisher.last_worker_duration_us,
                status_replaced_count: publisher.replaced_count,
                status_submitted_sequence: publisher.submitted_sequence,
                status_written_sequence: publisher.written_sequence,
                status_worker_errors: publisher.worker_errors,
                status_worker_active: publisher.worker_active,
                clock_update_due: frame.clock_update_due,
                clock_update_us: u128_to_u64_saturating(frame.clock_update_us),
                screensaver_archive_poll_us: u128_to_u64_saturating(
                    frame.screensaver_render_trace.archive_poll_us,
                ),
                screensaver_card_adopt_us: u128_to_u64_saturating(
                    frame.screensaver_render_trace.card_adopt_us,
                ),
                screensaver_parade_advance_us: u128_to_u64_saturating(
                    frame.screensaver_render_trace.parade_advance_us,
                ),
                screensaver_background_us: u128_to_u64_saturating(
                    frame.screensaver_render_trace.background_us,
                ),
                screensaver_draw_order_us: u128_to_u64_saturating(
                    frame.screensaver_render_trace.draw_order_us,
                ),
                screensaver_tile_blit_us: u128_to_u64_saturating(
                    frame.screensaver_render_trace.tile_blit_us,
                ),
                screensaver_raster_held_cards: usize_to_u64_saturating(
                    frame.screensaver_render_trace.raster_held_cards,
                ),
                screensaver_raster_moved_cards: usize_to_u64_saturating(
                    frame.screensaver_render_trace.raster_moved_cards,
                ),
                screensaver_raster_hold_layer_mask: frame
                    .screensaver_render_trace
                    .raster_hold_layer_mask,
                screensaver_raster_visible_layer_mask: frame
                    .screensaver_render_trace
                    .raster_visible_layer_mask,
                screensaver_phase_bank_bytes: usize_to_u64_saturating(
                    frame.screensaver_render_trace.phase_bank_resident_bytes,
                ),
                frame_production_class: frame.frame_production_trace.class.label(),
                frame_production_sequence: frame.frame_production_trace.sequence,
                frame_production_render_start_phase_us: frame
                    .frame_production_trace
                    .render_start_phase_us,
                frame_production_ready_depth: usize_to_u64_saturating(
                    frame.frame_production_trace.ready_depth,
                ),
                frame_production_ready_age_us: frame.frame_production_trace.ready_age_us,
                frame_production_render_wall_us: frame.frame_production_trace.render_wall_us,
                frame_production_starvation_count: frame.frame_production_trace.starvation_count,
                frame_production_cancelled: frame.frame_production_trace.cancelled,
            });
    }

    #[allow(clippy::too_many_arguments)]
    fn push_slow_frame_sample(
        &mut self,
        frame: &LauncherPresentedFrame,
        wall_us: u64,
        prepare_us: u64,
        render_us: u64,
        custom_draw_us: u64,
        vsync_us: u64,
        present_us: u64,
    ) {
        if self.slow_frame_samples.len() == FRAME_SLOW_SAMPLE_CAP {
            self.slow_frame_samples.remove(0);
        }
        let (dirty_y0, dirty_y1) = frame
            .dirty_rect
            .map(|rect| {
                (
                    usize_to_u32_saturating(rect.y0),
                    usize_to_u32_saturating(rect.y1),
                )
            })
            .unwrap_or((0, 0));
        self.slow_frame_samples
            .push(runtime_status::FrameBudgetSlowFrame {
                frame: frame.frames,
                severity: if wall_us > FRAME_BUDGET_US {
                    "cadence-overrun"
                } else {
                    "cadence-warning"
                },
                wall_us,
                warning_us: FRAME_CADENCE_WARNING_US,
                budget_us: FRAME_BUDGET_US,
                over_budget_us: wall_us.saturating_sub(FRAME_BUDGET_US),
                dominant_phase: dominant_frame_phase(
                    prepare_us,
                    render_us,
                    custom_draw_us,
                    vsync_us,
                    present_us,
                ),
                prepare_us,
                render_us,
                custom_draw_us,
                vsync_us,
                present_us,
                present_bytes: usize_to_u64_saturating(frame.present_bytes),
                wasted_present_bytes: usize_to_u64_saturating(frame.wasted_present_bytes),
                copied_rows: frame.copied_rows,
                direct_preview_rows: frame.direct_preview_rows,
                dirty_y0,
                dirty_y1,
                slint_timer_dispatch_us: u128_to_u64_saturating(
                    frame.prepare_trace.slint_timer_dispatch_us,
                ),
                navigation_commit_us: u128_to_u64_saturating(
                    frame.prepare_trace.navigation_commit_us,
                ),
                bridge_sync_us: u128_to_u64_saturating(frame.prepare_trace.bridge_sync_us),
                unattributed_prepare_us: prepare_us.saturating_sub(
                    u128_to_u64_saturating(frame.prepare_trace.slint_timer_dispatch_us)
                        .saturating_add(u128_to_u64_saturating(
                            frame.prepare_trace.navigation_commit_us,
                        ))
                        .saturating_add(u128_to_u64_saturating(frame.prepare_trace.bridge_sync_us))
                        .saturating_add(u128_to_u64_saturating(
                            frame.prepare_trace.catalog_worker_us,
                        ))
                        .saturating_add(u128_to_u64_saturating(frame.prepare_trace.media_worker_us))
                        .saturating_add(u128_to_u64_saturating(frame.prepare_trace.media_gate_us))
                        .saturating_add(u128_to_u64_saturating(
                            frame.prepare_trace.preview_schedule_us,
                        ))
                        .saturating_add(u128_to_u64_saturating(
                            frame.prepare_trace.preview_apply_us,
                        ))
                        .saturating_add(u128_to_u64_saturating(
                            frame.prepare_trace.status_string_copy_us,
                        )),
                ),
                catalog_worker_us: u128_to_u64_saturating(frame.prepare_trace.catalog_worker_us),
                catalog_message_count: frame.prepare_trace.catalog_message_count,
                catalog_backlog: frame.prepare_trace.catalog_backlog,
                catalog_ready_deferred: frame.prepare_trace.catalog_ready_deferred,
                catalog_ready_deferred_age_us: u128_to_u64_saturating(
                    frame.prepare_trace.catalog_ready_deferred_age_us,
                ),
                media_worker_us: u128_to_u64_saturating(frame.prepare_trace.media_worker_us),
                media_gate_us: u128_to_u64_saturating(frame.prepare_trace.media_gate_us),
                preview_schedule_us: u128_to_u64_saturating(
                    frame.prepare_trace.preview_schedule_us,
                ),
                preview_apply_us: u128_to_u64_saturating(frame.prepare_trace.preview_apply_us),
                preview_worker_drained: frame.prepare_trace.preview_worker_drained,
                preview_ready_processed: frame.prepare_trace.preview_ready_processed,
                preview_selected_processed: frame.prepare_trace.preview_selected_processed,
                preview_prefetch_processed: frame.prepare_trace.preview_prefetch_processed,
                preview_stale_results: frame.prepare_trace.preview_stale_results,
                preview_cache_inserts: frame.prepare_trace.preview_cache_inserts,
                preview_cache_evictions: frame.prepare_trace.preview_cache_evictions,
                preview_failed_results: frame.prepare_trace.preview_failed_results,
                preview_backlog: frame.prepare_trace.preview_backlog,
                status_write_due: frame.status_write_due,
                status_string_copy_us: u128_to_u64_saturating(
                    frame.prepare_trace.status_string_copy_us,
                ),
                status_string_copy_bytes: usize_to_u64_saturating(frame.status_string_copy_bytes),
                analytics_mode: frame_analytics_mode_label(self.frame_analytics_mode),
                vsync_source: vsync_source_label(frame.vsync_source),
                vsync_miss_streak: frame.vsync_miss_streak,
                vsync_stale_hits: frame.vsync_stale_hits,
                vsync_wait_start_age_us: frame.vsync_wait_start_age_us,
                vsync_accepted_hit_age_us: frame.vsync_accepted_hit_age_us,
                frame_start_phase_us: frame.frame_start_phase_us,
                present_phase_us: u128_to_u64_saturating(frame.present_phase_us),
            });
    }

    #[cfg(test)]
    fn current_frame_budget_status(&self) -> runtime_status::FrameBudgetStatus {
        self.frame_budget_status_with_samples(
            self.frame_analytics_samples.clone(),
            self.slow_frame_samples.clone(),
        )
    }

    fn take_frame_budget_status(&mut self) -> runtime_status::FrameBudgetStatus {
        let recent_frames = std::mem::replace(
            &mut self.frame_analytics_samples,
            Vec::with_capacity(FRAME_ANALYTICS_SAMPLE_CAP),
        );
        let slow_frames = std::mem::replace(
            &mut self.slow_frame_samples,
            Vec::with_capacity(FRAME_SLOW_SAMPLE_CAP),
        );
        self.frame_budget_status_with_samples(recent_frames, slow_frames)
    }

    fn frame_budget_status_with_samples(
        &self,
        recent_frames: Vec<runtime_status::FrameBudgetRecentFrame>,
        slow_frames: Vec<runtime_status::FrameBudgetSlowFrame>,
    ) -> runtime_status::FrameBudgetStatus {
        let total = self.frame_budget_total;
        let window = self.frame_budget_window;
        runtime_status::FrameBudgetStatus {
            budget_us: FRAME_BUDGET_US,
            frames_total: total.frames,
            over_budget_total: total.over_budget,
            over_20ms_total: total.over_20ms,
            over_33ms_total: total.over_33ms,
            max_wall_us: total.max_wall_us,
            latest_over_budget_frame: total.latest_over_budget_frame,
            latest_over_budget_wall_us: total.latest_over_budget_wall_us,
            max_vsync_miss_streak: total.max_vsync_miss_streak,
            vsync_total: total.vsync,
            fallback_total: total.fallback,
            timeout_total: total.timeout,
            error_total: total.error,
            window_frames: window.frames,
            window_over_budget: window.over_budget,
            window_over_20ms: window.over_20ms,
            window_over_33ms: window.over_33ms,
            window_max_wall_us: window.max_wall_us,
            window_max_vsync_miss_streak: window.max_vsync_miss_streak,
            window_prepare_us: FrameBudgetAccumulator::avg_us(window.prepare_us, window.frames),
            window_render_us: FrameBudgetAccumulator::avg_us(window.render_us, window.frames),
            window_custom_draw_us: FrameBudgetAccumulator::avg_us(
                window.custom_draw_us,
                window.frames,
            ),
            window_vsync_us: FrameBudgetAccumulator::avg_us(window.vsync_us, window.frames),
            window_present_us: FrameBudgetAccumulator::avg_us(window.present_us, window.frames),
            recent_frames,
            slow_frames,
        }
    }

    fn record_stable_samples(&mut self, frames: u64, disp: &mut MappedRgb565Framebuffer) {
        if frames == 30 && !self.stable_frame_logged {
            self.stable_frame_logged = true;
            boot_analytics::event("stable_frame", "frame=30");
            disp.record_visual_sample("stable_frame_30");
        } else if frames == 120 {
            disp.record_visual_sample("sample_frame_120");
        } else if frames == 240 {
            disp.record_visual_sample("sample_frame_240");
        }
    }

    #[cfg(feature = "profile")]
    fn record_boot_frame_profile(
        &mut self,
        frame: &LauncherPresentedFrame,
        disp: &MappedRgb565Framebuffer,
    ) {
        let reasserted = false;
        if self
            .boot_frame_profile
            .as_ref()
            .is_some_and(|profile| !profile.should_record(frame.frames))
        {
            self.boot_frame_profile = None;
        }
        if let Some(profile) = self.boot_frame_profile.as_mut() {
            let (edge1_hash, edge1_nonzero) = disp.right_edge_signature(1);
            let (edge8_hash, edge8_nonzero) = disp.right_edge_signature(8);
            let (left8_hash, left8_nonzero) = disp.left_edge_signature(8);
            let (top8_hash, top8_nonzero) = disp.top_edge_signature(8);
            let (bottom8_hash, bottom8_nonzero) = disp.bottom_edge_signature(8);
            let (full_sample_hash, full_sample_nonzero) = disp.sampled_signature();
            profile.record(
                frame.frames,
                (frame.frame_t1 - frame.frame_t0).as_micros() as u64,
                (frame.frame_t2 - frame.frame_t1).as_micros() as u64,
                (frame.frame_t3 - frame.frame_t2).as_micros() as u64,
                (frame.frame_t4 - frame.frame_t3).as_micros() as u64,
                frame.copied_rows,
                reasserted,
                edge1_hash,
                edge1_nonzero,
                edge8_hash,
                edge8_nonzero,
                left8_hash,
                left8_nonzero,
                top8_hash,
                top8_nonzero,
                bottom8_hash,
                bottom8_nonzero,
                full_sample_hash,
                full_sample_nonzero,
            );
        }
    }

    fn record_first_frame(
        &mut self,
        frame: &LauncherPresentedFrame,
        start: Instant,
        catalog_ready: bool,
    ) {
        if frame.copied_rows > 0 && !self.first_frame_logged {
            self.first_frame_logged = true;
            boot_analytics::event("first_frame", format!("catalog_ready={catalog_ready}"));
            print_startup_event(
                start,
                "first_frame",
                format!("catalog_ready={catalog_ready}"),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write_runtime_status(
        &mut self,
        status_write_due: bool,
        frames: u64,
        run_start: Instant,
        status: FrameStatusView<'_>,
        arcade_selected: usize,
        arcade_visual_index: f32,
        preview_cache_state: &str,
        preview_transition_effect: &str,
        preview_transition_progress: f32,
        screensaver_active_cards: usize,
        composition_status: &UiCompositionStatus,
        idle_status: Option<(u64, u64)>,
    ) {
        let FrameStatusView {
            nav,
            pad,
            catalog,
            catalog_ready,
            catalog_refresh_done,
            launching,
            loading_title,
            catalog_scan_visible,
            catalog_scan_percent,
            catalog_background_scan_visible,
            confirm_visible,
            confirm_selected,
            route_reassert_count,
            last_route_reassert_frame,
            last_route_reassert_ok,
            last_route_reassert_error,
            startup_status,
            return_session,
            ..
        } = status;
        let FrameStatusStrings {
            catalog_scan_title,
            catalog_scan_detail,
            catalog_scan_message,
            confirm_title,
            confirm_message,
            confirm_left_label,
            confirm_right_label,
        } = status.strings();
        if !status_write_due {
            return;
        }
        let idle = idle_status.is_some();
        let (idle_loops, last_frame_ms_ago) = idle_status.unwrap_or((0, 0));
        let fps_estimate = if run_start.elapsed().as_secs_f64() > 0.0 {
            frames as f64 / run_start.elapsed().as_secs_f64()
        } else {
            0.0
        };
        let rolling_fps = if idle { 0.0 } else { self.last_rolling_fps };
        let rolling_prepare_us = if idle {
            0
        } else {
            self.last_rolling_prepare_us
        };
        let rolling_render_us = if idle { 0 } else { self.last_rolling_render_us };
        let rolling_custom_draw_us = if idle {
            0
        } else {
            self.last_rolling_custom_draw_us
        };
        let rolling_vsync_us = if idle { 0 } else { self.last_rolling_vsync_us };
        let rolling_present_us = if idle {
            0
        } else {
            self.last_rolling_present_us
        };
        let rolling_rows = if idle { 0 } else { self.last_rolling_rows };
        let last_frame_budget_status =
            (!idle).then(|| self.frame_budget_status_with_samples(Vec::new(), Vec::new()));
        let frame_budget = if idle {
            self.last_frame_budget_status.clone()
        } else {
            self.take_frame_budget_status()
        };
        self.status_sequence = self.status_sequence.saturating_add(1);
        let receipt_system = nav.active_collection_scope_id(catalog);
        let receipt_key = nav
            .active_arcade_game_at(catalog, receipt_system, nav.arcade.selected)
            .map_or("", |game| game.preview_asset_key.as_ref());
        let media_receipt = format!(
            "key={receipt_key} cache={preview_cache_state} catalog_generation={} view={} present_backend={} present_status={} frozen={} output={}x{}",
            self.catalog_generation,
            self.effective_view,
            self.last_present_backend,
            self.last_present_status,
            self.display_frozen,
            self.framebuffer_width,
            self.framebuffer_height
        );
        if self.last_media_receipt != media_receipt {
            crate::media_diagnostics::record("preview_presentation_receipt", &media_receipt, false);
            self.last_media_receipt = media_receipt;
        }
        let screensaver_profile_state = cpu_profile::screensaver_profile_state();
        let build_identity = crate::build_identity::BuildIdentity::current();
        let selected_system_id = nav.active_collection_scope_id(catalog);
        let selected_game = (nav.screen == Screen::Arcade)
            .then(|| nav.active_arcade_game_at(catalog, selected_system_id, nav.arcade.selected))
            .flatten();
        let status_submitted = self.runtime_status_publisher.submit(LauncherStatus {
            build_package_version: build_identity.package_version,
            build_version: build_identity.version,
            build_number: build_identity.build_number,
            build_source_revision: build_identity.source_revision,
            build_source_dirty: build_identity.source_dirty_label(),
            build_time: build_identity.build_time,
            build_arch: build_identity.arch,
            scene: "launcher",
            screen: self.effective_view,
            effective_view: self.effective_view,
            return_screen: screen_label(nav.screen),
            menu_id: nav.current_menu_id(),
            selected_item_id: nav.current_menu_selected_item_id(),
            active_collection_id: nav.active_collection_id().unwrap_or(""),
            selected_system_id,
            selected_game_id: selected_game.map_or("", |game| game.mra_path.as_ref()),
            selected_game_title: selected_game.map_or("", |game| game.title.as_ref()),
            preview_asset_key: selected_game.map_or("", |game| game.preview_asset_key.as_ref()),
            catalog_generation: &self.catalog_generation,
            output_route: self.output_route,
            crt_font_experiment: self.crt_font_experiment,
            framebuffer_width: self.framebuffer_width,
            framebuffer_height: self.framebuffer_height,
            frames,
            idle,
            idle_loops,
            status_sequence: self.status_sequence,
            fps_estimate,
            rolling_fps,
            rolling_prepare_us,
            rolling_render_us,
            rolling_custom_draw_us,
            rolling_vsync_us,
            rolling_present_us,
            rolling_rows,
            last_frame_ms_ago,
            vsync_source: self.last_vsync_source,
            vsync_period_us: self.last_vsync_period_us,
            present_backend: self.last_present_backend,
            present_status: self.last_present_status,
            latch_failure_state: &self.latch_failure_state,
            latch_failure_stage: &self.latch_failure_stage,
            latch_failure_reason: &self.latch_failure_reason,
            latch_failure_detail: &self.latch_failure_detail,
            display_frozen: self.display_frozen,
            present_buffer: self.last_present_buffer,
            latch_publish_us: self.last_latch_publish_us,
            latch_sequence: self.last_latch_sequence,
            latch_flip_count: self.last_latch_flip_count,
            latch_drop_count: self.last_latch_drop_count,
            startup_intro: self.startup_intro.clone(),
            catalog_ready,
            catalog_games: catalog.len(),
            catalog_systems: catalog.systems.len(),
            catalog_refresh_done,
            catalog_refresh_policy: catalog_refresh_policy().label(),
            catalog_worker_enabled: catalog_refresh_policy().worker_enabled(),
            selected_game_has_preview: selected_game.is_some_and(|game| game.has_preview),
            screensaver_profile_state,
            catalog_scan_visible,
            catalog_scan_message,
            catalog_scan_title,
            catalog_scan_detail,
            catalog_scan_percent,
            catalog_background_scan_visible,
            confirm_visible,
            confirm_title,
            confirm_message,
            confirm_selected,
            confirm_left_label,
            confirm_right_label,
            arcade_selected,
            arcade_visual_index,
            arcade_scroll_y: nav.arcade.scroll_y,
            arcade_drawer_open: nav.arcade_filter.drawer_open,
            arcade_drawer_level: nav.arcade_filter.title(),
            arcade_drawer_selected: nav.arcade_filter.selected,
            arcade_drawer_requested_hash: if nav.arcade_filter.drawer_open {
                crate::arcade_list_renderer::requested_filter_content_hash()
            } else {
                0
            },
            arcade_drawer_rendered_hash: if nav.arcade_filter.drawer_open {
                crate::arcade_list_renderer::rendered_filter_content_hash()
            } else {
                0
            },
            arcade_search_active: nav.arcade_search.is_active(&nav.arcade_filter.active),
            arcade_search_status: match nav.arcade_search.status {
                crate::launcher::ArcadeSearchStatus::Idle => "idle",
                crate::launcher::ArcadeSearchStatus::Searching => "searching",
                crate::launcher::ArcadeSearchStatus::Ready => "ready",
                crate::launcher::ArcadeSearchStatus::Failed => "failed",
            },
            arcade_search_query: &nav.arcade_search.query,
            arcade_search_results: nav.arcade_search_result_count(),
            preview_cache_state,
            preview_presentation_state: composition_status.preview_state,
            preview_presentation_generation: composition_status.preview_generation,
            preview_transition_effect,
            preview_transition_progress,
            screensaver_active_cards,
            composition_state: composition_status.state,
            composition_recovery_count: composition_status.recovery_count,
            direct_layer_retirement_state: composition_status.retirement_state,
            direct_layer_retirement_generation: composition_status.retirement_generation,
            direct_layer_retirement_obligations: composition_status.retirement_obligations,
            direct_layer_retirement_receipt: &composition_status.retirement_receipt,
            direct_layer_retirement_receipt_sequence: composition_status
                .retirement_receipt_sequence,
            direct_layer_retirement_receipt_slot: composition_status.retirement_receipt_slot,
            direct_layer_retirement_receipt_route_epoch: composition_status
                .retirement_receipt_route_epoch,
            last_composition_invariant_kind: &composition_status.last_invariant_kind,
            last_composition_invariant_detail: &composition_status.last_invariant_detail,
            route_reassert_count,
            last_route_reassert_frame,
            last_route_reassert_ok,
            last_route_reassert_error,
            launch_state: if launching { "launching" } else { "idle" },
            loading_title,
            input_pad_count: pad.len(),
            active_pad_index: pad.active_idx(),
            active_pad_name: &pad.info().name,
            active_pad_path: pad.path(),
            last_raw_event: &pad.state().last_raw,
            last_input_ms_ago: if pad.state().last_raw_event.is_some() {
                0
            } else {
                u64::MAX
            },
            startup_mode: startup_status.mode.label(),
            startup_reveal_state: startup_status.state.label(),
            return_source: return_session.source,
            return_phase: return_session.phase,
            return_fallback_reason: &return_session.fallback_reason,
            revealed: startup_status.revealed,
            input_enabled: startup_status.input_enabled,
            reveal_ms: startup_status.reveal_ms,
            input_enabled_ms: startup_status.input_enabled_ms,
            process_start_monotonic_us: crate::process_start_monotonic_us(),
            exact_context_monotonic_us: return_session.exact_context_monotonic_us,
            preview_ready_monotonic_us: return_session.preview_ready_monotonic_us,
            first_correct_present_monotonic_us: return_session.first_correct_present_monotonic_us,
            frame_budget,
            phase_profile: super::phase_profile::latest(),
        });
        if screensaver_profile_state == "complete" && status_submitted {
            self.profile_completion_submitted = true;
        }
        if !idle {
            self.last_frame_budget_status =
                last_frame_budget_status.expect("rendered status has a cached summary");
            self.frame_budget_window = FrameBudgetAccumulator::default();
        }
        self.last_status_write = Instant::now();
        if idle {
            self.idle_loops_since_status = 0;
        }
    }
}

fn cpu_delta(start: FrameAnalyticsCpuStamp, end: FrameAnalyticsCpuStamp) -> u64 {
    end.thread_us.saturating_sub(start.thread_us)
}

fn vsync_source_label(source: Option<VsyncPaceSource>) -> &'static str {
    match source {
        Some(VsyncPaceSource::Vsync) => "vsync",
        Some(VsyncPaceSource::Fallback) => "fallback",
        Some(VsyncPaceSource::Timeout) => "timeout",
        Some(VsyncPaceSource::Error) => "error",
        None => "none",
    }
}

fn should_defer_runtime_status_write(frame: &LauncherPresentedFrame) -> bool {
    frame.status_write_due && frame.composition_status.state == "navigation-transition"
}

fn frame_analytics_mode_label(mode: FrameAnalyticsMode) -> &'static str {
    match mode {
        FrameAnalyticsMode::Off => "off",
        FrameAnalyticsMode::Wall => "wall",
        FrameAnalyticsMode::Thread => "thread",
        FrameAnalyticsMode::Process => "process",
    }
}

fn dominant_frame_phase(
    prepare_us: u64,
    render_us: u64,
    custom_draw_us: u64,
    vsync_us: u64,
    present_us: u64,
) -> &'static str {
    [
        ("prepare", prepare_us),
        ("slint-render", render_us),
        ("custom-draw", custom_draw_us),
        ("vsync", vsync_us),
        ("fb-present", present_us),
    ]
    .into_iter()
    .max_by_key(|(_, value)| *value)
    .map(|(label, _)| label)
    .unwrap_or("unknown")
}

#[cfg(target_os = "linux")]
pub(super) fn cpu_thread_us() -> Option<u64> {
    clock_us(libc::CLOCK_THREAD_CPUTIME_ID)
}

/// Keep a baseline's identity and optional read bracket together. Replacing an
/// unbracketed observation must not inherit timestamps from an earlier read.
#[cfg(feature = "tooling")]
#[derive(Clone, Copy)]
pub(super) struct ToolingPresentationObservation {
    pub(super) telemetry: mister_magik_latch_contract::PresentationTelemetry,
    pub(super) at: Instant,
    pub(super) motion: bool,
    pub(super) attempt_id: u64,
    pub(super) read_bracket_us: Option<[u64; 2]>,
}
#[cfg(feature = "tooling")]
impl ToolingPresentationObservation {
    /// Called only on the renderer's no-work path. Pending animation is not idle.
    pub(super) fn retire_motion_for_idle(&mut self, motion_active: bool) -> bool {
        if motion_active || !self.motion {
            return false;
        }
        self.motion = false;
        true
    }

    pub(super) fn new(
        telemetry: mister_magik_latch_contract::PresentationTelemetry,
        at: Instant,
        motion: bool,
        attempt_id: u64,
        read_before: Option<Instant>,
        origin: Instant,
    ) -> Self {
        Self {
            telemetry,
            at,
            motion,
            attempt_id,
            read_bracket_us: read_before.map(|before| {
                [
                    before.saturating_duration_since(origin).as_micros() as u64,
                    at.saturating_duration_since(origin).as_micros() as u64,
                ]
            }),
        }
    }
}

/// Observe the pose before rendering can retire it. This also supplies the
/// shared motion decision for confirmed and superseded presentations, even
/// when the telemetry read fails.
#[cfg(feature = "tooling")]
#[allow(clippy::too_many_arguments)]
pub(super) fn capture_evidence_state(
    evidence: &mut Option<mister_magik_tooling_support::frame_evidence::FrameEvidence>,
    nav: &LauncherNav,
    card: Option<&super::launcher_card_home::LauncherCardHomeSession>,
    class: FrameProductionClass,
    logical_time_us: u64,
    input_generation: u64,
    produced_frame_id: u64,
) -> bool {
    let card = card.filter(|_| nav.screen == Screen::Home);
    let motion = class != FrameProductionClass::EventDriven
        || nav.screen == Screen::Arcade && nav.arcade.is_scroll_active()
        || card.is_some_and(|card| card.is_animating());
    if let Some(frame) = evidence.as_mut() {
        frame.motion = motion;
        frame.produced_frame_id = produced_frame_id;
        frame.logical_time_us = logical_time_us;
        frame.view = if nav.screen == Screen::Home {
            "home"
        } else if nav.screen == Screen::Arcade {
            if nav.system_page_mode == launcher::SystemPageMode::Hub {
                "system-hub"
            } else {
                "games-list"
            }
        } else {
            "other"
        };
        frame.menu_token = nav
            .current_menu_id()
            .bytes()
            .fold(0xcbf29ce484222325u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
            });
        frame.selected = if nav.screen == Screen::Home {
            nav.selected
        } else {
            nav.arcade.selected
        };
        frame.input_generation = input_generation;
        if let Some(card) = card {
            (frame.pose_phase, frame.pose_progress) = card.evidence_pose();
            frame.content_generation = Some(card.content_generation());
            frame.card_snapshot_locked = card.is_level_trick_active();
        }
    }
    motion
}

/// Input can invalidate a completed disposable raster before it is posted.
/// Keep the produced ID and execution span; only refresh telemetry counts drops.
#[cfg(feature = "tooling")]
pub(super) fn record_abandoned_evidence_raster(
    evidence: &mut Option<mister_magik_tooling_support::frame_evidence::FrameEvidence>,
    session: Option<&mut mister_magik_tooling_support::Session>,
    origin: Instant,
    render_start: Instant,
    render_end: Instant,
) {
    capture_evidence_cpu(evidence, 2, origin);
    capture_evidence_cpu(evidence, 6, origin);
    if let Some(mut frame) = evidence.take()
        && let Some(session) = session
    {
        let started = Instant::now();
        frame.outcome = "input-priority-restart";
        frame.finish_us = started.saturating_duration_since(origin).as_micros() as u64;
        frame.record.reason = "raster abandoned for input before posting; not a counted drop";
        frame.record.timeline = Some(
            mister_magik_tooling_support::measurement::FramePhaseTimeline {
                frame_begin_us: frame.begin_us,
                render_start_us: render_start.saturating_duration_since(origin).as_micros() as u64,
                render_end_us: render_end.saturating_duration_since(origin).as_micros() as u64,
                ..Default::default()
            },
        );
        session.record_frame_evidence(frame, started);
    }
}

/// Diagnostic-only CPU samples bracketed in the app timeline. Unavailable stays null.
#[cfg(feature = "tooling")]
pub(super) fn capture_evidence_cpu(
    frame: &mut Option<mister_magik_tooling_support::frame_evidence::FrameEvidence>,
    index: usize,
    origin: Instant,
) {
    if let Some(frame) = frame.as_mut()
        && frame.phases_enabled
    {
        let before = Instant::now();
        frame.cpu_us[index] = cpu_thread_us();
        let after = Instant::now();
        frame.cpu_brackets_us[index] = [
            before.saturating_duration_since(origin).as_micros() as u64,
            after.saturating_duration_since(origin).as_micros() as u64,
        ];
        frame.observer_sampling_us += after.saturating_duration_since(before).as_micros() as u64;
    }
}

/// Calling thread's cumulative run delay: time runnable while another task
/// held its CPU. Measurement only: each call reads procfs.
#[cfg(target_os = "linux")]
pub(super) fn thread_run_delay_us() -> Option<u64> {
    let value = std::fs::read_to_string("/proc/thread-self/schedstat").ok()?;
    Some(value.split_whitespace().nth(1)?.parse::<u64>().ok()? / 1_000)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn thread_run_delay_us() -> Option<u64> {
    None
}

#[cfg(not(target_os = "linux"))]
pub(super) fn cpu_thread_us() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
pub(super) fn cpu_process_us() -> Option<u64> {
    clock_us(libc::CLOCK_PROCESS_CPUTIME_ID)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn cpu_process_us() -> Option<u64> {
    None
}

pub(super) fn monotonic_clock_us() -> Option<u64> {
    clock_us(libc::CLOCK_MONOTONIC)
}

fn clock_us(clock_id: libc::clockid_t) -> Option<u64> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is valid writable storage for this syscall; errors are
    // represented as missing CPU timing so telemetry remains best-effort.
    let rc = unsafe { libc::clock_gettime(clock_id, &mut ts) };
    (rc == 0).then(|| {
        (ts.tv_sec as u64)
            .saturating_mul(1_000_000)
            .saturating_add((ts.tv_nsec as u64) / 1_000)
    })
}

fn u128_to_u64_saturating(value: u128) -> u64 {
    value.min(u128::from(u64::MAX)) as u64
}

fn usize_to_u64_saturating(value: usize) -> u64 {
    value.min(u64::MAX as usize) as u64
}

fn usize_to_u32_saturating(value: usize) -> u32 {
    value.min(u32::MAX as usize) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "tooling")]
    #[test]
    fn settled_idle_retires_the_motion_baseline_but_resumed_overruns_still_count() {
        let origin = Instant::now();
        let telemetry = mister_magik_latch_contract::PresentationTelemetry {
            owned_vblank_count: 40,
            presented_vblank_count: 40,
            repeated_vblank_count: 0,
            ownership_loss_count: 0,
            active_sequence: 40,
            flags: 0,
            crc: 0,
        };
        let mut moving =
            ToolingPresentationObservation::new(telemetry, origin, true, 40, Some(origin), origin);
        assert!(!moving.retire_motion_for_idle(true));
        assert!(moving.motion);
        assert!(moving.retire_motion_for_idle(false));
        assert!(!moving.retire_motion_for_idle(false));
        assert_eq!(moving.telemetry.owned_vblank_count, 40);
        // The next render refreshes this idle baseline before doing any work.
        let resumed = ToolingPresentationObservation::new(
            mister_magik_latch_contract::PresentationTelemetry {
                owned_vblank_count: 160,
                repeated_vblank_count: 120,
                ..telemetry
            },
            origin + Duration::from_secs(2),
            false,
            161,
            None,
            origin,
        );
        assert!(!resumed.motion);
        assert_eq!(resumed.telemetry.repeated_vblank_count, 120);
        assert_eq!(
            mister_magik_tooling_support::measurement::first_frame_drops(1, 25_000, 16_667),
            1
        );
        assert_eq!(
            mister_magik_tooling_support::measurement::first_frame_drops(1, 8_000, 16_667),
            0
        );
    }

    #[cfg(feature = "tooling")]
    #[test]
    fn evidence_tracks_scroll_telemetry_failure_restart_and_replacement() {
        use mister_magik_tooling_support::{Session, frame_evidence::EvidenceMode};
        let now = Instant::now();
        let mut session = Session::new(std::env::temp_dir().join("unused-launcher-evidence-test"));
        session
            .metrics
            .frame_evidence
            .reset(EvidenceMode::Neighbors);
        session.metrics.motion_started_ms = Some(0);
        session.metrics.window_start = Some((0, session.metrics.counters.clone()));
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.arcade.handle_direction_input(1, 0, now, 10);
        assert!(nav.arcade.is_scroll_active());
        let mut first = session.frame_evidence_candidate(1, 0);
        assert!(capture_evidence_state(
            &mut first,
            &nav,
            None,
            FrameProductionClass::EventDriven,
            0,
            1,
            1
        ));
        // Telemetry failure has no state update: the real navigation state
        // still labels the motion and causes retention, including successors.
        let mut frame = first.take().unwrap();
        frame.outcome = "active";
        assert!(frame.motion && !frame.telemetry_valid);
        session.record_frame_evidence(frame, now);
        let mut superseded = session.frame_evidence_candidate(2, 16_667);
        capture_evidence_state(
            &mut superseded,
            &nav,
            None,
            FrameProductionClass::EventDriven,
            16_667,
            1,
            2,
        );
        let mut frame = superseded.take().unwrap();
        frame.outcome = "superseded-before-confirmation";
        session.record_frame_evidence(frame, now);
        // A disposable Home raster is produced, then abandoned for new input.
        nav.screen = Screen::Home;
        let mut raster = session.frame_evidence_candidate(3, 33_334);
        capture_evidence_state(
            &mut raster,
            &nav,
            None,
            FrameProductionClass::EventDriven,
            33_334,
            1,
            3,
        );
        let work = mister_magik_tooling_support::measurement::FrameWorkTiming {
            producer_us: 123,
            ..Default::default()
        };
        raster.as_mut().unwrap().record.work = Some(work);
        session.metrics.counters.card_rendered_frames = 1;
        session.metrics.counters.card_producer_total_us = work.producer_us;
        record_abandoned_evidence_raster(&mut raster, Some(&mut session), now, now, now);
        assert_eq!(session.metrics.counters.card_rendered_frames, 1);
        assert_eq!(session.metrics.counters.card_producer_total_us, 123);
        assert_eq!(session.metrics.counters.presentations, 0);
        assert!(raster.is_none());
        nav.selected = 1;
        let mut replacement = session.frame_evidence_candidate(4, 33_334);
        capture_evidence_state(
            &mut replacement,
            &nav,
            None,
            FrameProductionClass::EventDriven,
            33_334,
            2,
            4,
        );
        let mut frame = replacement.unwrap();
        frame.outcome = "active";
        session.record_frame_evidence(frame, now);
        let capture = session.metrics.frame_evidence.json();
        let frames = capture["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 4);
        assert_eq!(capture["unrecorded_loop_iterations"], 0);
        for (i, frame) in frames.iter().enumerate() {
            assert_eq!(frame["produced_frame_id"], i + 1);
            assert_eq!(frame["observation"]["dropped_frames"], 0);
        }
        assert_eq!(frames[0]["motion"], true);
        assert_eq!(frames[1]["motion"], true);
        assert_eq!(frames[2]["outcome"], "input-priority-restart");
        assert_eq!(frames[2]["observation"]["work"]["producer_us"], 123);
        assert_eq!(frames[3]["input_generation"], 2);
    }

    #[cfg(feature = "tooling")]
    #[test]
    fn observation_replacement_never_carries_a_previous_read_bracket() {
        let origin = Instant::now();
        let telemetry = mister_magik_latch_contract::PresentationTelemetry {
            owned_vblank_count: 0,
            presented_vblank_count: 0,
            repeated_vblank_count: 0,
            ownership_loss_count: 0,
            active_sequence: 0,
            flags: 0,
            crc: 0,
        };
        let first = ToolingPresentationObservation::new(
            telemetry,
            origin + Duration::from_micros(2),
            false,
            1,
            Some(origin),
            origin,
        );
        assert_eq!(first.read_bracket_us, Some([0, 2]));
        // Warmup or a prior OFF window does not perform bracket reads.
        let next = ToolingPresentationObservation::new(
            telemetry,
            origin + Duration::from_micros(9),
            true,
            2,
            None,
            origin,
        );
        assert_eq!(next.attempt_id, 2);
        assert_eq!(next.read_bracket_us, None);
        let measured = ToolingPresentationObservation::new(
            telemetry,
            origin + Duration::from_micros(14),
            true,
            3,
            Some(origin + Duration::from_micros(11)),
            origin,
        );
        assert_eq!(measured.read_bracket_us, Some([11, 14]));
    }

    #[test]
    fn fresh_analytics_lease_keeps_previous_mode_during_transient_read_failure() {
        let error = std::io::Error::other("transient empty replacement");
        assert_eq!(
            fresh_frame_analytics_mode(FrameAnalyticsMode::Process, true, Err(&error)),
            FrameAnalyticsMode::Process
        );
        assert_eq!(
            fresh_frame_analytics_mode(FrameAnalyticsMode::Thread, true, Ok("")),
            FrameAnalyticsMode::Thread
        );
        assert_eq!(
            fresh_frame_analytics_mode(FrameAnalyticsMode::Thread, true, Ok("off\n")),
            FrameAnalyticsMode::Off
        );
    }

    #[test]
    fn missing_or_expired_analytics_lease_disables_sampling() {
        assert_eq!(
            fresh_frame_analytics_mode(FrameAnalyticsMode::Process, false, Ok("process\n")),
            FrameAnalyticsMode::Off
        );
        assert_eq!(
            fresh_frame_analytics_mode(
                FrameAnalyticsMode::Thread,
                false,
                Err(&std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "lease removed"
                ))
            ),
            FrameAnalyticsMode::Off
        );
    }

    #[test]
    fn analytics_lease_mode_survives_consecutive_status_intervals() {
        let first = fresh_frame_analytics_mode(FrameAnalyticsMode::Off, true, Ok("process\n"));
        let second = fresh_frame_analytics_mode(first, true, Ok("process\n"));
        let transient = fresh_frame_analytics_mode(
            second,
            true,
            Err(&std::io::Error::other("replacement temporarily unreadable")),
        );

        assert_eq!(first, FrameAnalyticsMode::Process);
        assert_eq!(second, FrameAnalyticsMode::Process);
        assert_eq!(transient, FrameAnalyticsMode::Process);
        assert_eq!(
            fresh_frame_analytics_mode(transient, true, Ok("off\n")),
            FrameAnalyticsMode::Off
        );
    }

    fn sample(frame: u64, wall_us: u64) -> FrameBudgetSample {
        FrameBudgetSample {
            frame,
            wall_us,
            prepare_us: 100,
            render_us: 200,
            custom_draw_us: 300,
            vsync_us: 400,
            present_us: 500,
            vsync_source: Some(VsyncPaceSource::Vsync),
            vsync_miss_streak: 0,
        }
    }

    fn presented_frame(frame: u64, loop_start: Instant, wall_us: u64) -> LauncherPresentedFrame {
        let frame_t0 = loop_start;
        let frame_t1 = loop_start + Duration::from_micros(100);
        let frame_t2 = frame_t1 + Duration::from_micros(200);
        let custom_draw_start = frame_t2;
        let custom_draw_done = custom_draw_start + Duration::from_micros(300);
        let frame_t3 = custom_draw_done + Duration::from_micros(400);
        let frame_t4 = loop_start + Duration::from_micros(wall_us);
        LauncherPresentedFrame {
            frames: frame,
            selection_feedback: SelectionFeedbackStamp::default(),
            selected: 0,
            visual_index: 0.0,
            startup_start: loop_start,
            startup_monotonic_us: 1_000_000,
            run_start: loop_start,
            loop_start,
            frame_t0,
            frame_t1,
            frame_t2,
            frame_t3,
            frame_t4,
            pre_render_wait_us: 400,
            post_present_wait_us: 800,
            custom_draw_start,
            custom_draw_done,
            custom_draw_trace: LauncherCustomDrawTrace::default(),
            prepare_trace: LauncherPrepareTrace {
                slint_timer_dispatch_us: 0,
                navigation_commit_us: 0,
                bridge_sync_us: 0,
                bridge_model_projection_us: 0,
                bridge_model_replacements: 0,
                bridge_row_mutations: 0,
                bridge_row_allocations: 0,
                bridge_shared_string_constructions: 0,
                bridge_model_allocation_us: 0,
                catalog_worker_us: 50,
                catalog_message_count: 2,
                catalog_backlog: 1,
                catalog_ready_deferred: true,
                catalog_ready_deferred_age_us: 700,
                media_worker_us: 60,
                media_gate_us: 7,
                preview_schedule_us: 8,
                preview_apply_us: 9,
                preview_worker_drained: 5,
                preview_ready_processed: 4,
                preview_selected_processed: 1,
                preview_prefetch_processed: 3,
                preview_stale_results: 1,
                preview_cache_inserts: 4,
                preview_cache_evictions: 2,
                preview_failed_results: 1,
                preview_backlog: 6,
                status_string_copy_us: 10,
            },
            prepare_us: 1_000,
            dirty_rect: Some(DirtyRect {
                x0: 0,
                y0: 12,
                x1: 960,
                y1: 24,
            }),
            copied_rows: 12,
            direct_preview_rows: 4,
            present_bytes: 23_040,
            wasted_present_bytes: 1_280,
            fb_present_us_override: None,
            vsync_us_override: None,
            cached_present_us: 0,
            hidden_compose_us: 0,
            hidden_preview_compose_us: 0,
            hidden_arcade_compose_us: 0,
            direct_preview_present_us: 0,
            arcade_list_present_us: 0,
            main_present_backend: LauncherPresentBackend::Fb0Dirty,
            main_present_status: LauncherPresentStatus::None,
            main_present_buffer: 0,
            main_present_hidden_copy_us: 0,
            main_present_hidden_publish_us: 0,
            main_present_hidden_copied_bytes: 0,
            main_present_hidden_invalid_bytes: 0,
            main_present_hidden_rect_count: 0,
            main_present_hidden_catchup_bytes: 0,
            main_present_hidden_full_copy: false,
            main_present_copy_path: "vertical-partial",
            main_present_request_us: 0,
            main_present_set_vga_fb_us: 0,
            main_present_wait_us: 0,
            main_present_sequence: 0,
            main_present_post_active_sequence: 0,
            main_present_post_pending_sequence: 0,
            main_present_post_pending: false,
            main_present_active_sequence: 0,
            main_present_pending: false,
            main_present_completion_poll_count: 0,
            main_present_completion_poll_wall_us: 0,
            main_present_completion_poll_cpu_us: 0,
            main_present_flip_count: 0,
            main_present_drop_count: 0,
            main_present_receipt_crc: 0,
            vsync_source: Some(VsyncPaceSource::Timeout),
            vsync_period_us: 16_667,
            vsync_miss_streak: 3,
            vsync_stale_hits: 0,
            vsync_wait_start_age_us: 12_000,
            vsync_accepted_hit_age_us: 500,
            frame_start_phase_us: 8_000,
            present_phase_us: 0,
            redraw_pending: true,
            wake_reasons_bits: 0x40,
            preview_cache_state: "exact",
            preview_transition: PreviewTransitionTrace::default(),
            composition_status: UiCompositionStatus::default(),
            screensaver_active: false,
            screensaver_active_cards: 0,
            frame_production_trace: FrameProductionTrace::default(),
            screensaver_render_trace: ScreensaverRenderTrace::default(),
            status_write_due: false,
            status_string_copy_bytes: 128,
            clock_update_due: false,
            clock_update_us: 0,
            cpu_loop_start: FrameAnalyticsCpuStamp::default(),
            cpu_t0: FrameAnalyticsCpuStamp::default(),
            cpu_t1: FrameAnalyticsCpuStamp::default(),
            cpu_t2: FrameAnalyticsCpuStamp::default(),
            cpu_custom_draw_start: FrameAnalyticsCpuStamp::default(),
            cpu_custom_draw_done: FrameAnalyticsCpuStamp::default(),
            cpu_t3: FrameAnalyticsCpuStamp::default(),
            cpu_t4: FrameAnalyticsCpuStamp::default(),
        }
    }

    fn builder_from_frame(frame: &LauncherPresentedFrame) -> LauncherFrameSnapshotBuilder {
        LauncherFrameSnapshotBuilder {
            identity: LauncherFrameIdentity {
                frames: frame.frames,
                selection_feedback: frame.selection_feedback.clone(),
                selected: frame.selected,
                visual_index: frame.visual_index,
            },
            timing: LauncherFrameTiming {
                startup_start: frame.startup_start,
                startup_monotonic_us: frame.startup_monotonic_us,
                run_start: frame.run_start,
                loop_start: frame.loop_start,
                frame_t0: frame.frame_t0,
                frame_t1: frame.frame_t1,
                frame_t2: frame.frame_t2,
                frame_t3: frame.frame_t3,
                frame_t4: frame.frame_t4,
                pre_render_wait_us: frame.pre_render_wait_us,
                post_present_wait_us: frame.post_present_wait_us,
                custom_draw_start: frame.custom_draw_start,
                custom_draw_done: frame.custom_draw_done,
                prepare_us: frame.prepare_us,
                redraw_pending: frame.redraw_pending,
                wake_reasons_bits: frame.wake_reasons_bits,
            },
            render: LauncherFrameRenderData {
                custom_draw_trace: frame.custom_draw_trace,
                prepare_trace: frame.prepare_trace,
                dirty_rect: frame.dirty_rect,
                preview_cache_state: frame.preview_cache_state,
                preview_transition: frame.preview_transition,
                composition_status: frame.composition_status.clone(),
                screensaver_active: frame.screensaver_active,
                screensaver_active_cards: frame.screensaver_active_cards,
                frame_production_trace: frame.frame_production_trace,
                screensaver_render_trace: frame.screensaver_render_trace,
            },
            pacing: LauncherPacingTrace {
                vsync_source: frame.vsync_source,
                vsync_period_us: frame.vsync_period_us,
                vsync_miss_streak: frame.vsync_miss_streak,
                vsync_stale_hits: frame.vsync_stale_hits,
                vsync_wait_start_age_us: frame.vsync_wait_start_age_us,
                vsync_accepted_hit_age_us: frame.vsync_accepted_hit_age_us,
                frame_start_phase_us: frame.frame_start_phase_us,
                present_phase_us: frame.present_phase_us,
            },
            presentation: LauncherPresentResult {
                readiness_source_evidence: None,
                copied_rows: frame.copied_rows,
                direct_preview_rows: frame.direct_preview_rows,
                present_bytes: frame.present_bytes,
                wasted_present_bytes: frame.wasted_present_bytes,
                fb_present_us_override: frame.fb_present_us_override,
                vsync_us_override: frame.vsync_us_override,
                cached_present_us: frame.cached_present_us,
                hidden_compose_us: frame.hidden_compose_us,
                hidden_preview_compose_us: frame.hidden_preview_compose_us,
                hidden_arcade_compose_us: frame.hidden_arcade_compose_us,
                direct_preview_present_us: frame.direct_preview_present_us,
                arcade_list_present_us: frame.arcade_list_present_us,
                arcade_copy_trace: crate::arcade_list_renderer::PersistentArcadeCopyTrace::default(
                ),
                main_present_backend: frame.main_present_backend,
                main_present_status: frame.main_present_status,
                main_present_buffer: frame.main_present_buffer,
                main_present_hidden_copy_us: frame.main_present_hidden_copy_us,
                main_present_hidden_publish_us: frame.main_present_hidden_publish_us,
                main_present_hidden_copied_bytes: frame.main_present_hidden_copied_bytes,
                main_present_hidden_invalid_bytes: frame.main_present_hidden_invalid_bytes,
                main_present_hidden_rect_count: frame.main_present_hidden_rect_count,
                main_present_hidden_catchup_bytes: frame.main_present_hidden_catchup_bytes,
                main_present_hidden_full_copy: frame.main_present_hidden_full_copy,
                main_present_copy_path: frame.main_present_copy_path,
                main_present_request_us: frame.main_present_request_us,
                main_present_set_vga_fb_us: frame.main_present_set_vga_fb_us,
                main_present_wait_us: frame.main_present_wait_us,
                main_present_sequence: frame.main_present_sequence,
                main_present_post_active_sequence: frame.main_present_post_active_sequence,
                main_present_post_pending_sequence: frame.main_present_post_pending_sequence,
                main_present_post_pending: frame.main_present_post_pending,
                main_present_flip_count: frame.main_present_flip_count,
                main_present_drop_count: frame.main_present_drop_count,
                main_present_receipt_crc: frame.main_present_receipt_crc,
            },
            status: LauncherFrameStatusData {
                status_write_due: frame.status_write_due,
                status_string_copy_bytes: frame.status_string_copy_bytes,
                clock_update_due: frame.clock_update_due,
                clock_update_us: frame.clock_update_us,
            },
            cpu: LauncherFrameCpuTrace {
                loop_start: frame.cpu_loop_start,
                t0: frame.cpu_t0,
                t1: frame.cpu_t1,
                t2: frame.cpu_t2,
                custom_draw_start: frame.cpu_custom_draw_start,
                custom_draw_done: frame.cpu_custom_draw_done,
                t3: frame.cpu_t3,
                t4: frame.cpu_t4,
            },
        }
    }

    #[test]
    fn frame_snapshot_builder_populates_existing_fields() {
        let start = Instant::now();
        let mut expected = presented_frame(42, start, 21_000);
        expected.main_present_receipt_crc = 0x5a3c;

        let built = builder_from_frame(&expected).build();

        assert_eq!(built.frames, expected.frames);
        assert_eq!(built.selected, expected.selected);
        assert_eq!(built.visual_index, expected.visual_index);
        assert_eq!(built.frame_t0, expected.frame_t0);
        assert_eq!(built.frame_t4, expected.frame_t4);
        assert_eq!(built.prepare_trace.catalog_message_count, 2);
        assert_eq!(built.copied_rows, 12);
        assert_eq!(built.present_bytes, 23_040);
        assert_eq!(built.vsync_source, Some(VsyncPaceSource::Timeout));
        assert_eq!(built.vsync_miss_streak, 3);
        assert_eq!(built.frame_start_phase_us, 8_000);
        assert_eq!(built.main_present_receipt_crc, 0x5a3c);
        assert_eq!(built.preview_cache_state, "exact");
        assert_eq!(built.status_string_copy_bytes, 128);
    }

    #[test]
    fn frame_snapshot_builder_preserves_hidden_present_attribution() {
        let start = Instant::now();
        let mut expected = presented_frame(42, start, 21_000);
        expected.hidden_compose_us = 730;
        expected.hidden_preview_compose_us = 230;
        expected.hidden_arcade_compose_us = 500;
        expected.direct_preview_present_us = 230;
        expected.arcade_list_present_us = 500;

        let built = builder_from_frame(&expected).build();

        assert_eq!(built.hidden_compose_us, 730);
        assert_eq!(built.hidden_preview_compose_us, 230);
        assert_eq!(built.hidden_arcade_compose_us, 500);
        assert_eq!(built.direct_preview_present_us, 230);
        assert_eq!(built.arcade_list_present_us, 500);
        assert_eq!(
            built.hidden_compose_us,
            built.hidden_preview_compose_us + built.hidden_arcade_compose_us
        );
    }

    #[test]
    fn frame_snapshot_builder_keeps_default_pacing_values_when_missing() {
        let start = Instant::now();
        let frame = presented_frame(43, start, 16_500);
        let mut builder = builder_from_frame(&frame);
        builder.pacing = LauncherPacingTrace {
            vsync_source: None,
            vsync_period_us: 20_000,
            vsync_miss_streak: 0,
            vsync_stale_hits: 0,
            vsync_wait_start_age_us: 0,
            vsync_accepted_hit_age_us: 0,
            frame_start_phase_us: 1_234,
            present_phase_us: 0,
        };

        let built = builder.build();

        assert_eq!(built.vsync_source, None);
        assert_eq!(built.vsync_period_us, 20_000);
        assert_eq!(built.vsync_miss_streak, 0);
        assert_eq!(built.vsync_stale_hits, 0);
        assert_eq!(built.vsync_wait_start_age_us, 0);
        assert_eq!(built.vsync_accepted_hit_age_us, 0);
        assert_eq!(built.frame_start_phase_us, 1_234);
        assert_eq!(built.present_phase_us, 0);
    }

    #[test]
    fn navigation_transition_defers_status_without_consuming_the_deadline() {
        let start = Instant::now();
        let mut frame = presented_frame(49, start, 8_000);
        frame.status_write_due = true;
        frame.composition_status.state = "navigation-transition";
        assert!(should_defer_runtime_status_write(&frame));
    }

    #[test]
    fn completed_latch_frames_preserve_pacing_and_maintenance_evidence() {
        let start = Instant::now();
        let mut accounting =
            LauncherFrameAccounting::new(start, "hdmi", "baseline", 960, 540, false);
        accounting.frame_analytics_mode = FrameAnalyticsMode::Process;
        let mut frame = presented_frame(49, start, 16_667);
        frame.screensaver_active = true;
        frame.main_present_backend = LauncherPresentBackend::FpgaVblankLatchHidden;
        frame.main_present_status = LauncherPresentStatus::Ok;
        frame.main_present_sequence = 65_535;
        frame.main_present_active_sequence = 65_535;
        frame.main_present_flip_count = 42;
        frame.vsync_source = Some(VsyncPaceSource::Vsync);
        frame.vsync_us_override = Some(4_000);
        frame.fb_present_us_override = Some(3_000);
        frame.status_write_due = true;
        frame.clock_update_due = true;
        frame.clock_update_us = 45;
        frame.screensaver_render_trace.raster_held_cards = 2;
        frame.screensaver_render_trace.raster_moved_cards = 8;
        frame.screensaver_render_trace.raster_hold_layer_mask = 1;
        frame.screensaver_render_trace.raster_visible_layer_mask = 3;
        frame.screensaver_render_trace.phase_bank_resident_bytes = 12_345;

        accounting.accumulate_frame_budget(&frame, 321);

        let status = accounting.current_frame_budget_status();
        let recent = status
            .recent_frames
            .first()
            .expect("completed frame sample");
        assert_eq!(recent.wall_us, 16_667);
        assert_eq!(recent.vsync_us, 4_000);
        assert_eq!(recent.present_us, 3_000);
        assert_eq!(recent.vsync_source, "vsync");
        assert_eq!(recent.main_present_sequence, 65_535);
        assert_eq!(recent.main_present_active_sequence, 65_535);
        assert!(!recent.main_present_pending);
        assert_eq!(recent.main_present_status, "ok");
        assert_eq!(recent.runtime_status_write_us, 321);
        assert_eq!(recent.clock_update_us, 45);
        assert_eq!(recent.screensaver_raster_held_cards, 2);
        assert_eq!(recent.screensaver_raster_hold_layer_mask, 1);
        assert_eq!(recent.screensaver_raster_visible_layer_mask, 3);
        assert_eq!(recent.screensaver_phase_bank_bytes, 12_345);
    }

    #[test]
    fn frame_budget_accumulator_counts_thresholds_and_phases() {
        let mut acc = FrameBudgetAccumulator::default();
        acc.record(sample(1, 16_000));
        acc.record(sample(2, 17_000));
        acc.record(sample(3, 21_000));
        acc.record(sample(4, 34_000));

        assert_eq!(acc.frames, 4);
        assert_eq!(acc.over_budget, 3);
        assert_eq!(acc.over_20ms, 2);
        assert_eq!(acc.over_33ms, 1);
        assert_eq!(acc.max_wall_us, 34_000);
        assert_eq!(acc.latest_over_budget_frame, 4);
        assert_eq!(acc.latest_over_budget_wall_us, 34_000);
        assert_eq!(
            FrameBudgetAccumulator::avg_us(acc.present_us, acc.frames),
            500
        );
    }

    #[test]
    fn frame_budget_accumulator_tracks_vsync_sources_and_miss_streak() {
        let mut acc = FrameBudgetAccumulator::default();
        for (idx, source) in [
            VsyncPaceSource::Vsync,
            VsyncPaceSource::Fallback,
            VsyncPaceSource::Timeout,
            VsyncPaceSource::Error,
        ]
        .into_iter()
        .enumerate()
        {
            let mut item = sample(idx as u64, 17_000);
            item.vsync_source = Some(source);
            item.vsync_miss_streak = idx as u32;
            acc.record(item);
        }

        assert_eq!(acc.vsync, 1);
        assert_eq!(acc.fallback, 1);
        assert_eq!(acc.timeout, 1);
        assert_eq!(acc.error, 1);
        assert_eq!(acc.max_vsync_miss_streak, 3);
    }

    #[test]
    fn slow_frame_samples_are_bounded_and_survive_recent_frame_clears() {
        let start = Instant::now();
        let mut accounting =
            LauncherFrameAccounting::new(start, "crt-576p50", "baseline", 640, 576, false);
        for frame in 0..40 {
            accounting.accumulate_frame_budget(
                &presented_frame(frame, start + Duration::from_micros(frame * 25_000), 22_000),
                0,
            );
        }

        let status = accounting.current_frame_budget_status();
        assert_eq!(status.slow_frames.len(), FRAME_SLOW_SAMPLE_CAP);
        assert_eq!(status.slow_frames[0].frame, 8);
        assert_eq!(status.slow_frames[31].frame, 39);
        assert_eq!(status.slow_frames[31].dominant_phase, "fb-present");
        assert_eq!(status.slow_frames[31].catalog_message_count, 2);
        assert_eq!(status.slow_frames[31].media_worker_us, 60);
        assert_eq!(status.slow_frames[31].preview_worker_drained, 5);
        assert_eq!(status.slow_frames[31].preview_cache_evictions, 2);
        assert_eq!(status.slow_frames[31].preview_backlog, 6);
        assert_eq!(status.slow_frames[31].dirty_y0, 12);
        assert_eq!(status.slow_frames[31].dirty_y1, 24);
        assert_eq!(status.slow_frames[31].vsync_wait_start_age_us, 12_000);
        assert_eq!(status.slow_frames[31].vsync_accepted_hit_age_us, 500);
        assert_eq!(status.slow_frames[31].frame_start_phase_us, 8_000);

        accounting.frame_analytics_samples.clear();
        let status_after_recent_clear = accounting.current_frame_budget_status();
        assert!(status_after_recent_clear.recent_frames.is_empty());
        assert_eq!(
            status_after_recent_clear.slow_frames.len(),
            FRAME_SLOW_SAMPLE_CAP
        );
        assert_eq!(status_after_recent_clear.slow_frames[0].frame, 8);
    }

    #[test]
    fn cadence_warning_samples_are_retained_before_budget_overrun() {
        let start = Instant::now();
        let mut accounting =
            LauncherFrameAccounting::new(start, "crt-576p50", "baseline", 640, 576, false);
        accounting.accumulate_frame_budget(&presented_frame(7, start, FRAME_CADENCE_WARNING_US), 0);

        let status = accounting.current_frame_budget_status();
        assert_eq!(status.slow_frames.len(), 1);
        assert_eq!(status.slow_frames[0].frame, 7);
        assert_eq!(status.slow_frames[0].severity, "cadence-warning");
        assert_eq!(status.slow_frames[0].warning_us, FRAME_CADENCE_WARNING_US);
        assert_eq!(status.slow_frames[0].over_budget_us, 0);
    }
}
