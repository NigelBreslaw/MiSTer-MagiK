//! Per-frame render-loop instrumentation (env `MISTER_PROFILE`).
//!
//! Records prepare / anim / slint-render / custom-draw / vsync / fb-present timings every frame and prints a factual
//! summary at exit — percentiles, histogram, slow-frame breakdown by phase.

use std::fs::File;
use std::io::Write;
use std::time::Instant;

use mister_magik_fb::vsync_pacer::VsyncPaceSource;

const FRAME_BUDGET_US: u64 = 16_667; // 60 Hz

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

#[derive(Clone, Copy, Debug)]
pub struct FrameSample {
    pub prepare_us: u64,
    pub anim_us: u64,
    pub slint_render_us: u64,
    pub custom_draw_us: u64,
    pub vsync_us: u64,
    pub fb_present_us: u64,
    pub cached_present_us: u64,
    pub overlay_present_us: u64,
    pub rows: u32,
    pub present_rect: Option<FrameRect>,
    pub wall_us: u64,
    pub vsync_source: VsyncPaceSource,
    pub vsync_period_us: u64,
    pub vsync_miss_streak: u32,
}

impl FrameSample {
    pub fn phases_us(self) -> u64 {
        self.prepare_us
            + self.anim_us
            + self.slint_render_us
            + self.custom_draw_us
            + self.vsync_us
            + self.fb_present_us
    }

    fn dominant_phase(self) -> &'static str {
        let m = self
            .prepare_us
            .max(self.anim_us)
            .max(self.slint_render_us)
            .max(self.custom_draw_us)
            .max(self.vsync_us)
            .max(self.fb_present_us);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileMode {
    Off,
    Summary,
    Slow,
    Full,
    Trace,
}

impl ProfileMode {
    pub fn from_env() -> Self {
        match std::env::var("MISTER_PROFILE")
            .ok()
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            None | Some("") | Some("0") | Some("false") => Self::Off,
            Some("1") | Some("true") | Some("summary") => Self::Summary,
            Some("slow") => Self::Slow,
            Some("full") => Self::Full,
            Some("trace") => Self::Trace,
            other => {
                eprintln!(
                    "frame_profile: unknown MISTER_PROFILE={other:?}; use 1|summary|slow|full|trace"
                );
                Self::Summary
            }
        }
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
}

impl FrameProfiler {
    pub fn from_env() -> Self {
        let mode = ProfileMode::from_env();
        let slow_threshold_us = std::env::var("MISTER_PROFILE_SLOW_US")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(FRAME_BUDGET_US);
        let out_path = std::env::var("MISTER_PROFILE_FILE")
            .ok()
            .filter(|s| !s.is_empty());
        let trace_path = std::env::var("MISTER_TRACE_FILE")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                (mode == ProfileMode::Trace).then(|| "/tmp/mister-frame-trace.json".to_string())
            });
        if mode != ProfileMode::Off {
            println!(
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
                println!(
                    "  SLOW frame {}: wall={total}us phases={} prepare={} anim={} slint-render={} custom-draw={} vsync={} fb-present={} cached-present={} overlay-present={} rows={} dominant={}",
                    self.frames.len(),
                    sample.phases_us(),
                    sample.prepare_us,
                    sample.anim_us,
                    sample.slint_render_us,
                    sample.custom_draw_us,
                    sample.vsync_us,
                    sample.fb_present_us,
                    sample.cached_present_us,
                    sample.overlay_present_us,
                    sample.rows,
                    sample.dominant_phase()
                );
            }
            ProfileMode::Full => {
                println!(
                    "  frame {}: wall={total}us phases={} prepare={} anim={} slint-render={} custom-draw={} vsync={} fb-present={} cached-present={} overlay-present={} rows={}",
                    self.frames.len(),
                    sample.phases_us(),
                    sample.prepare_us,
                    sample.anim_us,
                    sample.slint_render_us,
                    sample.custom_draw_us,
                    sample.vsync_us,
                    sample.fb_present_us,
                    sample.cached_present_us,
                    sample.overlay_present_us,
                    sample.rows
                );
            }
            ProfileMode::Trace => {}
            _ => {}
        }

        self.frames.push(sample);
        self.window_frames += 1;
        self.window_prepare += sample.prepare_us as u128;
        self.window_anim += sample.anim_us as u128;
        self.window_slint_render += sample.slint_render_us as u128;
        self.window_custom_draw += sample.custom_draw_us as u128;
        self.window_vsync += sample.vsync_us as u128;
        self.window_fb_present += sample.fb_present_us as u128;
        self.window_cached_present += sample.cached_present_us as u128;
        self.window_overlay_present += sample.overlay_present_us as u128;
        self.window_rows += sample.rows as u128;
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
    }

    fn flush_window(&mut self) {
        let nn = self.window_frames.max(1) as u128;
        println!(
            "  fps ~ {}  | prepare {}us  anim {}us  slint-render {}us  custom-draw {}us  vsync-wait {}us  fb-present {}us cached-present {}us overlay-present {}us ({} logical rows avg)  vsync hits={} timeouts={} fallback={} errors={}",
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
            println!("frame_profile: no frames recorded");
            return;
        }
        if let Some(path) = &self.out_path {
            if let Err(e) = self.write_tsv(path) {
                eprintln!("frame_profile: failed to write {path}: {e}");
            } else {
                println!(
                    "frame_profile: wrote {} frames to {path}",
                    self.frames.len()
                );
            }
        }
        if let Some(path) = &self.trace_path {
            if let Err(e) = self.write_trace(path) {
                eprintln!("frame_profile: failed to write trace {path}: {e}");
            } else {
                println!(
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
            "frame\tprepare_us\tanim_us\tslint_render_us\tcustom_draw_us\tvsync_us\tfb_present_us\tcached_present_us\toverlay_present_us\tphases_us\twall_us\trows\tpresent_x0\tpresent_y0\tpresent_x1\tpresent_y1\tpresent_pixels\tpresent_bytes\tvsync_source\tvsync_period_us\tvsync_miss_streak\tdominant"
        )?;
        for (i, s) in self.frames.iter().enumerate() {
            let (x0, y0, x1, y1, pixels) = s
                .present_rect
                .map(|rect| (rect.x0, rect.y0, rect.x1, rect.y1, rect.pixels()))
                .unwrap_or((0, 0, 0, 0, 0));
            writeln!(
                f,
                "{i}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                s.prepare_us,
                s.anim_us,
                s.slint_render_us,
                s.custom_draw_us,
                s.vsync_us,
                s.fb_present_us,
                s.cached_present_us,
                s.overlay_present_us,
                s.phases_us(),
                s.wall_us,
                s.rows,
                x0,
                y0,
                x1,
                y1,
                pixels,
                pixels * 4,
                s.vsync_source.label(),
                s.vsync_period_us,
                s.vsync_miss_streak,
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
            if s.overlay_present_us > 0 {
                write_trace_event(
                    &mut f,
                    &mut first,
                    "overlay-present",
                    i,
                    present_ts.saturating_add(s.cached_present_us),
                    s.overlay_present_us,
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

        println!("=== frame profile summary ({n} frames) ===");
        print_phase_stats("wall", &totals);
        print_phase_stats("phases_sum", &phases);
        print_phase_stats("prepare", &col(&self.frames, |s| s.prepare_us));
        print_phase_stats("anim", &col(&self.frames, |s| s.anim_us));
        print_phase_stats("slint-render", &col(&self.frames, |s| s.slint_render_us));
        print_phase_stats("custom-draw", &col(&self.frames, |s| s.custom_draw_us));
        print_phase_stats("vsync", &col(&self.frames, |s| s.vsync_us));
        print_phase_stats("fb-present", &col(&self.frames, |s| s.fb_present_us));
        print_phase_stats("cached-present", &col(&self.frames, |s| s.cached_present_us));
        print_phase_stats("overlay-present", &col(&self.frames, |s| s.overlay_present_us));
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
        println!(
            "vsync: hits={hits} timeouts={timeouts} fallback_frames={fallback} errors={errors} max_miss_streak={max_miss_streak} inferred_hz={inferred_hz:.2}"
        );

        println!(
            "frames >= {}us ({} Hz budget): {over} ({over_pct:.2}%)",
            self.slow_threshold_us,
            1_000_000 / FRAME_BUDGET_US
        );

        let mut slow_by_phase = [0usize; 5];
        for s in &self.frames {
            if s.wall_us >= self.slow_threshold_us {
                match s.dominant_phase() {
                    "prepare" => slow_by_phase[0] += 1,
                    "anim" => slow_by_phase[1] += 1,
                    "slint-render" => slow_by_phase[2] += 1,
                    "custom-draw" => slow_by_phase[3] += 1,
                    "vsync" | "fb-present" => slow_by_phase[4] += 1,
                    _ => {}
                }
            }
        }
        println!(
            "slow-frame dominant phase: prepare={} anim={} slint-render={} custom-draw={} vsync-or-fb-present={}",
            slow_by_phase[0], slow_by_phase[1], slow_by_phase[2], slow_by_phase[3], slow_by_phase[4]
        );

        print_histogram("wall_ms", &totals, MS_BUCKETS);
        print_histogram("fb_present_us", &col(&self.frames, |s| s.fb_present_us), US_BUCKETS);

        println!("worst 10 frames (by wall_us):");
        let mut indexed: Vec<(usize, u64)> = self
            .frames
            .iter()
            .enumerate()
            .map(|(i, s)| (i, s.wall_us))
            .collect();
        indexed.sort_by_key(|&(_, t)| std::cmp::Reverse(t));
        for (i, total) in indexed.into_iter().take(10) {
            let s = self.frames[i];
            println!(
                "  #{i} wall={total}us phases={} prepare={} anim={} slint-render={} custom-draw={} vsync={} fb-present={} cached-present={} overlay-present={} rows={} dominant={}",
                s.phases_us(),
                s.prepare_us,
                s.anim_us,
                s.slint_render_us,
                s.custom_draw_us,
                s.vsync_us,
                s.fb_present_us,
                s.cached_present_us,
                s.overlay_present_us,
                s.rows,
                s.dominant_phase()
            );
        }
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
            let bytes = rect.pixels() * 4;
            if bytes == 0 {
                continue;
            }
            presented_frames += 1;
            total_bytes += bytes;
            total_present_us += s.fb_present_us;
            max_bytes = max_bytes.max(bytes);
        }
        if presented_frames == 0 || total_present_us == 0 {
            println!("present-bandwidth: no presented rects");
            return;
        }
        let avg_bytes = total_bytes / presented_frames;
        let mib_per_s = (total_bytes as f64 / 1_048_576.0) / (total_present_us as f64 / 1_000_000.0);
        println!(
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
        .map(|pixels| (pixels, pixels * 4))
        .unwrap_or((0, 0));
    let (rows, dominant) = sample
        .map(|s| (s.rows, s.dominant_phase()))
        .unwrap_or((0, ""));
    write!(
        f,
        "{{\"name\":\"{}\",\"cat\":\"frame\",\"ph\":\"X\",\"ts\":{},\"dur\":{},\"pid\":1,\"tid\":{},\"args\":{{\"frame\":{},\"rows\":{},\"present_pixels\":{},\"present_bytes\":{},\"dominant\":\"{}\"}}}}",
        name, ts, dur, frame, frame, rows, pixels, bytes, dominant
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
    println!(
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
    print!("  {label} histogram:");
    for &(lo, hi, name) in buckets {
        let c = sorted.iter().filter(|&&v| v >= lo && v < hi).count();
        if c > 0 {
            print!(" {name}={c}");
        }
    }
    println!();
}
