use super::*;
use std::collections::VecDeque;
#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
use std::fmt::Write as _;
#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
use std::io::{BufWriter, Write as _};

const FRAME_BUDGET_US: u64 = 16_667;
const FRAME_NEAR_DROP_US: u64 = 16_000;
const FRAME_BUDGET_20MS_US: u64 = 20_000;
const FRAME_BUDGET_33MS_US: u64 = 33_334;
const FRAME_ANALYTICS_LEASE_PATH: &str = "/tmp/mister-magik/realtime-frame-analytics";
const FRAME_ANALYTICS_LEASE_MAX_AGE: Duration = Duration::from_secs(3);
const FRAME_ANALYTICS_SAMPLE_CAP: usize = 75;
const FRAME_SLOW_SAMPLE_CAP: usize = 32;

pub(super) struct LauncherFrameAccounting {
    fps_window_start: Instant,
    fps_frames: u64,
    prepare_us: u128,
    render_us: u128,
    custom_draw_us: u128,
    vsync_us: u128,
    copy_us: u128,
    cached_present_us: u128,
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
    boot_frame_profile: Option<boot_analytics::LauncherFrameWriter>,
    last_status_write: Instant,
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
    frame_budget_total: FrameBudgetAccumulator,
    frame_budget_window: FrameBudgetAccumulator,
    last_frame_budget_status: runtime_status::FrameBudgetStatus,
    frame_analytics_mode: FrameAnalyticsMode,
    frame_analytics_samples: VecDeque<runtime_status::FrameBudgetRecentFrame>,
    slow_frame_samples: VecDeque<runtime_status::FrameBudgetSlowFrame>,
}

pub(super) struct LauncherPresentedFrame {
    pub(super) frames: u64,
    pub(super) selected: usize,
    pub(super) visual_index: f32,
    pub(super) run_start: Instant,
    pub(super) loop_start: Instant,
    pub(super) frame_t0: Instant,
    pub(super) frame_t1: Instant,
    pub(super) frame_t2: Instant,
    pub(super) frame_t3: Instant,
    pub(super) frame_t4: Instant,
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
    pub(super) cached_present_us: u128,
    pub(super) direct_preview_present_us: u128,
    pub(super) arcade_list_present_us: u128,
    pub(super) vsync_source: Option<VsyncPaceSource>,
    pub(super) vsync_period_us: u64,
    pub(super) vsync_miss_streak: u32,
    pub(super) vsync_stale_hits: u32,
    pub(super) vsync_wait_start_age_us: u64,
    pub(super) vsync_accepted_hit_age_us: u64,
    pub(super) frame_start_phase_us: u64,
    pub(super) present_phase_us: u128,
    pub(super) arcade_update_label: ArcadeUpdateTrace,
    pub(super) preview_cache_state: &'static str,
    pub(super) preview_transition: PreviewTransitionTrace,
    pub(super) composition_status: UiCompositionStatus,
    pub(super) status_write_due: bool,
    pub(super) status_string_copy_us: u128,
    pub(super) status_string_copy_bytes: usize,
    pub(super) cpu_loop_start: FrameAnalyticsCpuStamp,
    pub(super) cpu_t0: FrameAnalyticsCpuStamp,
    pub(super) cpu_t1: FrameAnalyticsCpuStamp,
    pub(super) cpu_t2: FrameAnalyticsCpuStamp,
    pub(super) cpu_custom_draw_start: FrameAnalyticsCpuStamp,
    pub(super) cpu_custom_draw_done: FrameAnalyticsCpuStamp,
    pub(super) cpu_t3: FrameAnalyticsCpuStamp,
    pub(super) cpu_t4: FrameAnalyticsCpuStamp,
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
    fn from_lease_text(text: &str) -> Self {
        match text.trim() {
            "wall" => Self::Wall,
            "thread" => Self::Thread,
            "process" | "1" | "true" => Self::Process,
            _ => Self::Off,
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
    effect_label_us: u128,
    vsync_us: u128,
    fb_present_us: u128,
    cached_present_us: u128,
    direct_preview_present_us: u128,
    arcade_list_present_us: u128,
    vsync_source: &'static str,
    vsync_period_us: u64,
    vsync_miss_streak: u32,
    vsync_stale_hits: u32,
    vsync_wait_start_age_us: u64,
    vsync_accepted_hit_age_us: u64,
    frame_start_phase_us: u64,
    present_phase_us: u128,
    dirty_y0: usize,
    dirty_y1: usize,
    status_write_due: u8,
    status_string_copy_us: u128,
    status_string_copy_bytes: usize,
    runtime_status_write_us: u128,
    wall_us: u128,
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
impl PreviewScrollTrace {
    fn new(writer: BufWriter<std::fs::File>) -> Self {
        Self {
            writer,
            rows: Vec::with_capacity(4096),
            row_text: String::with_capacity(384),
        }
    }

    fn push(&mut self, row: PreviewScrollTraceRow) {
        self.rows.push(row);
        if self.rows.len() >= 4096 {
            self.flush_rows();
        }
    }

    fn flush_rows(&mut self) {
        let rows = std::mem::take(&mut self.rows);
        for row in rows {
            self.row_text.clear();
            let _ = write!(
                self.row_text,
                "{}\t{}\t{}\t{}\t{:.6}\t{}\t{}\t{:.3}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                row.frame,
                row.elapsed_us,
                row.loop_delta_us,
                row.selected,
                row.visual_index,
                row.cache_state,
                row.transition_effect,
                row.transition_progress,
                row.arcade_update,
                row.copied_rows,
                row.direct_preview_rows,
                row.present_bytes,
                row.wasted_present_bytes,
                row.prepare_us,
                row.catalog_worker_us,
                row.catalog_message_count,
                row.catalog_backlog,
                row.catalog_ready_deferred,
                row.catalog_ready_deferred_age_us,
                row.media_worker_us,
                row.media_gate_us,
                row.preview_schedule_us,
                row.preview_apply_us,
                row.slint_render_us,
                row.custom_draw_us,
                row.arcade_list_update_us,
                row.preview_blit_us,
                row.effect_label_us,
                row.vsync_us,
                row.fb_present_us,
                row.cached_present_us,
                row.direct_preview_present_us,
                row.arcade_list_present_us,
                row.vsync_source,
                row.vsync_period_us,
                row.vsync_miss_streak,
                row.vsync_stale_hits,
                row.vsync_wait_start_age_us,
                row.vsync_accepted_hit_age_us,
                row.frame_start_phase_us,
                row.present_phase_us,
                row.dirty_y0,
                row.dirty_y1,
                row.status_write_due,
                row.status_string_copy_us,
                row.status_string_copy_bytes,
                row.runtime_status_write_us,
                row.wall_us
            );
            let _ = self.writer.write_all(self.row_text.as_bytes());
        }
        let _ = self.writer.flush();
    }
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
impl Drop for PreviewScrollTrace {
    fn drop(&mut self) {
        self.flush_rows();
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct LauncherCustomDrawTrace {
    pub(super) arcade_list_update_us: u128,
    pub(super) preview_blit_us: u128,
    pub(super) effect_label_us: u128,
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
    pub(super) fn new(run_start: Instant) -> Self {
        Self {
            fps_window_start: run_start,
            fps_frames: 0,
            prepare_us: 0,
            render_us: 0,
            custom_draw_us: 0,
            vsync_us: 0,
            copy_us: 0,
            cached_present_us: 0,
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
            boot_frame_profile: boot_analytics::LauncherFrameWriter::from_env(),
            last_status_write: Instant::now() - Duration::from_secs(2),
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
            frame_budget_total: FrameBudgetAccumulator::default(),
            frame_budget_window: FrameBudgetAccumulator::default(),
            last_frame_budget_status: runtime_status::FrameBudgetStatus {
                budget_us: FRAME_BUDGET_US,
                ..runtime_status::FrameBudgetStatus::default()
            },
            frame_analytics_mode: FrameAnalyticsMode::Off,
            frame_analytics_samples: VecDeque::with_capacity(FRAME_ANALYTICS_SAMPLE_CAP),
            slow_frame_samples: VecDeque::with_capacity(FRAME_SLOW_SAMPLE_CAP),
        }
    }

    pub(super) fn first_visible_copy_done(&self) -> bool {
        self.first_visible_copy_done
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
    ) {
        if frame.status_write_due {
            self.refresh_frame_analytics_mode();
        }
        self.record_first_copy(&frame, disp);
        self.accumulate_fps(&frame);
        self.accumulate_frame_budget(&frame);
        self.record_stable_samples(frame.frames, disp);
        self.last_rendered_frame_at = frame.frame_t4;
        self.idle_loops_since_status = 0;
        #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
        self.record_boot_frame_profile(&frame, disp);
        self.record_first_frame(&frame, start, catalog_ready);
        #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
        let runtime_status_write_start =
            (frame.status_write_due && self.preview_scroll_trace.is_some()).then(Instant::now);
        self.write_runtime_status(
            frame.status_write_due,
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
            confirm_selected,
            confirm_left_label,
            confirm_right_label,
            frame.selected,
            frame.visual_index,
            frame.preview_cache_state,
            frame.preview_transition.effect.label(),
            frame.preview_transition.progress,
            &frame.composition_status,
            launcher_bench_scenario,
            start_screen,
            lock_screen,
            route_reassert_count,
            last_route_reassert_frame,
            last_route_reassert_ok,
            last_route_reassert_error,
            startup_status,
            None,
        );
        #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
        {
            let runtime_status_write_us = runtime_status_write_start
                .map(|start| start.elapsed().as_micros())
                .unwrap_or(0);
            self.write_preview_trace(&frame, runtime_status_write_us);
        }
    }

    fn refresh_frame_analytics_mode(&mut self) {
        let mode = std::fs::metadata(FRAME_ANALYTICS_LEASE_PATH)
            .and_then(|metadata| {
                let age = metadata
                    .modified()?
                    .elapsed()
                    .unwrap_or(FRAME_ANALYTICS_LEASE_MAX_AGE);
                if age <= FRAME_ANALYTICS_LEASE_MAX_AGE {
                    std::fs::read_to_string(FRAME_ANALYTICS_LEASE_PATH)
                } else {
                    Ok(String::new())
                }
            })
            .map(|text| FrameAnalyticsMode::from_lease_text(&text))
            .unwrap_or(FrameAnalyticsMode::Off);
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
            confirm_selected,
            confirm_left_label,
            confirm_right_label,
            arcade_selected,
            arcade_visual_index,
            preview_cache_state,
            preview_transition_effect,
            preview_transition_progress,
            composition_status,
            launcher_bench_scenario,
            start_screen,
            lock_screen,
            route_reassert_count,
            last_route_reassert_frame,
            last_route_reassert_ok,
            last_route_reassert_error,
            startup_status,
            Some((self.idle_loops_since_status, last_frame_ms_ago)),
        );
    }

    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    fn write_preview_trace(
        &mut self,
        frame: &LauncherPresentedFrame,
        runtime_status_write_us: u128,
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
        self.last_preview_trace_loop_start = Some(frame.loop_start);

        let row = PreviewScrollTraceRow {
            frame: frame.frames,
            elapsed_us: frame.loop_start.duration_since(frame.run_start).as_micros(),
            loop_delta_us,
            selected: frame.selected,
            visual_index: frame.visual_index,
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
            effect_label_us: frame.custom_draw_trace.effect_label_us,
            vsync_us: (frame.frame_t3 - frame.custom_draw_done).as_micros(),
            fb_present_us: (frame.frame_t4 - frame.frame_t3).as_micros(),
            cached_present_us: frame.cached_present_us,
            direct_preview_present_us: frame.direct_preview_present_us,
            arcade_list_present_us: frame.arcade_list_present_us,
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
            dirty_y0: frame.dirty_rect.map(|rect| rect.y0).unwrap_or(0),
            dirty_y1: frame.dirty_rect.map(|rect| rect.y1).unwrap_or(0),
            status_write_due: u8::from(frame.status_write_due),
            status_string_copy_us: frame.prepare_trace.status_string_copy_us,
            status_string_copy_bytes: frame.status_string_copy_bytes,
            runtime_status_write_us,
            wall_us: (frame.frame_t4 - frame.loop_start).as_micros(),
        };
        if let Some(trace) = self.preview_scroll_trace.as_mut() {
            trace.push(row);
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
            crate::ui_logln!(
                "launcher fps ~ {} prepare {}us slint-render {}us custom-draw {}us vsync-wait {}us fb-present {}us cached-present {}us direct-preview-present {}us arcade-list-present {}us ({} rows avg)",
                self.fps_frames,
                self.prepare_us / n,
                self.render_us / n,
                self.custom_draw_us / n,
                self.vsync_us / n,
                self.copy_us / n,
                self.cached_present_us / n,
                self.direct_preview_present_us / n,
                self.arcade_list_present_us / n,
                self.rows / n
            );
            self.fps_window_start = Instant::now();
            self.fps_frames = 0;
            self.prepare_us = 0;
            self.render_us = 0;
            self.custom_draw_us = 0;
            self.vsync_us = 0;
            self.copy_us = 0;
            self.cached_present_us = 0;
            self.direct_preview_present_us = 0;
            self.arcade_list_present_us = 0;
            self.rows = 0;
        }
    }

    fn accumulate_frame_budget(&mut self, frame: &LauncherPresentedFrame) {
        let wall_us = u128_to_u64_saturating((frame.frame_t4 - frame.loop_start).as_micros());
        let prepare_us = u128_to_u64_saturating(frame.prepare_us);
        let render_us = u128_to_u64_saturating((frame.frame_t2 - frame.frame_t1).as_micros());
        let custom_draw_us =
            u128_to_u64_saturating((frame.custom_draw_done - frame.custom_draw_start).as_micros());
        let vsync_us =
            u128_to_u64_saturating((frame.frame_t3 - frame.custom_draw_done).as_micros());
        let present_us = u128_to_u64_saturating((frame.frame_t4 - frame.frame_t3).as_micros());
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
        if wall_us >= FRAME_NEAR_DROP_US {
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
    ) {
        if self.frame_analytics_samples.len() == FRAME_ANALYTICS_SAMPLE_CAP {
            self.frame_analytics_samples.pop_front();
        }
        self.frame_analytics_samples
            .push_back(runtime_status::FrameBudgetRecentFrame {
                frame: frame.frames,
                wall_us,
                prepare_us,
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
                cpu_present_us: cpu_delta(frame.cpu_t3, frame.cpu_t4),
                process_cpu_us: frame
                    .cpu_t4
                    .process_us
                    .saturating_sub(frame.cpu_loop_start.process_us),
                vsync_source: vsync_source_label(frame.vsync_source),
                vsync_miss_streak: frame.vsync_miss_streak,
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
            self.slow_frame_samples.pop_front();
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
            .push_back(runtime_status::FrameBudgetSlowFrame {
                frame: frame.frames,
                severity: if wall_us > FRAME_BUDGET_US {
                    "drop"
                } else {
                    "near-drop"
                },
                wall_us,
                warning_us: FRAME_NEAR_DROP_US,
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
            recent_frames: self.frame_analytics_samples.iter().copied().collect(),
            slow_frames: self.slow_frame_samples.iter().copied().collect(),
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

    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
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
        let frame_budget = if idle {
            self.last_frame_budget_status.clone()
        } else {
            self.current_frame_budget_status()
        };
        runtime_status::write_launcher_status(LauncherStatus {
            scene: "launcher",
            screen: screen_label(nav.screen),
            frames,
            idle,
            idle_loops,
            fps_estimate,
            rolling_fps,
            rolling_prepare_us,
            rolling_render_us,
            rolling_custom_draw_us,
            rolling_vsync_us,
            rolling_present_us,
            rolling_rows,
            last_frame_ms_ago,
            catalog_ready,
            catalog_games: catalog.len(),
            catalog_systems: catalog.systems.len(),
            catalog_refresh_done,
            catalog_scan_visible,
            catalog_scan_message,
            catalog_scan_title,
            catalog_scan_detail,
            catalog_scan_percent,
            catalog_background_scan_visible,
            confirm_visible,
            confirm_title,
            confirm_selected,
            confirm_left_label,
            confirm_right_label,
            arcade_selected,
            arcade_visual_index,
            preview_cache_state,
            preview_transition_effect,
            preview_transition_progress,
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
            revealed: startup_status.revealed,
            input_enabled: startup_status.input_enabled,
            reveal_ms: startup_status.reveal_ms,
            input_enabled_ms: startup_status.input_enabled_ms,
            frame_budget: frame_budget.clone(),
        });
        if !idle {
            self.last_frame_budget_status = frame_budget;
            self.frame_budget_window = FrameBudgetAccumulator::default();
        }
        self.frame_analytics_samples.clear();
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
    cpu_clock_us(libc::CLOCK_THREAD_CPUTIME_ID)
}

#[cfg(not(target_os = "linux"))]
fn cpu_thread_us() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn cpu_process_us() -> Option<u64> {
    cpu_clock_us(libc::CLOCK_PROCESS_CPUTIME_ID)
}

#[cfg(not(target_os = "linux"))]
fn cpu_process_us() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn cpu_clock_us(clock_id: libc::clockid_t) -> Option<u64> {
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
            selected: 0,
            visual_index: 0.0,
            run_start: loop_start,
            loop_start,
            frame_t0,
            frame_t1,
            frame_t2,
            frame_t3,
            frame_t4,
            custom_draw_start,
            custom_draw_done,
            custom_draw_trace: LauncherCustomDrawTrace::default(),
            prepare_trace: LauncherPrepareTrace {
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
            cached_present_us: 0,
            direct_preview_present_us: 0,
            arcade_list_present_us: 0,
            vsync_source: Some(VsyncPaceSource::Timeout),
            vsync_period_us: 16_667,
            vsync_miss_streak: 3,
            vsync_stale_hits: 0,
            vsync_wait_start_age_us: 12_000,
            vsync_accepted_hit_age_us: 500,
            frame_start_phase_us: 8_000,
            present_phase_us: 0,
            arcade_update_label: ArcadeUpdateTrace::None,
            preview_cache_state: "exact",
            preview_transition: PreviewTransitionTrace::default(),
            composition_status: UiCompositionStatus::default(),
            status_write_due: false,
            status_string_copy_us: 10,
            status_string_copy_bytes: 128,
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
        let mut accounting = LauncherFrameAccounting::new(start);
        for frame in 0..40 {
            accounting.accumulate_frame_budget(&presented_frame(
                frame,
                start + Duration::from_micros(frame * 25_000),
                22_000,
            ));
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
    fn near_drop_frame_samples_are_retained_before_budget_miss() {
        let start = Instant::now();
        let mut accounting = LauncherFrameAccounting::new(start);
        accounting.accumulate_frame_budget(&presented_frame(7, start, FRAME_NEAR_DROP_US));

        let status = accounting.current_frame_budget_status();
        assert_eq!(status.slow_frames.len(), 1);
        assert_eq!(status.slow_frames[0].frame, 7);
        assert_eq!(status.slow_frames[0].severity, "near-drop");
        assert_eq!(status.slow_frames[0].warning_us, FRAME_NEAR_DROP_US);
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
                b"frame\telapsed_us\tloop_delta_us\tselected\tvisual_index\tcache_state\ttransition_effect\ttransition_progress\tarcade_update\trows\tdirect_preview_rows\tpresent_bytes\twasted_present_bytes\tprepare_us\tcatalog_worker_us\tcatalog_message_count\tcatalog_backlog\tcatalog_ready_deferred\tcatalog_ready_deferred_age_us\tmedia_worker_us\tmedia_gate_us\tpreview_schedule_us\tpreview_apply_us\tslint_render_us\tcustom_draw_us\tarcade_list_update_us\tpreview_blit_us\teffect_label_us\tvsync_us\tfb_present_us\tcached_present_us\tdirect_preview_present_us\tarcade_list_present_us\tvsync_source\tvsync_period_us\tvsync_miss_streak\tvsync_stale_hits\tvsync_wait_start_age_us\tvsync_accepted_hit_age_us\tframe_start_phase_us\tpresent_phase_us\tdirty_y0\tdirty_y1\tstatus_write_due\tstatus_string_copy_us\tstatus_string_copy_bytes\truntime_status_write_us\twall_us\n",
            )
            .map_err(|e| crate::ui_errln!("preview scroll trace: header write failed: {e}"))
            .ok()?;
            crate::ui_logln!("preview_scroll_trace={path}");
            Some(PreviewScrollTrace::new(file))
        })
}
