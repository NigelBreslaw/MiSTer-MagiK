// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Per-frame render-loop instrumentation (env `MISTER_PROFILE`).
//!
//! Records prepare / anim / slint-render / custom-draw / vsync / fb-present timings every frame and prints a factual
//! summary at exit — percentiles, histogram, slow-frame breakdown by phase.

use std::fs::File;
use std::io::Write;
use std::time::Instant;

use mister_magik_fb::framebuffer::{format::RGB565_BYTES_PER_PIXEL, vsync::VsyncPaceSource};

const FRAME_BUDGET_US: u64 = 16_667; // 60 Hz
const PROFILE: &str = "MISTER_PROFILE";
const VIDEO_PROFILE: &str = "MISTER_VIDEO_PROFILE";
const PROFILE_SLOW_US: &str = "MISTER_PROFILE_SLOW_US";
const PROFILE_FILE: &str = "MISTER_PROFILE_FILE";
const TRACE_FILE: &str = "MISTER_TRACE_FILE";

#[derive(Clone, Copy, Debug)]
pub struct FrameRect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl FrameRect {
    fn width(self) -> u64 {
        self.x1.saturating_sub(self.x0) as u64
    }

    fn height(self) -> u64 {
        self.y1.saturating_sub(self.y0) as u64
    }

    fn pixels(self) -> u64 {
        self.width() * self.height()
    }
}

#[derive(Clone, Debug, Default)]
pub struct VideoFrameProfile {
    pub video_decode_us: u64,
    pub video_scale_us: u64,
    pub video_recv_us: u64,
    pub video_image_us: u64,
    pub video_blit_us: u64,
    pub audio_decode_us: u64,
    pub audio_resample_us: u64,
    pub audio_write_us: u64,
    pub video_frame_updated: bool,
    pub video_queue_depth: u32,
    pub audio_buffer_frames: u32,
    pub audio_underrun: bool,
    pub video_file: String,
    pub video_width: u32,
    pub video_height: u32,
    pub video_present_width: u32,
    pub video_present_height: u32,
    pub video_size_animating: bool,
    pub video_missed_deadlines: u32,
    pub video_codec: String,
    pub audio_codec: String,
}

impl VideoFrameProfile {
    fn dominant_phase(&self) -> Option<(&'static str, u64)> {
        [
            ("video-decode", self.video_decode_us),
            ("video-scale", self.video_scale_us),
            ("video-recv", self.video_recv_us),
            ("video-image", self.video_image_us),
            ("video-blit", self.video_blit_us),
            ("audio-decode", self.audio_decode_us),
            ("audio-resample", self.audio_resample_us),
            ("audio-write", self.audio_write_us),
        ]
        .into_iter()
        .max_by_key(|&(_, value)| value)
        .filter(|&(_, value)| value > 0)
    }
}

#[derive(Clone, Debug)]
pub struct FrameSample {
    pub prepare_us: u64,
    pub anim_us: u64,
    pub slint_render_us: u64,
    pub custom_draw_us: u64,
    pub vsync_us: u64,
    pub fb_present_us: u64,
    pub cached_present_us: u64,
    pub arcade_list_present_us: u64,
    pub rows: u32,
    pub present_rect: Option<FrameRect>,
    pub wall_us: u64,
    pub vsync_source: VsyncPaceSource,
    pub vsync_period_us: u64,
    pub vsync_miss_streak: u32,
    pub video: VideoFrameProfile,
}

impl FrameSample {
    pub fn phases_us(&self) -> u64 {
        self.prepare_us
            + self.anim_us
            + self.slint_render_us
            + self.custom_draw_us
            + self.vsync_us
            + self.fb_present_us
    }

    fn dominant_phase(&self) -> &'static str {
        let m = self
            .prepare_us
            .max(self.anim_us)
            .max(self.slint_render_us)
            .max(self.custom_draw_us)
            .max(self.vsync_us)
            .max(self.fb_present_us);
        let video_dominant = self.video.dominant_phase();
        if let Some((phase, value)) = video_dominant {
            if value > m {
                return phase;
            }
        }
        if m == self.fb_present_us {
            "fb-present"
        } else if m == self.slint_render_us {
            "slint-render"
        } else if m == self.custom_draw_us {
            "custom-draw"
        } else if m == self.vsync_us {
            "vsync"
        } else if m == self.anim_us {
            "anim"
        } else {
            "prepare"
        }
    }
}

fn present_bytes_for_pixels(pixels: u64) -> u64 {
    pixels * RGB565_BYTES_PER_PIXEL as u64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileMode {
    Off,
    Summary,
    Slow,
    Full,
    Trace,
}

impl ProfileMode {
    fn from_values(profile: Option<&str>, video_profile: Option<&str>) -> Self {
        match profile
            .or(video_profile)
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            None | Some("") | Some("0") | Some("false") => Self::Off,
            Some("1") | Some("true") | Some("summary") => Self::Summary,
            Some("slow") => Self::Slow,
            Some("full") => Self::Full,
            Some("trace") => Self::Trace,
            other => {
                crate::ui_errln!(
                    "frame_profile: unknown MISTER_PROFILE={other:?}; use 1|summary|slow|full|trace"
                );
                Self::Summary
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameProfilerConfig {
    mode: ProfileMode,
    slow_threshold_us: u64,
    out_path: Option<String>,
    trace_path: Option<String>,
}

impl FrameProfilerConfig {
    pub fn capture_with<'a>(mut get: impl FnMut(&str) -> Option<&'a str>) -> Self {
        let mode = ProfileMode::from_values(get(PROFILE), get(VIDEO_PROFILE));
        let slow_threshold_us = get(PROFILE_SLOW_US)
            .and_then(|value| value.parse().ok())
            .unwrap_or(FRAME_BUDGET_US);
        let out_path = get(PROFILE_FILE)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let trace_path = get(TRACE_FILE)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                (mode == ProfileMode::Trace).then(|| "/tmp/mister-frame-trace.json".to_owned())
            });
        Self {
            mode,
            slow_threshold_us,
            out_path,
            trace_path,
        }
    }

    pub fn fps_log_enabled(&self) -> bool {
        self.mode != ProfileMode::Off
    }
}

pub struct FrameProfiler {
    mode: ProfileMode,
    slow_threshold_us: u64,
    out_path: Option<String>,
    trace_path: Option<String>,
    frames: Vec<FrameSample>,
    // rolling 1s window for live line (same format as before)
    window_start: Instant,
    window_frames: u64,
    window_prepare: u128,
    window_anim: u128,
    window_slint_render: u128,
    window_custom_draw: u128,
    window_vsync: u128,
    window_fb_present: u128,
    window_cached_present: u128,
    window_overlay_present: u128,
    window_rows: u128,
    window_vsync_hits: u64,
    window_vsync_timeouts: u64,
    window_fallback_frames: u64,
    window_vsync_errors: u64,
    window_video_decode: u128,
    window_video_scale: u128,
    window_video_recv: u128,
    window_video_image: u128,
    window_video_blit: u128,
    window_audio_decode: u128,
    window_audio_resample: u128,
    window_audio_write: u128,
    window_video_updates: u64,
    window_video_missed_deadlines: u64,
    window_audio_underruns: u64,
}

impl FrameProfiler {
    pub fn from_config(config: FrameProfilerConfig) -> Self {
        let FrameProfilerConfig {
            mode,
            slow_threshold_us,
            out_path,
            trace_path,
        } = config;
        if mode != ProfileMode::Off {
            crate::ui_logln!(
                "frame_profile: mode={:?} slow_threshold_us={slow_threshold_us}{}{}",
                mode,
                out_path
                    .as_ref()
                    .map(|p| format!(" file={p}"))
                    .unwrap_or_default(),
                trace_path
                    .as_ref()
                    .map(|p| format!(" trace={p}"))
                    .unwrap_or_default()
            );
        }
        Self {
            mode,
            slow_threshold_us,
            out_path,
            trace_path,
            frames: Vec::new(),
            window_start: Instant::now(),
            window_frames: 0,
            window_prepare: 0,
            window_anim: 0,
            window_slint_render: 0,
            window_custom_draw: 0,
            window_vsync: 0,
            window_fb_present: 0,
            window_cached_present: 0,
            window_overlay_present: 0,
            window_rows: 0,
            window_vsync_hits: 0,
            window_vsync_timeouts: 0,
            window_fallback_frames: 0,
            window_vsync_errors: 0,
            window_video_decode: 0,
            window_video_scale: 0,
            window_video_recv: 0,
            window_video_image: 0,
            window_video_blit: 0,
            window_audio_decode: 0,
            window_audio_resample: 0,
            window_audio_write: 0,
            window_video_updates: 0,
            window_video_missed_deadlines: 0,
            window_audio_underruns: 0,
        }
    }

    pub fn enabled(&self) -> bool {
        self.mode != ProfileMode::Off
    }

    pub fn record(&mut self, sample: FrameSample) {
        if self.mode == ProfileMode::Off {
            return;
        }

        let total = sample.wall_us;
        match self.mode {
            ProfileMode::Slow if total >= self.slow_threshold_us => {
                crate::ui_logln!(
                    "  SLOW frame {}: wall={total}us phases={} prepare={} anim={} slint-render={} custom-draw={} vsync={} fb-present={} cached-present={} arcade-list-present={} rows={} dominant={}",
                    self.frames.len(),
                    sample.phases_us(),
                    sample.prepare_us,
                    sample.anim_us,
                    sample.slint_render_us,
                    sample.custom_draw_us,
                    sample.vsync_us,
                    sample.fb_present_us,
                    sample.cached_present_us,
                    sample.arcade_list_present_us,
                    sample.rows,
                    sample.dominant_phase()
                );
            }
            ProfileMode::Full => {
                crate::ui_logln!(
                    "  frame {}: wall={total}us phases={} prepare={} anim={} slint-render={} custom-draw={} vsync={} fb-present={} cached-present={} arcade-list-present={} rows={}",
                    self.frames.len(),
                    sample.phases_us(),
                    sample.prepare_us,
                    sample.anim_us,
                    sample.slint_render_us,
                    sample.custom_draw_us,
                    sample.vsync_us,
                    sample.fb_present_us,
                    sample.cached_present_us,
                    sample.arcade_list_present_us,
                    sample.rows
                );
            }
            ProfileMode::Trace => {}
            _ => {}
        }

        self.window_frames += 1;
        self.window_prepare += sample.prepare_us as u128;
        self.window_anim += sample.anim_us as u128;
        self.window_slint_render += sample.slint_render_us as u128;
        self.window_custom_draw += sample.custom_draw_us as u128;
        self.window_vsync += sample.vsync_us as u128;
        self.window_fb_present += sample.fb_present_us as u128;
        self.window_cached_present += sample.cached_present_us as u128;
        self.window_overlay_present += sample.arcade_list_present_us as u128;
        self.window_rows += sample.rows as u128;
        self.window_video_decode += sample.video.video_decode_us as u128;
        self.window_video_scale += sample.video.video_scale_us as u128;
        self.window_video_recv += sample.video.video_recv_us as u128;
        self.window_video_image += sample.video.video_image_us as u128;
        self.window_video_blit += sample.video.video_blit_us as u128;
        self.window_audio_decode += sample.video.audio_decode_us as u128;
        self.window_audio_resample += sample.video.audio_resample_us as u128;
        self.window_audio_write += sample.video.audio_write_us as u128;
        if sample.video.video_frame_updated {
            self.window_video_updates += 1;
        }
        self.window_video_missed_deadlines += u64::from(sample.video.video_missed_deadlines);
        if sample.video.audio_underrun {
            self.window_audio_underruns += 1;
        }
        match sample.vsync_source {
            VsyncPaceSource::Vsync => self.window_vsync_hits += 1,
            VsyncPaceSource::Timeout => {
                self.window_vsync_timeouts += 1;
                self.window_fallback_frames += 1;
            }
            VsyncPaceSource::Fallback => self.window_fallback_frames += 1,
            VsyncPaceSource::Error => {
                self.window_vsync_errors += 1;
                self.window_fallback_frames += 1;
            }
        }

        if self.window_start.elapsed().as_millis() >= 1000 {
            self.flush_window();
        }
        self.frames.push(sample);
    }

    fn flush_window(&mut self) {
        let nn = self.window_frames.max(1) as u128;
        crate::ui_logln!(
            "  fps ~ {}  | prepare {}us  anim {}us  slint-render {}us  custom-draw {}us  vsync-wait {}us  fb-present {}us cached-present {}us arcade-list-present {}us ({} logical rows avg)  vsync hits={} timeouts={} fallback={} errors={}",
            self.window_frames,
            self.window_prepare / nn,
            self.window_anim / nn,
            self.window_slint_render / nn,
            self.window_custom_draw / nn,
            self.window_vsync / nn,
            self.window_fb_present / nn,
            self.window_cached_present / nn,
            self.window_overlay_present / nn,
            self.window_rows / nn,
            self.window_vsync_hits,
            self.window_vsync_timeouts,
            self.window_fallback_frames,
            self.window_vsync_errors
        );
        if self.window_video_decode
            + self.window_video_scale
            + self.window_video_recv
            + self.window_video_image
            + self.window_video_blit
            + self.window_audio_decode
            + self.window_audio_resample
            + self.window_audio_write
            > 0
        {
            crate::ui_logln!(
                "  video-profile | updates {} missed-deadlines {} decode {}us scale {}us recv {}us image {}us blit {}us audio-decode {}us audio-resample {}us audio-write {}us underruns {}",
                self.window_video_updates,
                self.window_video_missed_deadlines,
                self.window_video_decode / nn,
                self.window_video_scale / nn,
                self.window_video_recv / nn,
                self.window_video_image / nn,
                self.window_video_blit / nn,
                self.window_audio_decode / nn,
                self.window_audio_resample / nn,
                self.window_audio_write / nn,
                self.window_audio_underruns
            );
        }
        self.window_frames = 0;
        self.window_prepare = 0;
        self.window_anim = 0;
        self.window_slint_render = 0;
        self.window_custom_draw = 0;
        self.window_vsync = 0;
        self.window_fb_present = 0;
        self.window_cached_present = 0;
        self.window_overlay_present = 0;
        self.window_rows = 0;
        self.window_vsync_hits = 0;
        self.window_vsync_timeouts = 0;
        self.window_fallback_frames = 0;
        self.window_vsync_errors = 0;
        self.window_video_decode = 0;
        self.window_video_scale = 0;
        self.window_video_recv = 0;
        self.window_video_image = 0;
        self.window_video_blit = 0;
        self.window_audio_decode = 0;
        self.window_audio_resample = 0;
        self.window_audio_write = 0;
        self.window_video_updates = 0;
        self.window_video_missed_deadlines = 0;
        self.window_audio_underruns = 0;
        self.window_start = Instant::now();
    }

    pub fn finish(mut self) {
        if self.mode == ProfileMode::Off {
            return;
        }
        if self.window_frames > 0 {
            self.flush_window();
        }
        if self.frames.is_empty() {
            crate::ui_logln!("frame_profile: no frames recorded");
            return;
        }
        if let Some(path) = &self.out_path {
            if let Err(e) = self.write_tsv(path) {
                crate::ui_errln!("frame_profile: failed to write {path}: {e}");
            } else {
                crate::ui_logln!(
                    "frame_profile: wrote {} frames to {path}",
                    self.frames.len()
                );
            }
        }
        if let Some(path) = &self.trace_path {
            if let Err(e) = self.write_trace(path) {
                crate::ui_errln!("frame_profile: failed to write trace {path}: {e}");
            } else {
                crate::ui_logln!(
                    "frame_profile: wrote Chrome trace for {} frames to {path}",
                    self.frames.len()
                );
            }
        }
        self.print_summary();
    }

    fn write_tsv(&self, path: &str) -> std::io::Result<()> {
        let mut f = File::create(path)?;
        writeln!(
            f,
            "frame\tprepare_us\tanim_us\tslint_render_us\tcustom_draw_us\tvsync_us\tfb_present_us\tcached_present_us\tarcade_list_present_us\tphases_us\twall_us\trows\tpresent_x0\tpresent_y0\tpresent_x1\tpresent_y1\tpresent_pixels\tpresent_bytes\tvsync_source\tvsync_period_us\tvsync_miss_streak\tvideo_decode_us\tvideo_scale_us\tvideo_recv_us\tvideo_image_us\tvideo_blit_us\taudio_decode_us\taudio_resample_us\taudio_write_us\tvideo_frame_updated\tvideo_queue_depth\taudio_buffer_frames\taudio_underrun\tvideo_file\tvideo_width\tvideo_height\tvideo_present_width\tvideo_present_height\tvideo_size_animating\tvideo_missed_deadlines\tvideo_codec\taudio_codec\tdominant"
        )?;
        for (i, s) in self.frames.iter().enumerate() {
            let (x0, y0, x1, y1, pixels) = s
                .present_rect
                .map(|rect| (rect.x0, rect.y0, rect.x1, rect.y1, rect.pixels()))
                .unwrap_or((0, 0, 0, 0, 0));
            writeln!(
                f,
                "{i}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                s.prepare_us,
                s.anim_us,
                s.slint_render_us,
                s.custom_draw_us,
                s.vsync_us,
                s.fb_present_us,
                s.cached_present_us,
                s.arcade_list_present_us,
                s.phases_us(),
                s.wall_us,
                s.rows,
                x0,
                y0,
                x1,
                y1,
                pixels,
                present_bytes_for_pixels(pixels),
                s.vsync_source.label(),
                s.vsync_period_us,
                s.vsync_miss_streak,
                s.video.video_decode_us,
                s.video.video_scale_us,
                s.video.video_recv_us,
                s.video.video_image_us,
                s.video.video_blit_us,
                s.video.audio_decode_us,
                s.video.audio_resample_us,
                s.video.audio_write_us,
                s.video.video_frame_updated as u8,
                s.video.video_queue_depth,
                s.video.audio_buffer_frames,
                s.video.audio_underrun as u8,
                tsv_escape(&s.video.video_file),
                s.video.video_width,
                s.video.video_height,
                s.video.video_present_width,
                s.video.video_present_height,
                s.video.video_size_animating as u8,
                s.video.video_missed_deadlines,
                tsv_escape(&s.video.video_codec),
                tsv_escape(&s.video.audio_codec),
                s.dominant_phase()
            )?;
        }
        Ok(())
    }

    fn write_trace(&self, path: &str) -> std::io::Result<()> {
        let mut f = File::create(path)?;
        writeln!(f, "{{\"traceEvents\":[")?;
        let mut first = true;
        let mut frame_ts = 0u64;
        for (i, s) in self.frames.iter().enumerate() {
            write_trace_event(&mut f, &mut first, "frame", i, frame_ts, s.wall_us, Some(s))?;
            let mut phase_ts = frame_ts;
            for (name, dur) in [
                ("prepare", s.prepare_us),
                ("anim", s.anim_us),
                ("slint-render", s.slint_render_us),
                ("custom-draw", s.custom_draw_us),
                ("vsync-wait", s.vsync_us),
                ("fb-present", s.fb_present_us),
            ] {
                if dur > 0 {
                    write_trace_event(&mut f, &mut first, name, i, phase_ts, dur, Some(s))?;
                }
                phase_ts = phase_ts.saturating_add(dur);
            }
            let present_ts = phase_ts.saturating_sub(s.fb_present_us);
            if s.cached_present_us > 0 {
                write_trace_event(
                    &mut f,
                    &mut first,
                    "cached-present",
                    i,
                    present_ts,
                    s.cached_present_us,
                    Some(s),
                )?;
            }
            if s.arcade_list_present_us > 0 {
                write_trace_event(
                    &mut f,
                    &mut first,
                    "arcade-list-present",
                    i,
                    present_ts.saturating_add(s.cached_present_us),
                    s.arcade_list_present_us,
                    Some(s),
                )?;
            }
            frame_ts = frame_ts.saturating_add(s.wall_us);
        }
        writeln!(f, "\n]}}")?;
        Ok(())
    }

    fn print_summary(&self) {
        let n = self.frames.len();
        let mut totals: Vec<u64> = self.frames.iter().map(|s| s.wall_us).collect();
        totals.sort_unstable();
        let mut phases: Vec<u64> = self.frames.iter().map(|s| s.phases_us()).collect();
        phases.sort_unstable();

        let over = totals
            .iter()
            .filter(|&&t| t >= self.slow_threshold_us)
            .count();
        let over_pct = (over as f64) * 100.0 / n as f64;

        crate::ui_logln!("=== frame profile summary ({n} frames) ===");
        print_phase_stats("wall", &totals);
        print_phase_stats("phases_sum", &phases);
        print_phase_stats("prepare", &col(&self.frames, |s| s.prepare_us));
        print_phase_stats("anim", &col(&self.frames, |s| s.anim_us));
        print_phase_stats("slint-render", &col(&self.frames, |s| s.slint_render_us));
        print_phase_stats("custom-draw", &col(&self.frames, |s| s.custom_draw_us));
        print_phase_stats("vsync", &col(&self.frames, |s| s.vsync_us));
        print_phase_stats("fb-present", &col(&self.frames, |s| s.fb_present_us));
        print_phase_stats(
            "cached-present",
            &col(&self.frames, |s| s.cached_present_us),
        );
        print_phase_stats(
            "arcade-list-present",
            &col(&self.frames, |s| s.arcade_list_present_us),
        );
        self.print_video_profile_summary();
        self.print_present_bandwidth();
        let hits = self
            .frames
            .iter()
            .filter(|s| s.vsync_source == VsyncPaceSource::Vsync)
            .count();
        let timeouts = self
            .frames
            .iter()
            .filter(|s| s.vsync_source == VsyncPaceSource::Timeout)
            .count();
        let fallback = self
            .frames
            .iter()
            .filter(|s| s.vsync_source == VsyncPaceSource::Fallback)
            .count();
        let errors = self
            .frames
            .iter()
            .filter(|s| s.vsync_source == VsyncPaceSource::Error)
            .count();
        let max_miss_streak = self
            .frames
            .iter()
            .map(|s| s.vsync_miss_streak)
            .max()
            .unwrap_or(0);
        let inferred_hz = self
            .frames
            .iter()
            .rev()
            .find(|s| s.vsync_period_us > 0)
            .map(|s| 1_000_000.0 / s.vsync_period_us as f64)
            .unwrap_or(0.0);
        crate::ui_logln!(
            "vsync: hits={hits} timeouts={timeouts} fallback_frames={fallback} errors={errors} max_miss_streak={max_miss_streak} inferred_hz={inferred_hz:.2}"
        );

        crate::ui_logln!(
            "frames >= {}us ({} Hz budget): {over} ({over_pct:.2}%)",
            self.slow_threshold_us,
            1_000_000 / FRAME_BUDGET_US
        );

        let mut slow_by_phase = [0usize; 6];
        for s in &self.frames {
            if s.wall_us >= self.slow_threshold_us {
                match s.dominant_phase() {
                    "prepare" => slow_by_phase[0] += 1,
                    "anim" => slow_by_phase[1] += 1,
                    "slint-render" => slow_by_phase[2] += 1,
                    "custom-draw" => slow_by_phase[3] += 1,
                    "vsync" | "fb-present" => slow_by_phase[4] += 1,
                    phase if phase.starts_with("video-") || phase.starts_with("audio-") => {
                        slow_by_phase[5] += 1
                    }
                    _ => {}
                }
            }
        }
        crate::ui_logln!(
            "slow-frame dominant phase: prepare={} anim={} slint-render={} custom-draw={} vsync-or-fb-present={} video-or-audio={}",
            slow_by_phase[0],
            slow_by_phase[1],
            slow_by_phase[2],
            slow_by_phase[3],
            slow_by_phase[4],
            slow_by_phase[5]
        );

        print_histogram("wall_ms", &totals, MS_BUCKETS);
        print_histogram(
            "fb_present_us",
            &col(&self.frames, |s| s.fb_present_us),
            US_BUCKETS,
        );

        crate::ui_logln!("worst 10 frames (by wall_us):");
        let mut indexed: Vec<(usize, u64)> = self
            .frames
            .iter()
            .enumerate()
            .map(|(i, s)| (i, s.wall_us))
            .collect();
        indexed.sort_by_key(|&(_, t)| std::cmp::Reverse(t));
        for (i, total) in indexed.into_iter().take(10) {
            let s = &self.frames[i];
            crate::ui_logln!(
                "  #{i} wall={total}us phases={} prepare={} anim={} slint-render={} custom-draw={} vsync={} fb-present={} cached-present={} arcade-list-present={} rows={} dominant={}",
                s.phases_us(),
                s.prepare_us,
                s.anim_us,
                s.slint_render_us,
                s.custom_draw_us,
                s.vsync_us,
                s.fb_present_us,
                s.cached_present_us,
                s.arcade_list_present_us,
                s.rows,
                s.dominant_phase()
            );
        }
    }

    fn print_video_profile_summary(&self) {
        if !self.frames.iter().any(|s| s.video.video_frame_updated) {
            return;
        }
        crate::ui_logln!("video profile summary:");
        print_phase_stats(
            "video-decode",
            &col(&self.frames, |s| s.video.video_decode_us),
        );
        print_phase_stats(
            "video-scale",
            &col(&self.frames, |s| s.video.video_scale_us),
        );
        print_phase_stats("video-recv", &col(&self.frames, |s| s.video.video_recv_us));
        print_phase_stats(
            "video-image",
            &col(&self.frames, |s| s.video.video_image_us),
        );
        print_phase_stats("video-blit", &col(&self.frames, |s| s.video.video_blit_us));
        print_phase_stats(
            "audio-decode",
            &col(&self.frames, |s| s.video.audio_decode_us),
        );
        print_phase_stats(
            "audio-resample",
            &col(&self.frames, |s| s.video.audio_resample_us),
        );
        print_phase_stats(
            "audio-write",
            &col(&self.frames, |s| s.video.audio_write_us),
        );
        let updates = self
            .frames
            .iter()
            .filter(|s| s.video.video_frame_updated)
            .count();
        let underruns = self
            .frames
            .iter()
            .filter(|s| s.video.audio_underrun)
            .count();
        let missed_deadlines: u64 = self
            .frames
            .iter()
            .map(|s| u64::from(s.video.video_missed_deadlines))
            .sum();
        crate::ui_logln!(
            "video-updates={updates} video-missed-deadlines={missed_deadlines} audio-underrun-frames={underruns}"
        );
    }

    fn print_present_bandwidth(&self) {
        let mut presented_frames = 0u64;
        let mut total_bytes = 0u64;
        let mut total_present_us = 0u64;
        let mut max_bytes = 0u64;
        for s in &self.frames {
            let Some(rect) = s.present_rect else {
                continue;
            };
            let bytes = present_bytes_for_pixels(rect.pixels());
            if bytes == 0 {
                continue;
            }
            presented_frames += 1;
            total_bytes += bytes;
            total_present_us += s.fb_present_us;
            max_bytes = max_bytes.max(bytes);
        }
        if presented_frames == 0 || total_present_us == 0 {
            crate::ui_logln!("present-bandwidth: no presented rects");
            return;
        }
        let avg_bytes = total_bytes / presented_frames;
        let mib_per_s =
            (total_bytes as f64 / 1_048_576.0) / (total_present_us as f64 / 1_000_000.0);
        crate::ui_logln!(
            "present-bandwidth: frames={presented_frames} avg_bytes={} max_bytes={} total_bytes={} active_copy_mib_s={mib_per_s:.1}",
            avg_bytes,
            max_bytes,
            total_bytes
        );
    }
}

fn col<F: Fn(&FrameSample) -> u64>(frames: &[FrameSample], f: F) -> Vec<u64> {
    let mut v: Vec<u64> = frames.iter().map(f).collect();
    v.sort_unstable();
    v
}

fn tsv_escape(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

fn write_trace_event(
    f: &mut File,
    first: &mut bool,
    name: &str,
    frame: usize,
    ts: u64,
    dur: u64,
    sample: Option<&FrameSample>,
) -> std::io::Result<()> {
    if !*first {
        writeln!(f, ",")?;
    }
    *first = false;
    let (pixels, bytes) = sample
        .and_then(|s| s.present_rect.map(|rect| rect.pixels()))
        .map(|pixels| (pixels, present_bytes_for_pixels(pixels)))
        .unwrap_or((0, 0));
    let (rows, dominant, video_decode, video_scale, audio_decode, audio_resample, audio_write) =
        sample
            .map(|s| {
                (
                    s.rows,
                    s.dominant_phase(),
                    s.video.video_decode_us,
                    s.video.video_scale_us,
                    s.video.audio_decode_us,
                    s.video.audio_resample_us,
                    s.video.audio_write_us,
                )
            })
            .unwrap_or((0, "", 0, 0, 0, 0, 0));
    write!(
        f,
        "{{\"name\":\"{}\",\"cat\":\"frame\",\"ph\":\"X\",\"ts\":{},\"dur\":{},\"pid\":1,\"tid\":{},\"args\":{{\"frame\":{},\"rows\":{},\"present_pixels\":{},\"present_bytes\":{},\"dominant\":\"{}\",\"video_decode_us\":{},\"video_scale_us\":{},\"audio_decode_us\":{},\"audio_resample_us\":{},\"audio_write_us\":{}}}}}",
        name,
        ts,
        dur,
        frame,
        frame,
        rows,
        pixels,
        bytes,
        dominant,
        video_decode,
        video_scale,
        audio_decode,
        audio_resample,
        audio_write
    )
}

fn percentile(sorted: &[u64], pct: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() - 1) as f64 * pct / 100.0).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn print_phase_stats(label: &str, sorted: &[u64]) {
    if sorted.is_empty() {
        return;
    }
    crate::ui_logln!(
        "  {label}: min={} p50={} p95={} p99={} max={} avg={}",
        sorted[0],
        percentile(sorted, 50.0),
        percentile(sorted, 95.0),
        percentile(sorted, 99.0),
        sorted[sorted.len() - 1],
        sorted.iter().sum::<u64>() / sorted.len() as u64
    );
}

const MS_BUCKETS: &[(u64, u64, &str)] = &[
    (0, 12_000, "[ 0, 12ms)"),
    (12_000, 14_000, "[12, 14ms)"),
    (14_000, 16_000, "[14, 16ms)"),
    (16_000, 17_000, "[16, 17ms)"),
    (17_000, 20_000, "[17, 20ms)"),
    (20_000, 30_000, "[20, 30ms)"),
    (30_000, u64::MAX, "[30ms,   )"),
];

const US_BUCKETS: &[(u64, u64, &str)] = &[
    (0, 2_000, "[   0, 2ms)"),
    (2_000, 5_000, "[ 2ms, 5ms)"),
    (5_000, 8_000, "[ 5ms, 8ms)"),
    (8_000, 10_000, "[ 8ms,10ms)"),
    (10_000, 12_000, "[10ms,12ms)"),
    (12_000, 16_000, "[12ms,16ms)"),
    (16_000, u64::MAX, "[16ms,   )"),
];

fn print_histogram(label: &str, sorted: &[u64], buckets: &[(u64, u64, &str)]) {
    crate::ui_log!("  {label} histogram:");
    for &(lo, hi, name) in buckets {
        let c = sorted.iter().filter(|&&v| v >= lo && v < hi).count();
        if c > 0 {
            crate::ui_log!(" {name}={c}");
        }
    }
    crate::ui_logln!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_bytes_use_rgb565_pixel_width() {
        let rect = FrameRect {
            x0: 10,
            y0: 20,
            x1: 14,
            y1: 23,
        };

        assert_eq!(rect.pixels(), 12);
        assert_eq!(present_bytes_for_pixels(rect.pixels()), 24);
    }

    #[test]
    fn profiler_config_owns_captured_values() {
        let mut values = std::collections::BTreeMap::from([
            (PROFILE, "trace"),
            (PROFILE_SLOW_US, "12345"),
            (PROFILE_FILE, "/tmp/frame-profile.json"),
        ]);
        let config = FrameProfilerConfig::capture_with(|name| values.get(name).copied());
        values.insert(PROFILE, "off");

        assert_eq!(config.mode, ProfileMode::Trace);
        assert_eq!(config.slow_threshold_us, 12_345);
        assert_eq!(config.out_path.as_deref(), Some("/tmp/frame-profile.json"));
        assert_eq!(
            config.trace_path.as_deref(),
            Some("/tmp/mister-frame-trace.json")
        );
    }
}
