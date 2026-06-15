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
    boot_frame_profile: Option<boot_analytics::LauncherFrameWriter>,
    last_status_write: Instant,
    first_copy_logged: bool,
    first_frame_logged: bool,
    first_visible_copy_done: bool,
    stable_frame_logged: bool,
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
    pub(super) arcade_update_label: String,
    pub(super) preview_cache_state: &'static str,
    pub(super) preview_transition: PreviewTransitionTrace,
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
            boot_frame_profile: boot_analytics::LauncherFrameWriter::from_env(),
            last_status_write: Instant::now() - Duration::from_secs(2),
            first_copy_logged: false,
            first_frame_logged: false,
            first_visible_copy_done: false,
            stable_frame_logged: false,
        }
    }

    pub(super) fn first_visible_copy_done(&self) -> bool {
        self.first_visible_copy_done
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
    ) {
        self.write_preview_trace(&frame);
        self.record_first_copy(&frame, disp);
        self.accumulate_fps(&frame);
        self.record_stable_samples(frame.frames, disp);
        self.record_boot_frame_profile(&frame, disp);
        self.record_first_frame(start, catalog_ready);
        self.write_runtime_status(
            frame.frames,
            frame.run_start,
            nav,
            pad,
            catalog,
            catalog_ready,
            catalog_refresh_done,
            launching,
            loading_title,
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
            let _ = std::io::Write::write_fmt(
                file,
                format_args!(
                    "{}\t{}\t{}\t{:.6}\t{}\t{}\t{:.3}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    frame.frames,
                    frame.loop_start.duration_since(frame.run_start).as_micros(),
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
        frames: u64,
        run_start: Instant,
        nav: &LauncherNav,
        pad: &PadPool,
        catalog: &ArcadeCatalog,
        catalog_ready: bool,
        catalog_refresh_done: bool,
        launching: bool,
        loading_title: &str,
    ) {
        if self.last_status_write.elapsed() < Duration::from_secs(1) {
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
            last_frame_ms_ago: 0,
            catalog_ready,
            catalog_games: catalog.len(),
            catalog_systems: catalog.systems.len(),
            catalog_refresh_done,
            launch_state: if launching { "launching" } else { "idle" },
            loading_title,
            input_pad_count: pad.len(),
            active_pad_index: pad.active_idx(),
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
                b"frame\telapsed_us\tselected\tvisual_index\tcache_state\ttransition_effect\ttransition_progress\tarcade_update\trows\tprepare_us\tslint_render_us\tcustom_draw_us\tvsync_us\tfb_present_us\tcached_present_us\toverlay_present_us\tpresent_probe_us\twall_us\n",
            )
            .map_err(|e| eprintln!("preview scroll trace: header write failed: {e}"))
            .ok()?;
            println!("preview_scroll_trace={path}");
            Some(file)
        })
}
