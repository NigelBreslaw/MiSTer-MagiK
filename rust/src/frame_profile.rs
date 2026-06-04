//! Per-frame render-loop instrumentation (env `MISTER_PROFILE`).
//!
//! Records anim / render / vsync / copy timings every frame and prints a factual
//! summary at exit — percentiles, histogram, slow-frame breakdown by phase.

use std::fs::File;
use std::io::Write;
use std::time::Instant;

const FRAME_BUDGET_US: u64 = 16_667; // 60 Hz

#[derive(Clone, Copy, Debug)]
pub struct FrameSample {
    pub anim_us: u64,
    pub render_us: u64,
    pub vsync_us: u64,
    pub copy_us: u64,
    pub rows: u32,
    pub wall_us: u64,
}

impl FrameSample {
    pub fn phases_us(self) -> u64 {
        self.anim_us + self.render_us + self.vsync_us + self.copy_us
    }

    pub fn total_us(self) -> u64 {
        self.wall_us
    }

    fn dominant_phase(self) -> &'static str {
        let m = self
            .anim_us
            .max(self.render_us)
            .max(self.vsync_us)
            .max(self.copy_us);
        if m == self.copy_us {
            "copy"
        } else if m == self.render_us {
            "render"
        } else if m == self.vsync_us {
            "vsync"
        } else {
            "anim"
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileMode {
    Off,
    Summary,
    Slow,
    Full,
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
            other => {
                eprintln!(
                    "frame_profile: unknown MISTER_PROFILE={other:?}; use 1|summary|slow|full"
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
    frames: Vec<FrameSample>,
    // rolling 1s window for live line (same format as before)
    window_start: Instant,
    window_frames: u64,
    window_anim: u128,
    window_render: u128,
    window_vsync: u128,
    window_copy: u128,
    window_rows: u128,
}

impl FrameProfiler {
    pub fn from_env() -> Self {
        let mode = ProfileMode::from_env();
        let slow_threshold_us = std::env::var("MISTER_PROFILE_SLOW_US")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(FRAME_BUDGET_US);
        let out_path = std::env::var("MISTER_PROFILE_FILE").ok().filter(|s| !s.is_empty());
        if mode != ProfileMode::Off {
            println!(
                "frame_profile: mode={:?} slow_threshold_us={slow_threshold_us}{}",
                mode,
                out_path
                    .as_ref()
                    .map(|p| format!(" file={p}"))
                    .unwrap_or_default()
            );
        }
        Self {
            mode,
            slow_threshold_us,
            out_path,
            frames: Vec::new(),
            window_start: Instant::now(),
            window_frames: 0,
            window_anim: 0,
            window_render: 0,
            window_vsync: 0,
            window_copy: 0,
            window_rows: 0,
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
                    "  SLOW frame {}: wall={total}us phases={} anim={} render={} vsync={} copy={} rows={} dominant={}",
                    self.frames.len(),
                    sample.phases_us(),
                    sample.anim_us,
                    sample.render_us,
                    sample.vsync_us,
                    sample.copy_us,
                    sample.rows,
                    sample.dominant_phase()
                );
            }
            ProfileMode::Full => {
                println!(
                    "  frame {}: wall={total}us phases={} anim={} render={} vsync={} copy={} rows={}",
                    self.frames.len(),
                    sample.phases_us(),
                    sample.anim_us,
                    sample.render_us,
                    sample.vsync_us,
                    sample.copy_us,
                    sample.rows
                );
            }
            _ => {}
        }

        self.frames.push(sample);
        self.window_frames += 1;
        self.window_anim += sample.anim_us as u128;
        self.window_render += sample.render_us as u128;
        self.window_vsync += sample.vsync_us as u128;
        self.window_copy += sample.copy_us as u128;
        self.window_rows += sample.rows as u128;

        if self.window_start.elapsed().as_millis() >= 1000 {
            self.flush_window();
        }
    }

    fn flush_window(&mut self) {
        let nn = self.window_frames.max(1) as u128;
        println!(
            "  fps ~ {}  | anim {}us  render {}us  vsync-wait {}us  copy {}us ({} logical rows avg)",
            self.window_frames,
            self.window_anim / nn,
            self.window_render / nn,
            self.window_vsync / nn,
            self.window_copy / nn,
            self.window_rows / nn
        );
        self.window_frames = 0;
        self.window_anim = 0;
        self.window_render = 0;
        self.window_vsync = 0;
        self.window_copy = 0;
        self.window_rows = 0;
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
                println!("frame_profile: wrote {} frames to {path}", self.frames.len());
            }
        }
        self.print_summary();
    }

    fn write_tsv(&self, path: &str) -> std::io::Result<()> {
        let mut f = File::create(path)?;
        writeln!(
            f,
            "frame\tanim_us\trender_us\tvsync_us\tcopy_us\tphases_us\twall_us\trows\tdominant"
        )?;
        for (i, s) in self.frames.iter().enumerate() {
            writeln!(
                f,
                "{i}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                s.anim_us,
                s.render_us,
                s.vsync_us,
                s.copy_us,
                s.phases_us(),
                s.wall_us,
                s.rows,
                s.dominant_phase()
            )?;
        }
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
        print_phase_stats("anim", &col(&self.frames, |s| s.anim_us));
        print_phase_stats("render", &col(&self.frames, |s| s.render_us));
        print_phase_stats("vsync", &col(&self.frames, |s| s.vsync_us));
        print_phase_stats("copy", &col(&self.frames, |s| s.copy_us));

        println!(
            "frames >= {}us ({} Hz budget): {over} ({over_pct:.2}%)",
            self.slow_threshold_us,
            1_000_000 / FRAME_BUDGET_US
        );

        let mut slow_by_phase = [0usize; 4];
        for s in &self.frames {
            if s.wall_us >= self.slow_threshold_us {
                match s.dominant_phase() {
                    "anim" => slow_by_phase[0] += 1,
                    "render" => slow_by_phase[1] += 1,
                    "vsync" => slow_by_phase[2] += 1,
                    "copy" => slow_by_phase[3] += 1,
                    _ => {}
                }
            }
        }
        println!(
            "slow-frame dominant phase: anim={} render={} vsync={} copy={}",
            slow_by_phase[0], slow_by_phase[1], slow_by_phase[2], slow_by_phase[3]
        );

        print_histogram("wall_ms", &totals, MS_BUCKETS);
        print_histogram("copy_us", &col(&self.frames, |s| s.copy_us), US_BUCKETS);

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
                "  #{i} wall={total}us phases={} anim={} render={} vsync={} copy={} rows={} dominant={}",
                s.phases_us(),
                s.anim_us,
                s.render_us,
                s.vsync_us,
                s.copy_us,
                s.rows,
                s.dominant_phase()
            );
        }
    }
}

fn col<F: Fn(&FrameSample) -> u64>(frames: &[FrameSample], f: F) -> Vec<u64> {
    let mut v: Vec<u64> = frames.iter().map(f).collect();
    v.sort_unstable();
    v
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
