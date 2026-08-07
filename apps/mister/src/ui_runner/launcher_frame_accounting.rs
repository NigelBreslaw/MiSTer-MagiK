// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::launcher_compositor::{
    LauncherPresentBackend, LauncherPresentResult, LauncherPresentStatus,
};
use super::launcher_loop::LaunchReturnSession;
use super::launcher_pacing::LauncherPacingTrace;
use super::launcher_screensaver::ScreensaverFrameTrace;
use super::*;
use mister_magik_fb::latch_readiness::LatchFailure;
#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
use std::fmt::Write as _;
#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
use std::io::{BufWriter, Write as _};

const FRAME_BUDGET_US: u64 = 16_667;
const FRAME_CADENCE_WARNING_US: u64 = 16_000;
const FRAME_BUDGET_20MS_US: u64 = 20_000;
const FRAME_BUDGET_33MS_US: u64 = 33_334;
const FRAME_ANALYTICS_LEASE_PATH: &str = "/tmp/mister-magik/realtime-frame-analytics";
const FRAME_ANALYTICS_LEASE_MAX_AGE: Duration = Duration::from_secs(3);
const FRAME_ANALYTICS_SAMPLE_CAP: usize = 75;
const FRAME_SLOW_SAMPLE_CAP: usize = 32;
const AUTOMATION_STATE_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
const PREVIEW_SCROLL_TRACE_FLUSH_ROWS: usize = 60;

pub(super) struct LauncherFrameAccounting {
    output_route: &'static str,
    framebuffer_width: usize,
    framebuffer_height: usize,
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
    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    preview_scroll_trace: Option<PreviewScrollTrace>,
    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    preview_scroll_trace_duration: Option<Duration>,
    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    last_preview_trace_loop_start: Option<Instant>,
    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    last_preview_trace_frame_t4: Option<Instant>,
    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    last_preview_trace_finish_done: Option<Instant>,
    #[cfg(any(feature = "bench-tools", feature = "diagnostics", feature = "profile"))]
    boot_frame_profile: Option<boot_analytics::LauncherFrameWriter>,
    runtime_status_publisher: runtime_status::RuntimeStatusPublisher,
    last_status_write: Instant,
    status_sequence: u64,
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
    automation_state_hash: u64,
    automation_state_revision: u64,
    automation_presented_state_revision: u64,
    automation_action_sequence: u64,
    automation_presented_action_sequence: u64,
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
    pub(super) automation: AutomationFrameStamp,
    pub(super) selected: usize,
    pub(super) visual_index: f32,
    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    pub(super) home_trace: LauncherHomeFrameTrace,
    pub(super) search_index_state: &'static str,
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
    pub(super) main_present_hidden_invalid_bytes: usize,
    pub(super) main_present_hidden_rect_count: u32,
    pub(super) main_present_hidden_catchup_bytes: usize,
    pub(super) main_present_hidden_full_copy: bool,
    pub(super) main_present_copy_path: &'static str,
    pub(super) main_present_request_us: u128,
    pub(super) main_present_set_vga_fb_us: u128,
    pub(super) main_present_wait_us: u64,
    pub(super) main_present_sequence: u16,
    pub(super) main_present_active_sequence: u16,
    pub(super) main_present_pending: bool,
    pub(super) main_present_completion_poll_count: u16,
    pub(super) main_present_completion_poll_wall_us: u64,
    pub(super) main_present_completion_poll_cpu_us: u64,
    pub(super) main_present_flip_count: u16,
    pub(super) main_present_drop_count: u16,
    pub(super) vsync_source: Option<VsyncPaceSource>,
    pub(super) vsync_period_us: u64,
    pub(super) vsync_miss_streak: u32,
    pub(super) vsync_stale_hits: u32,
    pub(super) vsync_wait_start_age_us: u64,
    pub(super) vsync_accepted_hit_age_us: u64,
    pub(super) frame_start_phase_us: u64,
    pub(super) present_phase_us: u128,
    pub(super) home_pan_present_active: bool,
    pub(super) home_horizontal_input_held: bool,
    pub(super) redraw_pending: bool,
    pub(super) wake_reasons_bits: u64,
    pub(super) arcade_update_label: ArcadeUpdateTrace,
    pub(super) preview_cache_state: &'static str,
    pub(super) preview_transition: PreviewTransitionTrace,
    pub(super) composition_status: UiCompositionStatus,
    pub(super) screensaver_active: bool,
    pub(super) screensaver_active_cards: usize,
    pub(super) screensaver_archive_loading: bool,
    pub(super) screensaver_frame_trace: ScreensaverFrameTrace,
    pub(super) status_write_due: bool,
    pub(super) status_string_copy_us: u128,
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
    pub(super) automation: AutomationFrameStamp,
    pub(super) selected: usize,
    pub(super) visual_index: f32,
    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    pub(super) home_trace: LauncherHomeFrameTrace,
    pub(super) search_index_state: &'static str,
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct LauncherHomeFrameTrace {
    pub(super) screen: &'static str,
    pub(super) menu_token: u64,
    pub(super) selected_token: u64,
    pub(super) selected_index: usize,
    pub(super) scroll_x: i32,
    pub(super) scroll_max: i32,
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
impl LauncherHomeFrameTrace {
    pub(super) fn from_nav(nav: &LauncherNav) -> Self {
        Self {
            screen: screen_label(nav.screen),
            menu_token: stable_trace_token(nav.current_menu_id()),
            selected_token: stable_trace_token(nav.current_menu_selected_item_id()),
            selected_index: nav.selected,
            scroll_x: nav.scroll_x,
            scroll_max: nav.home_scroll_max(),
        }
    }
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
fn stable_trace_token(value: &str) -> u64 {
    // FNV-1a gives trace consumers a stable identity without cloning taxonomy
    // strings into every buffered frame row.
    if value.is_empty() {
        return 0;
    }
    value
        .as_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
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
    pub(super) home_pan_present_active: bool,
    pub(super) home_horizontal_input_held: bool,
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
    pub(super) screensaver_archive_loading: bool,
    pub(super) screensaver_frame_trace: ScreensaverFrameTrace,
}

pub(super) struct LauncherFrameStatusData {
    pub(super) status_write_due: bool,
    pub(super) status_string_copy_us: u128,
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
    runtime_status_write_deferred: bool,
    frame_finish_us: u128,
}

impl LauncherFrameSnapshotBuilder {
    pub(super) fn build(self) -> LauncherPresentedFrame {
        LauncherPresentedFrame {
            frames: self.identity.frames,
            automation: self.identity.automation,
            selected: self.identity.selected,
            visual_index: self.identity.visual_index,
            #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
            home_trace: self.identity.home_trace,
            search_index_state: self.identity.search_index_state,
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
            main_present_hidden_invalid_bytes: self.presentation.main_present_hidden_invalid_bytes,
            main_present_hidden_rect_count: self.presentation.main_present_hidden_rect_count,
            main_present_hidden_catchup_bytes: self.presentation.main_present_hidden_catchup_bytes,
            main_present_hidden_full_copy: self.presentation.main_present_hidden_full_copy,
            main_present_copy_path: self.presentation.main_present_copy_path,
            main_present_request_us: self.presentation.main_present_request_us,
            main_present_set_vga_fb_us: self.presentation.main_present_set_vga_fb_us,
            main_present_wait_us: self.presentation.main_present_wait_us,
            main_present_sequence: self.presentation.main_present_sequence,
            main_present_active_sequence: self.presentation.main_present_sequence,
            main_present_pending: false,
            main_present_completion_poll_count: 0,
            main_present_completion_poll_wall_us: 0,
            main_present_completion_poll_cpu_us: 0,
            main_present_flip_count: self.presentation.main_present_flip_count,
            main_present_drop_count: self.presentation.main_present_drop_count,
            vsync_source: self.pacing.vsync_source,
            vsync_period_us: self.pacing.vsync_period_us,
            vsync_miss_streak: self.pacing.vsync_miss_streak,
            vsync_stale_hits: self.pacing.vsync_stale_hits,
            vsync_wait_start_age_us: self.pacing.vsync_wait_start_age_us,
            vsync_accepted_hit_age_us: self.pacing.vsync_accepted_hit_age_us,
            frame_start_phase_us: self.pacing.frame_start_phase_us,
            present_phase_us: self.pacing.present_phase_us,
            home_pan_present_active: self.timing.home_pan_present_active,
            home_horizontal_input_held: self.timing.home_horizontal_input_held,
            redraw_pending: self.timing.redraw_pending,
            wake_reasons_bits: self.timing.wake_reasons_bits,
            arcade_update_label: self.presentation.arcade_update_label,
            preview_cache_state: self.render.preview_cache_state,
            preview_transition: self.render.preview_transition,
            composition_status: self.render.composition_status,
            screensaver_active: self.render.screensaver_active,
            screensaver_active_cards: self.render.screensaver_active_cards,
            screensaver_archive_loading: self.render.screensaver_archive_loading,
            screensaver_frame_trace: self.render.screensaver_frame_trace,
            status_write_due: self.status.status_write_due,
            status_string_copy_us: self.status.status_string_copy_us,
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

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
struct PreviewScrollTrace {
    writer: BufWriter<std::fs::File>,
    rows: Vec<PreviewScrollTraceRow>,
    row_text: String,
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
struct PreviewScrollTraceRow {
    frame: u64,
    elapsed_us: u128,
    loop_delta_us: u128,
    selected: usize,
    visual_index: f32,
    home_screen: &'static str,
    home_menu_token: u64,
    home_selected_token: u64,
    home_selected_index: usize,
    home_scroll_x: i32,
    home_scroll_max: i32,
    cache_state: &'static str,
    transition_effect: &'static str,
    transition_progress: f32,
    arcade_update: ArcadeUpdateTrace,
    copied_rows: u32,
    direct_preview_rows: u32,
    present_bytes: usize,
    wasted_present_bytes: usize,
    prepare_us: u128,
    catalog_worker_us: u128,
    catalog_message_count: u32,
    catalog_backlog: u32,
    catalog_ready_deferred: u8,
    catalog_ready_deferred_age_us: u128,
    media_worker_us: u128,
    media_gate_us: u128,
    preview_schedule_us: u128,
    preview_apply_us: u128,
    slint_render_us: u128,
    custom_draw_us: u128,
    arcade_list_update_us: u128,
    preview_blit_us: u128,
    preview_fade_wall_us: u64,
    preview_fade_cpu_us: u64,
    preview_fade_pixels: u32,
    preview_fade_rows: u32,
    preview_fade_path: &'static str,
    preview_fade_alpha_bucket: u8,
    effect_label_us: u128,
    pre_render_wait_us: u128,
    post_present_wait_us: u128,
    post_frame_tail_us: u128,
    frame_finish_us: u128,
    post_finish_tail_us: u128,
    vsync_us: u128,
    fb_present_us: u128,
    cached_present_us: u128,
    hidden_compose_us: u128,
    hidden_preview_compose_us: u128,
    hidden_arcade_compose_us: u128,
    direct_preview_present_us: u128,
    arcade_list_present_us: u128,
    main_present_backend: &'static str,
    main_present_status: &'static str,
    main_present_buffer: u8,
    main_present_hidden_copy_us: u128,
    main_present_hidden_invalid_bytes: usize,
    main_present_hidden_rect_count: u32,
    main_present_hidden_catchup_bytes: usize,
    main_present_hidden_full_copy: bool,
    main_present_request_us: u128,
    main_present_set_vga_fb_us: u128,
    main_present_wait_us: u64,
    main_present_sequence: u16,
    main_present_flip_count: u16,
    main_present_drop_count: u16,
    vsync_source: &'static str,
    vsync_period_us: u64,
    vsync_miss_streak: u32,
    vsync_stale_hits: u32,
    vsync_wait_start_age_us: u64,
    vsync_accepted_hit_age_us: u64,
    frame_start_phase_us: u64,
    present_phase_us: u128,
    home_pan_present_active: u8,
    home_horizontal_input_held: u8,
    redraw_pending: u8,
    wake_reasons_bits: u64,
    dirty_y0: usize,
    dirty_y1: usize,
    status_write_due: u8,
    runtime_status_write_deferred: u8,
    frame_tail_slack_us: u128,
    status_string_copy_us: u128,
    status_string_copy_bytes: usize,
    runtime_status_write_us: u128,
    status_write_duration_us: u128,
    wall_us: u128,
    search_index_state: &'static str,
    startup_elapsed_us: u128,
    monotonic_us: u128,
    screensaver_active: u8,
    screensaver_active_cards: usize,
    screensaver_archive_loading: u8,
    screensaver_archive_poll_us: u128,
    screensaver_card_adopt_us: u128,
    screensaver_cards_adopted: usize,
    screensaver_parade_advance_us: u128,
    screensaver_background_us: u128,
    screensaver_draw_order_us: u128,
    screensaver_tile_blit_us: u128,
    screensaver_cards_drawn: usize,
    screensaver_cards_culled: usize,
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
impl PreviewScrollTrace {
    fn new(writer: BufWriter<std::fs::File>) -> Self {
        Self {
            writer,
            rows: Vec::with_capacity(PREVIEW_SCROLL_TRACE_FLUSH_ROWS),
            row_text: String::with_capacity(384),
        }
    }

    fn push(&mut self, row: PreviewScrollTraceRow, allow_flush: bool) {
        self.rows.push(row);
        if allow_flush && self.rows.len() >= PREVIEW_SCROLL_TRACE_FLUSH_ROWS {
            self.flush_rows();
        }
    }

    fn flush_rows(&mut self) {
        let rows = std::mem::take(&mut self.rows);
        for row in rows {
            self.row_text.clear();
            row.write_tsv(&mut self.row_text);
            let _ = self.writer.write_all(self.row_text.as_bytes());
        }
        let _ = self.writer.flush();
    }
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
impl PreviewScrollTraceRow {
    fn write_tsv(&self, out: &mut String) {
        let _ = write!(
            out,
            "{}\t{}\t{}\t{}\t{:.6}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.3}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            self.frame,
            self.elapsed_us,
            self.loop_delta_us,
            self.selected,
            self.visual_index,
            self.home_screen,
            self.home_menu_token,
            self.home_selected_token,
            self.home_selected_index,
            self.home_scroll_x,
            self.home_scroll_max,
            self.cache_state,
            self.transition_effect,
            self.transition_progress,
            self.arcade_update,
            self.copied_rows,
            self.direct_preview_rows,
            self.present_bytes,
            self.wasted_present_bytes,
            self.prepare_us,
            self.catalog_worker_us,
            self.catalog_message_count,
            self.catalog_backlog,
            self.catalog_ready_deferred,
            self.catalog_ready_deferred_age_us,
            self.media_worker_us,
            self.media_gate_us,
            self.preview_schedule_us,
            self.preview_apply_us,
            self.slint_render_us,
            self.custom_draw_us,
            self.arcade_list_update_us,
            self.preview_blit_us,
            self.preview_fade_wall_us,
            self.preview_fade_cpu_us,
            self.preview_fade_pixels,
            self.preview_fade_rows,
            self.preview_fade_path,
            self.preview_fade_alpha_bucket,
            self.effect_label_us,
            self.pre_render_wait_us,
            self.post_present_wait_us,
            self.post_frame_tail_us,
            self.vsync_us,
            self.fb_present_us,
            self.cached_present_us,
            self.hidden_compose_us,
            self.hidden_preview_compose_us,
            self.hidden_arcade_compose_us,
            self.direct_preview_present_us,
            self.arcade_list_present_us,
            self.main_present_backend,
            self.main_present_status,
            self.main_present_buffer,
            self.main_present_hidden_copy_us,
            self.main_present_hidden_invalid_bytes,
            self.main_present_hidden_rect_count,
            self.main_present_hidden_catchup_bytes,
            u8::from(self.main_present_hidden_full_copy),
            self.main_present_request_us,
            self.main_present_set_vga_fb_us,
            self.main_present_wait_us,
            self.main_present_sequence,
            self.main_present_flip_count,
            self.main_present_drop_count,
            self.vsync_source,
            self.vsync_period_us,
            self.vsync_miss_streak,
            self.vsync_stale_hits,
            self.vsync_wait_start_age_us,
            self.vsync_accepted_hit_age_us,
            self.frame_start_phase_us,
            self.present_phase_us,
            self.home_pan_present_active,
            self.home_horizontal_input_held,
            self.redraw_pending,
            self.wake_reasons_bits,
            self.dirty_y0,
            self.dirty_y1,
            self.status_write_due,
            self.runtime_status_write_deferred,
            self.frame_tail_slack_us,
            self.status_string_copy_us,
            self.status_string_copy_bytes,
            self.runtime_status_write_us,
            self.status_write_duration_us,
            self.wall_us
        );
        out.pop();
        let _ = writeln!(
            out,
            "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.frame_finish_us,
            self.post_finish_tail_us,
            self.screensaver_active,
            self.screensaver_active_cards,
            self.screensaver_archive_loading,
            self.screensaver_archive_poll_us,
            self.screensaver_card_adopt_us,
            self.screensaver_cards_adopted,
            self.screensaver_parade_advance_us,
            self.screensaver_background_us,
            self.screensaver_draw_order_us,
            self.screensaver_tile_blit_us,
            self.screensaver_cards_drawn,
            self.screensaver_cards_culled,
            self.search_index_state,
            self.startup_elapsed_us,
            self.monotonic_us
        );
    }
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
fn preview_scroll_trace_row_from_frame(
    frame: &LauncherPresentedFrame,
    loop_delta_us: u128,
    post_frame_tail_us: u128,
    runtime_status_write_us: u128,
    runtime_status_write_deferred: bool,
    frame_finish_us: u128,
    post_finish_tail_us: u128,
) -> PreviewScrollTraceRow {
    let wall_us = (frame.frame_t4 - frame.loop_start).as_micros();
    let frame_tail_slack_us = u128::from(frame.vsync_period_us).saturating_sub(wall_us);
    PreviewScrollTraceRow {
        frame: frame.frames,
        elapsed_us: frame.loop_start.duration_since(frame.run_start).as_micros(),
        loop_delta_us,
        selected: frame.selected,
        visual_index: frame.visual_index,
        home_screen: frame.home_trace.screen,
        home_menu_token: frame.home_trace.menu_token,
        home_selected_token: frame.home_trace.selected_token,
        home_selected_index: frame.home_trace.selected_index,
        home_scroll_x: frame.home_trace.scroll_x,
        home_scroll_max: frame.home_trace.scroll_max,
        cache_state: frame.preview_cache_state,
        transition_effect: frame.preview_transition.effect.label(),
        transition_progress: frame.preview_transition.progress,
        arcade_update: frame.arcade_update_label,
        copied_rows: frame.copied_rows,
        direct_preview_rows: frame.direct_preview_rows,
        present_bytes: frame.present_bytes,
        wasted_present_bytes: frame.wasted_present_bytes,
        prepare_us: frame.prepare_us,
        catalog_worker_us: frame.prepare_trace.catalog_worker_us,
        catalog_message_count: frame.prepare_trace.catalog_message_count,
        catalog_backlog: frame.prepare_trace.catalog_backlog,
        catalog_ready_deferred: u8::from(frame.prepare_trace.catalog_ready_deferred),
        catalog_ready_deferred_age_us: frame.prepare_trace.catalog_ready_deferred_age_us,
        media_worker_us: frame.prepare_trace.media_worker_us,
        media_gate_us: frame.prepare_trace.media_gate_us,
        preview_schedule_us: frame.prepare_trace.preview_schedule_us,
        preview_apply_us: frame.prepare_trace.preview_apply_us,
        slint_render_us: (frame.frame_t2 - frame.frame_t1).as_micros(),
        custom_draw_us: (frame.custom_draw_done - frame.custom_draw_start).as_micros(),
        arcade_list_update_us: frame.custom_draw_trace.arcade_list_update_us,
        preview_blit_us: frame.custom_draw_trace.preview_blit_us,
        preview_fade_wall_us: frame.preview_transition.fade.wall_us,
        preview_fade_cpu_us: frame.preview_transition.fade.cpu_us,
        preview_fade_pixels: frame.preview_transition.fade.pixels,
        preview_fade_rows: frame.preview_transition.fade.rows,
        preview_fade_path: frame.preview_transition.fade.label(),
        preview_fade_alpha_bucket: frame.preview_transition.fade.alpha_bucket,
        effect_label_us: frame.custom_draw_trace.effect_label_us,
        pre_render_wait_us: frame.pre_render_wait_us,
        post_present_wait_us: frame.post_present_wait_us,
        post_frame_tail_us,
        frame_finish_us,
        post_finish_tail_us,
        vsync_us: frame
            .vsync_us_override
            .unwrap_or_else(|| (frame.frame_t3 - frame.custom_draw_done).as_micros()),
        fb_present_us: frame
            .fb_present_us_override
            .unwrap_or_else(|| (frame.frame_t4 - frame.frame_t3).as_micros()),
        cached_present_us: frame.cached_present_us,
        hidden_compose_us: frame.hidden_compose_us,
        hidden_preview_compose_us: frame.hidden_preview_compose_us,
        hidden_arcade_compose_us: frame.hidden_arcade_compose_us,
        direct_preview_present_us: frame.direct_preview_present_us,
        arcade_list_present_us: frame.arcade_list_present_us,
        main_present_backend: frame.main_present_backend.trace_label(),
        main_present_status: frame.main_present_status.trace_label(),
        main_present_buffer: frame.main_present_buffer,
        main_present_hidden_copy_us: frame.main_present_hidden_copy_us,
        main_present_hidden_invalid_bytes: frame.main_present_hidden_invalid_bytes,
        main_present_hidden_rect_count: frame.main_present_hidden_rect_count,
        main_present_hidden_catchup_bytes: frame.main_present_hidden_catchup_bytes,
        main_present_hidden_full_copy: frame.main_present_hidden_full_copy,
        main_present_request_us: frame.main_present_request_us,
        main_present_set_vga_fb_us: frame.main_present_set_vga_fb_us,
        main_present_wait_us: frame.main_present_wait_us,
        main_present_sequence: frame.main_present_sequence,
        main_present_flip_count: frame.main_present_flip_count,
        main_present_drop_count: frame.main_present_drop_count,
        vsync_source: frame
            .vsync_source
            .map(VsyncPaceSource::label)
            .unwrap_or("none"),
        vsync_period_us: frame.vsync_period_us,
        vsync_miss_streak: frame.vsync_miss_streak,
        vsync_stale_hits: frame.vsync_stale_hits,
        vsync_wait_start_age_us: frame.vsync_wait_start_age_us,
        vsync_accepted_hit_age_us: frame.vsync_accepted_hit_age_us,
        frame_start_phase_us: frame.frame_start_phase_us,
        present_phase_us: frame.present_phase_us,
        home_pan_present_active: u8::from(frame.home_pan_present_active),
        home_horizontal_input_held: u8::from(frame.home_horizontal_input_held),
        redraw_pending: u8::from(frame.redraw_pending),
        wake_reasons_bits: frame.wake_reasons_bits,
        dirty_y0: frame.dirty_rect.map(|rect| rect.y0).unwrap_or(0),
        dirty_y1: frame.dirty_rect.map(|rect| rect.y1).unwrap_or(0),
        status_write_due: u8::from(frame.status_write_due),
        runtime_status_write_deferred: u8::from(runtime_status_write_deferred),
        frame_tail_slack_us,
        status_string_copy_us: frame.prepare_trace.status_string_copy_us,
        status_string_copy_bytes: frame.status_string_copy_bytes,
        runtime_status_write_us,
        status_write_duration_us: runtime_status_write_us,
        wall_us,
        search_index_state: frame.search_index_state,
        startup_elapsed_us: frame
            .loop_start
            .duration_since(frame.startup_start)
            .as_micros(),
        monotonic_us: u128::from(frame.startup_monotonic_us).saturating_add(
            frame
                .loop_start
                .duration_since(frame.startup_start)
                .as_micros(),
        ),
        screensaver_active: u8::from(frame.screensaver_active),
        screensaver_active_cards: frame.screensaver_active_cards,
        screensaver_archive_loading: u8::from(frame.screensaver_archive_loading),
        screensaver_archive_poll_us: frame.screensaver_frame_trace.archive_poll_us,
        screensaver_card_adopt_us: frame.screensaver_frame_trace.card_adopt_us,
        screensaver_cards_adopted: frame.screensaver_frame_trace.cards_adopted,
        screensaver_parade_advance_us: frame.screensaver_frame_trace.parade_advance_us,
        screensaver_background_us: frame.screensaver_frame_trace.background_us,
        screensaver_draw_order_us: frame.screensaver_frame_trace.draw_order_us,
        screensaver_tile_blit_us: frame.screensaver_frame_trace.tile_blit_us,
        screensaver_cards_drawn: frame.screensaver_frame_trace.cards_drawn,
        screensaver_cards_culled: frame.screensaver_frame_trace.cards_culled,
    }
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
impl Drop for PreviewScrollTrace {
    fn drop(&mut self) {
        self.flush_rows();
    }
}

impl LauncherFrameAccounting {
    pub(super) fn close_preview_scroll_trace_for_restart(&mut self) {
        #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
        self.close_preview_scroll_trace();
    }

    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    pub(super) fn finish_preview_scroll_trace(&mut self) {
        if let Some(trace) = self.preview_scroll_trace.as_mut() {
            trace.flush_rows();
        }
        if let Ok(path) = std::env::var("MISTER_PREVIEW_SCROLL_TRACE_COMPLETE") {
            let _ = std::fs::File::create(path);
        }
    }

    #[cfg(not(any(feature = "bench-tools", feature = "diagnostics")))]
    pub(super) fn finish_preview_scroll_trace(&mut self) {}
}

#[derive(Clone, Copy, Default)]
pub(super) struct LauncherCustomDrawTrace {
    pub(super) arcade_list_update_us: u128,
    pub(super) preview_blit_us: u128,
    pub(super) effect_label_us: u128,
    pub(super) navigation_transition_overlay_us: u128,
    pub(super) navigation_transition_edge: &'static str,
    pub(super) navigation_transition_direction: &'static str,
    pub(super) navigation_snapshot_locked: bool,
    pub(super) navigation_slint_render_called: bool,
    pub(super) navigation_status_quiesce_wait_us: u64,
    pub(super) navigation_status_quiesce_timeout: bool,
    pub(super) orientation_transition_active: bool,
    pub(super) orientation_transition_leg: u8,
    pub(super) orientation_transition_from: &'static str,
    pub(super) orientation_transition_to: &'static str,
    pub(super) orientation_transition_destination_capture_us: u128,
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
pub(super) enum ArcadeUpdateTrace {
    None,
    Full,
    Scroll { delta_y: isize },
}

impl ArcadeUpdateTrace {
    pub(super) fn from_update(update: Option<&ArcadeListUpdate>) -> Self {
        match update {
            Some(ArcadeListUpdate::Full(_)) => Self::Full,
            Some(ArcadeListUpdate::Scroll { delta_y, .. }) => Self::Scroll { delta_y: *delta_y },
            None => Self::None,
        }
    }
}

impl std::fmt::Display for ArcadeUpdateTrace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Full => f.write_str("full"),
            Self::Scroll { delta_y } => write!(f, "scroll:{delta_y}"),
        }
    }
}

impl LauncherFrameAccounting {
    pub(super) fn new(
        run_start: Instant,
        output_route: &'static str,
        framebuffer_width: usize,
        framebuffer_height: usize,
    ) -> Self {
        Self {
            output_route,
            framebuffer_width,
            framebuffer_height,
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
            #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
            preview_scroll_trace: open_preview_scroll_trace(),
            #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
            preview_scroll_trace_duration: preview_scroll_trace_duration_from_env(),
            #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
            last_preview_trace_loop_start: None,
            #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
            last_preview_trace_frame_t4: None,
            #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
            last_preview_trace_finish_done: None,
            #[cfg(any(feature = "bench-tools", feature = "diagnostics", feature = "profile"))]
            boot_frame_profile: boot_analytics::LauncherFrameWriter::from_env(),
            runtime_status_publisher: runtime_status::RuntimeStatusPublisher::new(),
            last_status_write: Instant::now() - Duration::from_secs(2),
            status_sequence: 0,
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
            automation_state_hash: 0,
            automation_state_revision: 0,
            automation_presented_state_revision: 0,
            automation_action_sequence: 0,
            automation_presented_action_sequence: 0,
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

    pub(super) fn set_automation_action_sequence(&mut self, sequence: u64) {
        self.automation_action_sequence = sequence;
    }

    #[allow(clippy::too_many_arguments)]
    fn observe_automation_state(
        &mut self,
        nav: &LauncherNav,
        catalog: &ArcadeCatalog,
        confirm_visible: bool,
        confirm_title: &str,
        confirm_message: &str,
        launching: bool,
        loading_title: &str,
        preview_cache_state: &str,
        composition_status: &UiCompositionStatus,
    ) {
        let selected_system_id = nav.active_collection_scope_id(catalog);
        let selected_game = (nav.screen == Screen::Arcade)
            .then(|| nav.active_arcade_game_at(catalog, selected_system_id, nav.arcade.selected))
            .flatten();
        let mut hash = AUTOMATION_STATE_HASH_OFFSET;
        for value in [
            self.effective_view,
            screen_label(nav.screen),
            nav.current_menu_id(),
            nav.current_menu_selected_item_id(),
            nav.active_collection_id().unwrap_or(""),
            selected_system_id,
            selected_game.map_or("", |game| game.mra_path.as_ref()),
            selected_game.map_or("", |game| game.title.as_ref()),
            selected_game.map_or("", |game| game.preview_asset_key.as_ref()),
            confirm_title,
            confirm_message,
            loading_title,
            preview_cache_state,
            composition_status.state,
            &self.catalog_generation,
        ] {
            hash = automation_hash_bytes(hash, value.as_bytes());
            hash = automation_hash_bytes(hash, &[0xff]);
        }
        for value in [
            nav.selected as u64,
            nav.arcade.selected as u64,
            nav.arcade_filter.selected as u64,
            nav.settings_selected as u64,
            nav.display_selected as u64,
            nav.screensaver_selected as u64,
            u64::from(nav.settings_focused),
            u64::from(nav.arcade_filter.drawer_open),
            u64::from(nav.arcade_search.is_active(&nav.arcade_filter.active)),
            u64::from(confirm_visible),
            u64::from(launching),
        ] {
            hash = automation_hash_bytes(hash, &value.to_le_bytes());
        }
        if self.automation_state_revision == 0 || hash != self.automation_state_hash {
            self.automation_state_hash = hash;
            self.automation_state_revision = self.automation_state_revision.saturating_add(1);
        }
    }

    pub(super) fn preview_scroll_trace_enabled(&self) -> bool {
        #[cfg(not(any(feature = "bench-tools", feature = "diagnostics")))]
        {
            false
        }
        #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
        {
            self.preview_scroll_trace.is_some()
        }
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

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_frame(
        &mut self,
        frame: LauncherPresentedFrame,
        start: Instant,
        disp: &mut MappedRgb565Framebuffer,
        nav: &LauncherNav,
        pad: &PadPool,
        catalog: &ArcadeCatalog,
        catalog_ready: bool,
        catalog_refresh_done: bool,
        launching: bool,
        loading_title: &str,
        catalog_scan_visible: bool,
        catalog_scan_title: &str,
        catalog_scan_detail: &str,
        catalog_scan_percent: i32,
        catalog_background_scan_visible: bool,
        catalog_scan_message: &str,
        confirm_visible: bool,
        confirm_title: &str,
        confirm_message: &str,
        confirm_selected: i32,
        confirm_left_label: &str,
        confirm_right_label: &str,
        launcher_bench_scenario: Option<LauncherBenchScenario>,
        start_screen: Screen,
        lock_screen: Option<Screen>,
        route_reassert_count: u64,
        last_route_reassert_frame: u64,
        last_route_reassert_ok: bool,
        last_route_reassert_error: &str,
        startup_status: StartupRevealStatus,
        return_session: &LaunchReturnSession,
        #[cfg_attr(
            not(any(feature = "bench-tools", feature = "diagnostics")),
            allow(unused_variables)
        )]
        defer_preview_trace_flush: bool,
    ) {
        let timing = self.finish_frame_before_trace(
            &frame,
            nav,
            pad,
            catalog,
            catalog_ready,
            catalog_refresh_done,
            launching,
            loading_title,
            catalog_scan_visible,
            catalog_scan_title,
            catalog_scan_detail,
            catalog_scan_percent,
            catalog_background_scan_visible,
            catalog_scan_message,
            confirm_visible,
            confirm_title,
            confirm_message,
            confirm_selected,
            confirm_left_label,
            confirm_right_label,
            launcher_bench_scenario,
            start_screen,
            lock_screen,
            route_reassert_count,
            last_route_reassert_frame,
            last_route_reassert_ok,
            last_route_reassert_error,
            startup_status,
            return_session,
        );
        self.record_finished_frame(
            &frame,
            start,
            disp,
            catalog_ready,
            timing.runtime_status_write_us,
        );
        self.write_finished_frame_trace(&frame, timing, defer_preview_trace_flush);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_frame_before_trace(
        &mut self,
        frame: &LauncherPresentedFrame,
        nav: &LauncherNav,
        pad: &PadPool,
        catalog: &ArcadeCatalog,
        catalog_ready: bool,
        catalog_refresh_done: bool,
        launching: bool,
        loading_title: &str,
        catalog_scan_visible: bool,
        catalog_scan_title: &str,
        catalog_scan_detail: &str,
        catalog_scan_percent: i32,
        catalog_background_scan_visible: bool,
        catalog_scan_message: &str,
        confirm_visible: bool,
        confirm_title: &str,
        confirm_message: &str,
        confirm_selected: i32,
        confirm_left_label: &str,
        confirm_right_label: &str,
        launcher_bench_scenario: Option<LauncherBenchScenario>,
        start_screen: Screen,
        lock_screen: Option<Screen>,
        route_reassert_count: u64,
        last_route_reassert_frame: u64,
        last_route_reassert_ok: bool,
        last_route_reassert_error: &str,
        startup_status: StartupRevealStatus,
        return_session: &LaunchReturnSession,
    ) -> LauncherFrameFinishTraceTiming {
        let frame_finish_start = Instant::now();
        self.observe_automation_state(
            nav,
            catalog,
            confirm_visible,
            confirm_title,
            confirm_message,
            launching,
            loading_title,
            frame.preview_cache_state,
            &frame.composition_status,
        );
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
            nav,
            pad,
            catalog,
            catalog_ready,
            catalog_refresh_done,
            launching,
            loading_title,
            catalog_scan_visible,
            catalog_scan_title,
            catalog_scan_detail,
            catalog_scan_percent,
            catalog_background_scan_visible,
            catalog_scan_message,
            confirm_visible,
            confirm_title,
            confirm_message,
            confirm_selected,
            confirm_left_label,
            confirm_right_label,
            frame.selected,
            frame.visual_index,
            frame.preview_cache_state,
            frame.preview_transition.effect.label(),
            frame.preview_transition.progress,
            frame.screensaver_active_cards,
            &frame.composition_status,
            launcher_bench_scenario,
            start_screen,
            lock_screen,
            route_reassert_count,
            last_route_reassert_frame,
            last_route_reassert_ok,
            last_route_reassert_error,
            startup_status,
            return_session,
            None,
        );
        let runtime_status_write_us = runtime_status_write_start
            .map(|start| start.elapsed().as_micros())
            .unwrap_or(0);
        let frame_finish_us = frame_finish_start.elapsed().as_micros();
        LauncherFrameFinishTraceTiming {
            runtime_status_write_us,
            runtime_status_write_deferred,
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
        if launcher_frame_was_presented(frame) {
            self.automation_presented_state_revision = self.automation_state_revision;
            self.automation_presented_action_sequence = self.automation_action_sequence;
        }
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
        #[cfg(any(feature = "bench-tools", feature = "diagnostics", feature = "profile"))]
        self.record_boot_frame_profile(frame, disp);
        self.record_first_frame(frame, start, catalog_ready);
    }

    pub(super) fn write_finished_frame_trace(
        &mut self,
        frame: &LauncherPresentedFrame,
        timing: LauncherFrameFinishTraceTiming,
        defer_preview_trace_flush: bool,
    ) {
        #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
        {
            self.write_preview_trace(
                frame,
                timing.runtime_status_write_us,
                timing.runtime_status_write_deferred,
                timing.frame_finish_us,
                defer_preview_trace_flush,
            );
            self.last_preview_trace_finish_done = Some(Instant::now());
        }
        #[cfg(not(any(feature = "bench-tools", feature = "diagnostics")))]
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
        nav: &LauncherNav,
        pad: &PadPool,
        catalog: &ArcadeCatalog,
        catalog_ready: bool,
        catalog_refresh_done: bool,
        launching: bool,
        loading_title: &str,
        catalog_scan_visible: bool,
        catalog_scan_title: &str,
        catalog_scan_detail: &str,
        catalog_scan_percent: i32,
        catalog_background_scan_visible: bool,
        catalog_scan_message: &str,
        confirm_visible: bool,
        confirm_title: &str,
        confirm_message: &str,
        confirm_selected: i32,
        confirm_left_label: &str,
        confirm_right_label: &str,
        arcade_selected: usize,
        arcade_visual_index: f32,
        preview_cache_state: &str,
        preview_transition_effect: &str,
        preview_transition_progress: f32,
        composition_status: &UiCompositionStatus,
        launcher_bench_scenario: Option<LauncherBenchScenario>,
        start_screen: Screen,
        lock_screen: Option<Screen>,
        route_reassert_count: u64,
        last_route_reassert_frame: u64,
        last_route_reassert_ok: bool,
        last_route_reassert_error: &str,
        startup_status: StartupRevealStatus,
        return_session: &LaunchReturnSession,
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
            nav,
            pad,
            catalog,
            catalog_ready,
            catalog_refresh_done,
            launching,
            loading_title,
            catalog_scan_visible,
            catalog_scan_title,
            catalog_scan_detail,
            catalog_scan_percent,
            catalog_background_scan_visible,
            catalog_scan_message,
            confirm_visible,
            confirm_title,
            confirm_message,
            confirm_selected,
            confirm_left_label,
            confirm_right_label,
            arcade_selected,
            arcade_visual_index,
            preview_cache_state,
            preview_transition_effect,
            preview_transition_progress,
            0,
            composition_status,
            launcher_bench_scenario,
            start_screen,
            lock_screen,
            route_reassert_count,
            last_route_reassert_frame,
            last_route_reassert_ok,
            last_route_reassert_error,
            startup_status,
            return_session,
            Some((self.idle_loops_since_status, last_frame_ms_ago)),
        );
    }

    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    fn write_preview_trace(
        &mut self,
        frame: &LauncherPresentedFrame,
        runtime_status_write_us: u128,
        runtime_status_write_deferred: bool,
        frame_finish_us: u128,
        defer_flush: bool,
    ) {
        if self
            .preview_scroll_trace_duration
            .is_some_and(|limit| frame.loop_start.duration_since(frame.run_start) > limit)
        {
            self.close_preview_scroll_trace();
            return;
        }

        if self.preview_scroll_trace.is_none() {
            return;
        }

        let loop_delta_us = self
            .last_preview_trace_loop_start
            .map(|previous| {
                frame
                    .loop_start
                    .saturating_duration_since(previous)
                    .as_micros()
            })
            .unwrap_or(0);
        let post_frame_tail_us = self
            .last_preview_trace_frame_t4
            .map(|previous| {
                frame
                    .loop_start
                    .saturating_duration_since(previous)
                    .as_micros()
            })
            .unwrap_or(0);
        let post_finish_tail_us = self
            .last_preview_trace_finish_done
            .map(|previous| {
                frame
                    .loop_start
                    .saturating_duration_since(previous)
                    .as_micros()
            })
            .unwrap_or(0);
        self.last_preview_trace_loop_start = Some(frame.loop_start);
        self.last_preview_trace_frame_t4 = Some(frame.frame_t4);

        let row = preview_scroll_trace_row_from_frame(
            frame,
            loop_delta_us,
            post_frame_tail_us,
            runtime_status_write_us,
            runtime_status_write_deferred,
            frame_finish_us,
            post_finish_tail_us,
        );
        if let Some(trace) = self.preview_scroll_trace.as_mut() {
            trace.push(row, !defer_flush);
        }
    }

    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    fn close_preview_scroll_trace(&mut self) {
        if let Some(mut trace) = self.preview_scroll_trace.take() {
            trace.flush_rows();
        }
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
            if launcher_fps_log_enabled() {
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
                screensaver_renderer: frame.screensaver_frame_trace.renderer,
                navigation_transition_edge: frame.custom_draw_trace.navigation_transition_edge,
                navigation_transition_direction: frame
                    .custom_draw_trace
                    .navigation_transition_direction,
                navigation_transition_us: u128_to_u64_saturating(
                    frame.custom_draw_trace.effect_label_us,
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
                orientation_transition_leg: frame.custom_draw_trace.orientation_transition_leg,
                orientation_transition_from: frame.custom_draw_trace.orientation_transition_from,
                orientation_transition_to: frame.custom_draw_trace.orientation_transition_to,
                orientation_transition_destination_capture_us: u128_to_u64_saturating(
                    frame
                        .custom_draw_trace
                        .orientation_transition_destination_capture_us,
                ),
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
                main_present_status: frame.main_present_status.trace_label(),
                main_present_copy_path: frame.main_present_copy_path,
                main_present_sequence: frame.main_present_sequence,
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
                    frame.screensaver_frame_trace.archive_poll_us,
                ),
                screensaver_card_adopt_us: u128_to_u64_saturating(
                    frame.screensaver_frame_trace.card_adopt_us,
                ),
                screensaver_parade_advance_us: u128_to_u64_saturating(
                    frame.screensaver_frame_trace.parade_advance_us,
                ),
                screensaver_background_us: u128_to_u64_saturating(
                    frame.screensaver_frame_trace.background_us,
                ),
                screensaver_draw_order_us: u128_to_u64_saturating(
                    frame.screensaver_frame_trace.draw_order_us,
                ),
                screensaver_tile_blit_us: u128_to_u64_saturating(
                    frame.screensaver_frame_trace.tile_blit_us,
                ),
                screensaver_raster_held_cards: usize_to_u64_saturating(
                    frame.screensaver_frame_trace.raster_held_cards,
                ),
                screensaver_raster_moved_cards: usize_to_u64_saturating(
                    frame.screensaver_frame_trace.raster_moved_cards,
                ),
                screensaver_raster_hold_layer_mask: frame
                    .screensaver_frame_trace
                    .raster_hold_layer_mask,
                screensaver_raster_visible_layer_mask: frame
                    .screensaver_frame_trace
                    .raster_visible_layer_mask,
                screensaver_phase_bank_bytes: usize_to_u64_saturating(
                    frame.screensaver_frame_trace.phase_bank_resident_bytes,
                ),
                screensaver_render_ahead_sequence: frame
                    .screensaver_frame_trace
                    .render_ahead_sequence,
                screensaver_render_ahead_queue_depth: usize_to_u64_saturating(
                    frame.screensaver_frame_trace.render_ahead_queue_depth,
                ),
                screensaver_render_ahead_frame_age_us: frame
                    .screensaver_frame_trace
                    .render_ahead_frame_age_us,
                screensaver_render_ahead_render_wall_us: frame
                    .screensaver_frame_trace
                    .render_ahead_render_wall_us,
                screensaver_render_ahead_render_cpu_us: frame
                    .screensaver_frame_trace
                    .render_ahead_render_cpu_us,
                screensaver_render_ahead_starvation_count: frame
                    .screensaver_frame_trace
                    .render_ahead_starvation_count,
                screensaver_render_ahead_superseded_frames: frame
                    .screensaver_frame_trace
                    .render_ahead_superseded_frames,
                screensaver_render_ahead_reused_frames: frame
                    .screensaver_frame_trace
                    .render_ahead_reused_frames,
                screensaver_render_ahead_cancelled: frame
                    .screensaver_frame_trace
                    .render_ahead_cancelled,
            });
    }

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

    #[cfg(any(feature = "bench-tools", feature = "diagnostics", feature = "profile"))]
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
        nav: &LauncherNav,
        pad: &PadPool,
        catalog: &ArcadeCatalog,
        catalog_ready: bool,
        catalog_refresh_done: bool,
        launching: bool,
        loading_title: &str,
        catalog_scan_visible: bool,
        catalog_scan_title: &str,
        catalog_scan_detail: &str,
        catalog_scan_percent: i32,
        catalog_background_scan_visible: bool,
        catalog_scan_message: &str,
        confirm_visible: bool,
        confirm_title: &str,
        confirm_message: &str,
        confirm_selected: i32,
        confirm_left_label: &str,
        confirm_right_label: &str,
        arcade_selected: usize,
        arcade_visual_index: f32,
        preview_cache_state: &str,
        preview_transition_effect: &str,
        preview_transition_progress: f32,
        screensaver_active_cards: usize,
        composition_status: &UiCompositionStatus,
        launcher_bench_scenario: Option<LauncherBenchScenario>,
        start_screen: Screen,
        lock_screen: Option<Screen>,
        route_reassert_count: u64,
        last_route_reassert_frame: u64,
        last_route_reassert_ok: bool,
        last_route_reassert_error: &str,
        startup_status: StartupRevealStatus,
        return_session: &LaunchReturnSession,
        idle_status: Option<(u64, u64)>,
    ) {
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
            framebuffer_width: self.framebuffer_width,
            framebuffer_height: self.framebuffer_height,
            frames,
            idle,
            idle_loops,
            status_sequence: self.status_sequence,
            state_revision: self.automation_state_revision,
            presented_state_revision: self.automation_presented_state_revision,
            action_sequence: self.automation_action_sequence,
            presented_action_sequence: self.automation_presented_action_sequence,
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
            preview_transition_effect,
            preview_transition_progress,
            screensaver_active_cards,
            composition_state: composition_status.state,
            composition_recovery_count: composition_status.recovery_count,
            last_composition_invariant_kind: &composition_status.last_invariant_kind,
            last_composition_invariant_detail: &composition_status.last_invariant_detail,
            bench_scenario: launcher_bench_scenario
                .map(LauncherBenchScenario::label)
                .unwrap_or("none"),
            start_screen: screen_label(start_screen),
            lock_screen: lock_screen.map(screen_label).unwrap_or("none"),
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

fn automation_hash_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn launcher_frame_was_presented(frame: &LauncherPresentedFrame) -> bool {
    if frame.main_present_status != LauncherPresentStatus::Ok {
        return false;
    }
    !frame.main_present_backend.is_latch()
        || (frame.main_present_sequence != 0
            && frame.main_present_active_sequence == frame.main_present_sequence
            && !frame.main_present_pending)
}

fn launcher_fps_log_enabled() -> bool {
    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    {
        true
    }
    #[cfg(not(any(feature = "bench-tools", feature = "diagnostics")))]
    {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENABLED.get_or_init(|| {
            std::env::var("MISTER_PROFILE")
                .map(|value| {
                    let value = value.trim();
                    !value.is_empty() && !matches!(value, "0" | "off" | "false")
                })
                .unwrap_or(false)
        })
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
fn cpu_thread_us() -> Option<u64> {
    clock_us(libc::CLOCK_THREAD_CPUTIME_ID)
}

#[cfg(not(target_os = "linux"))]
fn cpu_thread_us() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn cpu_process_us() -> Option<u64> {
    clock_us(libc::CLOCK_PROCESS_CPUTIME_ID)
}

#[cfg(not(target_os = "linux"))]
fn cpu_process_us() -> Option<u64> {
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
            automation: AutomationFrameStamp::default(),
            selected: 0,
            visual_index: 0.0,
            #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
            home_trace: LauncherHomeFrameTrace::default(),
            search_index_state: "building",
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
            main_present_hidden_invalid_bytes: 0,
            main_present_hidden_rect_count: 0,
            main_present_hidden_catchup_bytes: 0,
            main_present_hidden_full_copy: false,
            main_present_copy_path: "vertical-partial",
            main_present_request_us: 0,
            main_present_set_vga_fb_us: 0,
            main_present_wait_us: 0,
            main_present_sequence: 0,
            main_present_active_sequence: 0,
            main_present_pending: false,
            main_present_completion_poll_count: 0,
            main_present_completion_poll_wall_us: 0,
            main_present_completion_poll_cpu_us: 0,
            main_present_flip_count: 0,
            main_present_drop_count: 0,
            vsync_source: Some(VsyncPaceSource::Timeout),
            vsync_period_us: 16_667,
            vsync_miss_streak: 3,
            vsync_stale_hits: 0,
            vsync_wait_start_age_us: 12_000,
            vsync_accepted_hit_age_us: 500,
            frame_start_phase_us: 8_000,
            present_phase_us: 0,
            home_pan_present_active: true,
            home_horizontal_input_held: true,
            redraw_pending: true,
            wake_reasons_bits: 0x40,
            arcade_update_label: ArcadeUpdateTrace::None,
            preview_cache_state: "exact",
            preview_transition: PreviewTransitionTrace::default(),
            composition_status: UiCompositionStatus::default(),
            screensaver_active: false,
            screensaver_active_cards: 0,
            screensaver_archive_loading: false,
            screensaver_frame_trace: ScreensaverFrameTrace::default(),
            status_write_due: false,
            status_string_copy_us: 10,
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
                automation: frame.automation,
                selected: frame.selected,
                visual_index: frame.visual_index,
                #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
                home_trace: frame.home_trace,
                search_index_state: frame.search_index_state,
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
                home_pan_present_active: frame.home_pan_present_active,
                home_horizontal_input_held: frame.home_horizontal_input_held,
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
                screensaver_archive_loading: frame.screensaver_archive_loading,
                screensaver_frame_trace: frame.screensaver_frame_trace,
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
                main_present_backend: frame.main_present_backend,
                main_present_status: frame.main_present_status,
                main_present_buffer: frame.main_present_buffer,
                main_present_hidden_copy_us: frame.main_present_hidden_copy_us,
                main_present_hidden_publish_us: frame.main_present_hidden_publish_us,
                main_present_hidden_invalid_bytes: frame.main_present_hidden_invalid_bytes,
                main_present_hidden_rect_count: frame.main_present_hidden_rect_count,
                main_present_hidden_catchup_bytes: frame.main_present_hidden_catchup_bytes,
                main_present_hidden_full_copy: frame.main_present_hidden_full_copy,
                main_present_copy_path: frame.main_present_copy_path,
                main_present_request_us: frame.main_present_request_us,
                main_present_set_vga_fb_us: frame.main_present_set_vga_fb_us,
                main_present_wait_us: frame.main_present_wait_us,
                main_present_sequence: frame.main_present_sequence,
                main_present_flip_count: frame.main_present_flip_count,
                main_present_drop_count: frame.main_present_drop_count,
                arcade_update_label: frame.arcade_update_label,
            },
            status: LauncherFrameStatusData {
                status_write_due: frame.status_write_due,
                status_string_copy_us: frame.status_string_copy_us,
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
    fn automation_ack_requires_successful_completed_presentation() {
        let now = Instant::now();
        let mut frame = presented_frame(1, now, 16_000);
        frame.main_present_backend = LauncherPresentBackend::FpgaVblankLatchHidden;
        frame.main_present_status = LauncherPresentStatus::Ok;
        frame.main_present_sequence = 17;
        frame.main_present_active_sequence = 17;
        frame.main_present_pending = false;
        assert!(launcher_frame_was_presented(&frame));

        frame.main_present_pending = true;
        assert!(!launcher_frame_was_presented(&frame));
        frame.main_present_pending = false;
        frame.main_present_status = LauncherPresentStatus::Frozen;
        assert!(!launcher_frame_was_presented(&frame));
    }

    #[test]
    fn frame_snapshot_builder_populates_existing_fields() {
        let start = Instant::now();
        let expected = presented_frame(42, start, 21_000);

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

    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    #[test]
    fn frame_snapshot_builder_preserves_preview_trace_row_output() {
        let start = Instant::now();
        let expected = presented_frame(44, start, 22_000);
        let built = builder_from_frame(&expected).build();
        let loop_delta_us = 16_667;
        let runtime_status_write_us = 321;
        let mut expected_row = String::new();
        let mut built_row = String::new();

        preview_scroll_trace_row_from_frame(
            &expected,
            loop_delta_us,
            3_210,
            runtime_status_write_us,
            false,
            654,
            987,
        )
        .write_tsv(&mut expected_row);
        preview_scroll_trace_row_from_frame(
            &built,
            loop_delta_us,
            3_210,
            runtime_status_write_us,
            false,
            654,
            987,
        )
        .write_tsv(&mut built_row);

        assert_eq!(built_row, expected_row);
    }

    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    #[test]
    fn preview_trace_keeps_launcher_start_clock_when_benchmark_clock_is_rebased() {
        let startup_start = Instant::now();
        let loop_start = startup_start + Duration::from_millis(250);
        let mut frame = presented_frame(45, loop_start, 16_000);
        frame.startup_start = startup_start;
        frame.startup_monotonic_us = 1_000_000;
        frame.run_start = loop_start;

        let row = preview_scroll_trace_row_from_frame(&frame, 16_667, 0, 0, false, 0, 0);

        assert_eq!(row.elapsed_us, 0);
        assert_eq!(row.startup_elapsed_us, 250_000);
        assert_eq!(row.monotonic_us, 1_250_000);
    }

    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    #[test]
    fn preview_trace_uses_present_timing_overrides_when_present_happens_before_vsync() {
        let start = Instant::now();
        let frame = presented_frame(45, start, 22_000);
        let mut builder = builder_from_frame(&frame);
        builder.presentation.fb_present_us_override = Some(1_700);
        builder.presentation.vsync_us_override = Some(8_200);
        builder.presentation.hidden_compose_us = 730;
        builder.presentation.hidden_preview_compose_us = 230;
        builder.presentation.hidden_arcade_compose_us = 500;
        builder.presentation.direct_preview_present_us = 230;
        builder.presentation.arcade_list_present_us = 500;
        let built = builder.build();

        let row = preview_scroll_trace_row_from_frame(&built, 16_667, 3_210, 0, false, 654, 987);

        assert_eq!(row.fb_present_us, 1_700);
        assert_eq!(row.vsync_us, 8_200);
        assert_eq!(row.hidden_compose_us, 730);
        assert_eq!(row.hidden_preview_compose_us, 230);
        assert_eq!(row.hidden_arcade_compose_us, 500);
        assert_eq!(row.direct_preview_present_us, 230);
        assert_eq!(row.arcade_list_present_us, 500);
        assert_eq!(row.pre_render_wait_us, 400);
        assert_eq!(row.post_present_wait_us, 800);
        assert_eq!(row.post_frame_tail_us, 3_210);
        assert_eq!(row.runtime_status_write_deferred, 0);
        assert_eq!(row.frame_tail_slack_us, 0);
        assert_eq!(row.status_write_duration_us, 0);
        assert_eq!(row.frame_finish_us, 654);
        assert_eq!(row.post_finish_tail_us, 987);
        assert_eq!(row.home_pan_present_active, 1);
        assert_eq!(row.home_horizontal_input_held, 1);
        assert_eq!(row.redraw_pending, 1);
        assert_eq!(row.wake_reasons_bits, 0x40);
    }

    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    #[test]
    fn preview_trace_serializes_typed_presentation_labels_at_the_accounting_edge() {
        let start = Instant::now();
        let cases = [
            (
                LauncherPresentBackend::None,
                LauncherPresentStatus::None,
                "none",
                "none",
            ),
            (
                LauncherPresentBackend::Fb0Dirty,
                LauncherPresentStatus::None,
                "fb0-dirty",
                "none",
            ),
            (
                LauncherPresentBackend::FpgaVblankLatchHidden,
                LauncherPresentStatus::Ok,
                "fpga-vblank-latch-hidden",
                "ok",
            ),
            (
                LauncherPresentBackend::FpgaVblankLatchHidden,
                LauncherPresentStatus::Unsupported,
                "fpga-vblank-latch-hidden",
                "unsupported",
            ),
        ];

        for (backend, status, expected_backend, expected_status) in cases {
            let mut frame = presented_frame(45, start, 22_000);
            frame.main_present_backend = backend;
            frame.main_present_status = status;

            let row = preview_scroll_trace_row_from_frame(&frame, 16_667, 0, 0, false, 0, 0);

            assert_eq!(row.main_present_backend, expected_backend);
            assert_eq!(row.main_present_status, expected_status);
        }
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
        let mut accounting = LauncherFrameAccounting::new(start, "hdmi", 960, 540);
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
        frame.screensaver_frame_trace.raster_held_cards = 2;
        frame.screensaver_frame_trace.raster_moved_cards = 8;
        frame.screensaver_frame_trace.raster_hold_layer_mask = 1;
        frame.screensaver_frame_trace.raster_visible_layer_mask = 3;
        frame.screensaver_frame_trace.phase_bank_resident_bytes = 12_345;

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
        let mut accounting = LauncherFrameAccounting::new(start, "crt-576p50", 640, 576);
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
        let mut accounting = LauncherFrameAccounting::new(start, "crt-576p50", 640, 576);
        accounting.accumulate_frame_budget(&presented_frame(7, start, FRAME_CADENCE_WARNING_US), 0);

        let status = accounting.current_frame_budget_status();
        assert_eq!(status.slow_frames.len(), 1);
        assert_eq!(status.slow_frames[0].frame, 7);
        assert_eq!(status.slow_frames[0].severity, "cadence-warning");
        assert_eq!(status.slow_frames[0].warning_us, FRAME_CADENCE_WARNING_US);
        assert_eq!(status.slow_frames[0].over_budget_us, 0);
    }
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
fn preview_scroll_trace_duration_from_env() -> Option<Duration> {
    let secs = std::env::var("MISTER_PREVIEW_SCROLL_TRACE_SECS")
        .ok()?
        .parse::<u64>()
        .ok()?;
    (secs > 0).then(|| Duration::from_secs(secs))
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
fn open_preview_scroll_trace() -> Option<PreviewScrollTrace> {
    std::env::var("MISTER_PREVIEW_SCROLL_TRACE")
        .ok()
        .and_then(|path| {
            let file = std::fs::File::create(&path)
                .map_err(|e| crate::ui_errln!("preview scroll trace: create {path} failed: {e}"))
                .ok()?;
            let mut file = BufWriter::with_capacity(64 * 1024, file);
            file.write_all(
                b"frame\telapsed_us\tloop_delta_us\tselected\tvisual_index\thome_screen\thome_menu_token\thome_selected_token\thome_selected_index\thome_scroll_x\thome_scroll_max\tcache_state\ttransition_effect\ttransition_progress\tarcade_update\trows\tdirect_preview_rows\tpresent_bytes\twasted_present_bytes\tprepare_us\tcatalog_worker_us\tcatalog_message_count\tcatalog_backlog\tcatalog_ready_deferred\tcatalog_ready_deferred_age_us\tmedia_worker_us\tmedia_gate_us\tpreview_schedule_us\tpreview_apply_us\tslint_render_us\tcustom_draw_us\tarcade_list_update_us\tpreview_blit_us\tpreview_fade_wall_us\tpreview_fade_cpu_us\tpreview_fade_pixels\tpreview_fade_rows\tpreview_fade_path\tpreview_fade_alpha_bucket\teffect_label_us\tpre_render_wait_us\tpost_present_wait_us\tpost_frame_tail_us\tvsync_us\tfb_present_us\tcached_present_us\thidden_compose_us\thidden_preview_compose_us\thidden_arcade_compose_us\tdirect_preview_present_us\tarcade_list_present_us\tmain_present_backend\tmain_present_status\tmain_present_buffer\tmain_present_hidden_copy_us\tmain_present_hidden_invalid_bytes\tmain_present_hidden_rect_count\tmain_present_hidden_catchup_bytes\tmain_present_hidden_full_copy\tmain_present_request_us\tmain_present_set_vga_fb_us\tmain_present_wait_us\tmain_present_sequence\tmain_present_flip_count\tmain_present_drop_count\tvsync_source\tvsync_period_us\tvsync_miss_streak\tvsync_stale_hits\tvsync_wait_start_age_us\tvsync_accepted_hit_age_us\tframe_start_phase_us\tpresent_phase_us\thome_pan_present_active\thome_horizontal_input_held\tredraw_pending\twake_reasons_bits\tdirty_y0\tdirty_y1\tstatus_write_due\truntime_status_write_deferred\tframe_tail_slack_us\tstatus_string_copy_us\tstatus_string_copy_bytes\truntime_status_write_us\tstatus_write_duration_us\twall_us\tframe_finish_us\tpost_finish_tail_us\tscreensaver_active\tscreensaver_active_cards\tscreensaver_archive_loading\tscreensaver_archive_poll_us\tscreensaver_card_adopt_us\tscreensaver_cards_adopted\tscreensaver_parade_advance_us\tscreensaver_background_us\tscreensaver_draw_order_us\tscreensaver_tile_blit_us\tscreensaver_cards_drawn\tscreensaver_cards_culled\tsearch_index_state\tstartup_elapsed_us\tmonotonic_us\n",
            )
            .map_err(|e| crate::ui_errln!("preview scroll trace: header write failed: {e}"))
            .ok()?;
            crate::ui_logln!("preview_scroll_trace={path}");
            Some(PreviewScrollTrace::new(file))
        })
}
