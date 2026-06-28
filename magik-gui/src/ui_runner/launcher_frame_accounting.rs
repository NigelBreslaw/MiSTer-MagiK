use super::*;
use std::fmt::Write as _;
use std::io::{BufWriter, Write as _};

pub(super) struct LauncherFrameAccounting {
    fps_window_start: Instant,
    fps_frames: u64,
    prepare_us: u128,
    render_us: u128,
    custom_draw_us: u128,
    vsync_us: u128,
    copy_us: u128,
    cached_present_us: u128,
    arcade_list_present_us: u128,
    rows: u128,
    preview_scroll_trace: Option<PreviewScrollTrace>,
    preview_scroll_trace_duration: Option<Duration>,
    last_preview_trace_loop_start: Option<Instant>,
    boot_frame_profile: Option<boot_analytics::LauncherFrameWriter>,
    last_status_write: Instant,
    first_copy_logged: bool,
    first_frame_logged: bool,
    first_visible_copy_done: bool,
    stable_frame_logged: bool,
    last_rolling_fps: f64,
    last_rolling_prepare_us: u64,
    last_rolling_render_us: u64,
    last_rolling_custom_draw_us: u64,
    last_rolling_vsync_us: u64,
    last_rolling_present_us: u64,
    last_rolling_rows: u64,
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
    pub(super) cached_present_us: u128,
    pub(super) arcade_list_present_us: u128,
    pub(super) vsync_source: Option<VsyncPaceSource>,
    pub(super) vsync_period_us: u64,
    pub(super) vsync_miss_streak: u32,
    pub(super) arcade_update_label: ArcadeUpdateTrace,
    pub(super) preview_cache_state: &'static str,
    pub(super) preview_transition: PreviewTransitionTrace,
    pub(super) status_write_due: bool,
    pub(super) status_string_copy_us: u128,
    pub(super) status_string_copy_bytes: usize,
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
    pub(super) status_string_copy_us: u128,
}

struct PreviewScrollTrace {
    writer: BufWriter<std::fs::File>,
    rows: Vec<PreviewScrollTraceRow>,
    row_text: String,
}

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
    arcade_list_present_us: u128,
    vsync_source: &'static str,
    vsync_period_us: u64,
    vsync_miss_streak: u32,
    status_write_due: u8,
    status_string_copy_us: u128,
    status_string_copy_bytes: usize,
    runtime_status_write_us: u128,
    wall_us: u128,
}

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
                "{}\t{}\t{}\t{}\t{:.6}\t{}\t{}\t{:.3}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
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
                row.arcade_list_present_us,
                row.vsync_source,
                row.vsync_period_us,
                row.vsync_miss_streak,
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
            arcade_list_present_us: 0,
            rows: 0,
            preview_scroll_trace: open_preview_scroll_trace(),
            preview_scroll_trace_duration: preview_scroll_trace_duration_from_env(),
            last_preview_trace_loop_start: None,
            boot_frame_profile: boot_analytics::LauncherFrameWriter::from_env(),
            last_status_write: Instant::now() - Duration::from_secs(2),
            first_copy_logged: false,
            first_frame_logged: false,
            first_visible_copy_done: false,
            stable_frame_logged: false,
            last_rolling_fps: 0.0,
            last_rolling_prepare_us: 0,
            last_rolling_render_us: 0,
            last_rolling_custom_draw_us: 0,
            last_rolling_vsync_us: 0,
            last_rolling_present_us: 0,
            last_rolling_rows: 0,
        }
    }

    pub(super) fn first_visible_copy_done(&self) -> bool {
        self.first_visible_copy_done
    }

    pub(super) fn preview_scroll_trace_enabled(&self) -> bool {
        self.preview_scroll_trace.is_some()
    }

    pub(super) fn status_write_due(&self) -> bool {
        self.last_status_write.elapsed() >= Duration::from_secs(1)
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
    ) {
        self.record_first_copy(&frame, disp);
        self.accumulate_fps(&frame);
        self.record_stable_samples(frame.frames, disp);
        self.record_boot_frame_profile(&frame, disp);
        self.record_first_frame(start, catalog_ready);
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
            launcher_bench_scenario,
            start_screen,
            lock_screen,
            route_reassert_count,
            last_route_reassert_frame,
            last_route_reassert_ok,
            last_route_reassert_error,
        );
        let runtime_status_write_us = runtime_status_write_start
            .map(|start| start.elapsed().as_micros())
            .unwrap_or(0);
        self.write_preview_trace(&frame, runtime_status_write_us);
    }

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
            arcade_list_present_us: frame.arcade_list_present_us,
            vsync_source: frame
                .vsync_source
                .map(VsyncPaceSource::label)
                .unwrap_or("none"),
            vsync_period_us: frame.vsync_period_us,
            vsync_miss_streak: frame.vsync_miss_streak,
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
            println!(
                "launcher fps ~ {} prepare {}us slint-render {}us custom-draw {}us vsync-wait {}us fb-present {}us cached-present {}us arcade-list-present {}us ({} rows avg)",
                self.fps_frames,
                self.prepare_us / n,
                self.render_us / n,
                self.custom_draw_us / n,
                self.vsync_us / n,
                self.copy_us / n,
                self.cached_present_us / n,
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
            self.arcade_list_present_us = 0;
            self.rows = 0;
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

    fn record_first_frame(&mut self, start: Instant, catalog_ready: bool) {
        if !self.first_frame_logged {
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
        launcher_bench_scenario: Option<LauncherBenchScenario>,
        start_screen: Screen,
        lock_screen: Option<Screen>,
        route_reassert_count: u64,
        last_route_reassert_frame: u64,
        last_route_reassert_ok: bool,
        last_route_reassert_error: &str,
    ) {
        if !status_write_due {
            return;
        }
        let fps_estimate = if run_start.elapsed().as_secs_f64() > 0.0 {
            frames as f64 / run_start.elapsed().as_secs_f64()
        } else {
            0.0
        };
        runtime_status::write_launcher_status(LauncherStatus {
            scene: "launcher",
            screen: screen_label(nav.screen),
            frames,
            fps_estimate,
            rolling_fps: self.last_rolling_fps,
            rolling_prepare_us: self.last_rolling_prepare_us,
            rolling_render_us: self.last_rolling_render_us,
            rolling_custom_draw_us: self.last_rolling_custom_draw_us,
            rolling_vsync_us: self.last_rolling_vsync_us,
            rolling_present_us: self.last_rolling_present_us,
            rolling_rows: self.last_rolling_rows,
            last_frame_ms_ago: 0,
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
        });
        self.last_status_write = Instant::now();
    }
}

fn preview_scroll_trace_duration_from_env() -> Option<Duration> {
    let secs = std::env::var("MISTER_PREVIEW_SCROLL_TRACE_SECS")
        .ok()?
        .parse::<u64>()
        .ok()?;
    (secs > 0).then(|| Duration::from_secs(secs))
}

fn open_preview_scroll_trace() -> Option<PreviewScrollTrace> {
    std::env::var("MISTER_PREVIEW_SCROLL_TRACE")
        .ok()
        .and_then(|path| {
            let file = std::fs::File::create(&path)
                .map_err(|e| eprintln!("preview scroll trace: create {path} failed: {e}"))
                .ok()?;
            let mut file = BufWriter::with_capacity(64 * 1024, file);
            file.write_all(
                b"frame\telapsed_us\tloop_delta_us\tselected\tvisual_index\tcache_state\ttransition_effect\ttransition_progress\tarcade_update\trows\tprepare_us\tcatalog_worker_us\tcatalog_message_count\tcatalog_backlog\tcatalog_ready_deferred\tcatalog_ready_deferred_age_us\tmedia_worker_us\tmedia_gate_us\tpreview_schedule_us\tpreview_apply_us\tslint_render_us\tcustom_draw_us\tarcade_list_update_us\tpreview_blit_us\teffect_label_us\tvsync_us\tfb_present_us\tcached_present_us\tarcade_list_present_us\tvsync_source\tvsync_period_us\tvsync_miss_streak\tstatus_write_due\tstatus_string_copy_us\tstatus_string_copy_bytes\truntime_status_write_us\twall_us\n",
            )
            .map_err(|e| eprintln!("preview scroll trace: header write failed: {e}"))
            .ok()?;
            println!("preview_scroll_trace={path}");
            Some(PreviewScrollTrace::new(file))
        })
}
