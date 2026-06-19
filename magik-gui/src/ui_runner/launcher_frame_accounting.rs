use super::*;

pub(super) struct LauncherFrameAccounting {
    fps_window_start: Instant,
    fps_frames: u64,
    prepare_us: u128,
    render_us: u128,
    custom_draw_us: u128,
    vsync_us: u128,
    copy_us: u128,
    cached_present_us: u128,
    overlay_present_us: u128,
    rows: u128,
    preview_scroll_trace: Option<std::fs::File>,
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
    pub(super) prepare_us: u128,
    pub(super) dirty_rect: Option<DirtyRect>,
    pub(super) copied_rows: u32,
    pub(super) cached_present_us: u128,
    pub(super) overlay_present_us: u128,
    pub(super) present_probe_us: u128,
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
            Some(ArcadeListUpdate::Scroll { delta_y }) => Self::Scroll { delta_y: *delta_y },
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
            overlay_present_us: 0,
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
        disp: &mut Display,
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
        launcher_bench_scenario: Option<LauncherBenchScenario>,
        start_screen: Screen,
        lock_screen: Option<Screen>,
        route_reassert_count: u64,
        last_route_reassert_frame: u64,
        last_route_reassert_ok: bool,
        last_route_reassert_error: &str,
    ) {
        self.write_preview_trace(&frame);
        self.record_first_copy(&frame, disp);
        self.accumulate_fps(&frame);
        self.record_stable_samples(frame.frames, disp);
        self.record_boot_frame_profile(&frame, disp);
        self.record_first_frame(start, catalog_ready);
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
    }

    fn write_preview_trace(&mut self, frame: &LauncherPresentedFrame) {
        if self
            .preview_scroll_trace_duration
            .is_some_and(|limit| frame.loop_start.duration_since(frame.run_start) > limit)
        {
            self.preview_scroll_trace = None;
            return;
        }
        if let Some(file) = self.preview_scroll_trace.as_mut() {
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
            let _ = std::io::Write::write_fmt(
                file,
                format_args!(
                    "{}\t{}\t{}\t{}\t{:.6}\t{}\t{}\t{:.3}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    frame.frames,
                    frame.loop_start.duration_since(frame.run_start).as_micros(),
                    loop_delta_us,
                    frame.selected,
                    frame.visual_index,
                    frame.preview_cache_state,
                    frame.preview_transition.effect.label(),
                    frame.preview_transition.progress,
                    frame.arcade_update_label,
                    frame.copied_rows,
                    frame.prepare_us,
                    (frame.frame_t2 - frame.frame_t1).as_micros(),
                    (frame.custom_draw_done - frame.custom_draw_start).as_micros(),
                    (frame.frame_t3 - frame.custom_draw_done).as_micros(),
                    (frame.frame_t4 - frame.frame_t3).as_micros(),
                    frame.cached_present_us,
                    frame.overlay_present_us,
                    frame.present_probe_us,
                    frame
                        .vsync_source
                        .map(VsyncPaceSource::label)
                        .unwrap_or("none"),
                    frame.vsync_period_us,
                    frame.vsync_miss_streak,
                    u8::from(frame.status_write_due),
                    frame.status_string_copy_us,
                    frame.status_string_copy_bytes,
                    (frame.frame_t4 - frame.loop_start).as_micros()
                ),
            );
            let _ = std::io::Write::flush(file);
        }
    }

    fn record_first_copy(&mut self, frame: &LauncherPresentedFrame, disp: &mut Display) {
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
        self.overlay_present_us += frame.overlay_present_us;
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
                "launcher fps ~ {} prepare {}us slint-render {}us custom-draw {}us vsync-wait {}us fb-present {}us cached-present {}us overlay-present {}us ({} rows avg)",
                self.fps_frames,
                self.prepare_us / n,
                self.render_us / n,
                self.custom_draw_us / n,
                self.vsync_us / n,
                self.copy_us / n,
                self.cached_present_us / n,
                self.overlay_present_us / n,
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
            self.overlay_present_us = 0;
            self.rows = 0;
        }
    }

    fn record_stable_samples(&mut self, frames: u64, disp: &mut Display) {
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

    fn record_boot_frame_profile(&mut self, frame: &LauncherPresentedFrame, disp: &Display) {
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
            catalog_scan_title,
            catalog_scan_detail,
            catalog_scan_percent,
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

fn open_preview_scroll_trace() -> Option<std::fs::File> {
    std::env::var("MISTER_PREVIEW_SCROLL_TRACE")
        .ok()
        .and_then(|path| {
            let mut file = std::fs::File::create(&path)
                .map_err(|e| eprintln!("preview scroll trace: create {path} failed: {e}"))
                .ok()?;
            std::io::Write::write_all(
                &mut file,
                b"frame\telapsed_us\tloop_delta_us\tselected\tvisual_index\tcache_state\ttransition_effect\ttransition_progress\tarcade_update\trows\tprepare_us\tslint_render_us\tcustom_draw_us\tvsync_us\tfb_present_us\tcached_present_us\toverlay_present_us\tpresent_probe_us\tvsync_source\tvsync_period_us\tvsync_miss_streak\tstatus_write_due\tstatus_string_copy_us\tstatus_string_copy_bytes\twall_us\n",
            )
            .map_err(|e| eprintln!("preview scroll trace: header write failed: {e}"))
            .ok()?;
            println!("preview_scroll_trace={path}");
            Some(file)
        })
}
