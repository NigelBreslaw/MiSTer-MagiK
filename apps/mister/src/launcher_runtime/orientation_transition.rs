// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral RGB565 monitor-orientation transition compositor.

use crate::settings::ScreenOrientation;
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::{Duration, Instant};

pub const ORIENTATION_QUARTER_TURN_DURATION: Duration = Duration::from_millis(300);
pub const ORIENTATION_OPPOSITE_TURN_DURATION: Duration = Duration::from_millis(450);
const ORIENTATION_DIALOG_SCALE: f32 = 0.40;
const ORDERED_DITHER_8X8: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46, 6, 38, 60,
    28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27, 49, 17, 57, 25, 15,
    47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrientationTransitionCompletion {
    pub from: ScreenOrientation,
    pub to: ScreenOrientation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OrientationTransitionRenderStats {
    pub fill_us: u64,
    pub map_us: u64,
    pub crossfade_us: u64,
    pub total_us: u64,
    pub mapped_pixels: u64,
    pub blended_pixels: u64,
    pub progress_ppm: u32,
}

#[derive(Clone, Copy)]
pub enum OrientationPmuPhase {
    Destination,
    Fill,
    Map,
    Crossfade,
    CacheRestore,
}

pub const fn orientation_pmu_label(
    from: ScreenOrientation,
    to: ScreenOrientation,
    phase: OrientationPmuPhase,
) -> &'static str {
    const LABELS: [[&str; 5]; 6] = [
        [
            "orientation.normal-clockwise.destination",
            "orientation.normal-clockwise.fill",
            "orientation.normal-clockwise.map",
            "orientation.normal-clockwise.crossfade",
            "orientation.normal-clockwise.cache-restore",
        ],
        [
            "orientation.clockwise-counterclockwise.destination",
            "orientation.clockwise-counterclockwise.fill",
            "orientation.clockwise-counterclockwise.map",
            "orientation.clockwise-counterclockwise.crossfade",
            "orientation.clockwise-counterclockwise.cache-restore",
        ],
        [
            "orientation.counterclockwise-normal.destination",
            "orientation.counterclockwise-normal.fill",
            "orientation.counterclockwise-normal.map",
            "orientation.counterclockwise-normal.crossfade",
            "orientation.counterclockwise-normal.cache-restore",
        ],
        [
            "orientation.normal-counterclockwise.destination",
            "orientation.normal-counterclockwise.fill",
            "orientation.normal-counterclockwise.map",
            "orientation.normal-counterclockwise.crossfade",
            "orientation.normal-counterclockwise.cache-restore",
        ],
        [
            "orientation.counterclockwise-clockwise.destination",
            "orientation.counterclockwise-clockwise.fill",
            "orientation.counterclockwise-clockwise.map",
            "orientation.counterclockwise-clockwise.crossfade",
            "orientation.counterclockwise-clockwise.cache-restore",
        ],
        [
            "orientation.clockwise-normal.destination",
            "orientation.clockwise-normal.fill",
            "orientation.clockwise-normal.map",
            "orientation.clockwise-normal.crossfade",
            "orientation.clockwise-normal.cache-restore",
        ],
    ];
    let leg = match (from, to) {
        (ScreenOrientation::Normal, ScreenOrientation::MonitorClockwise) => 0,
        (ScreenOrientation::MonitorClockwise, ScreenOrientation::MonitorCounterclockwise) => 1,
        (ScreenOrientation::MonitorCounterclockwise, ScreenOrientation::Normal) => 2,
        (ScreenOrientation::Normal, ScreenOrientation::MonitorCounterclockwise) => 3,
        (ScreenOrientation::MonitorCounterclockwise, ScreenOrientation::MonitorClockwise) => 4,
        (ScreenOrientation::MonitorClockwise, ScreenOrientation::Normal) => 5,
        _ => return "orientation.invalid",
    };
    let phase = match phase {
        OrientationPmuPhase::Destination => 0,
        OrientationPmuPhase::Fill => 1,
        OrientationPmuPhase::Map => 2,
        OrientationPmuPhase::Crossfade => 3,
        OrientationPmuPhase::CacheRestore => 4,
    };
    LABELS[leg][phase]
}

pub struct OrientationTransitionRuntime {
    width: usize,
    height: usize,
    from: ScreenOrientation,
    to: ScreenOrientation,
    started_at: Instant,
    duration: Duration,
    source: Vec<Rgb565Pixel>,
    destination: Vec<Rgb565Pixel>,
    output: Vec<Rgb565Pixel>,
    destination_ready: bool,
    active: bool,
    completion: Option<OrientationTransitionCompletion>,
    last_render_stats: OrientationTransitionRenderStats,
}

impl OrientationTransitionRuntime {
    pub fn new(width: usize, height: usize) -> Self {
        let len = width.saturating_mul(height);
        Self {
            width,
            height,
            from: ScreenOrientation::Normal,
            to: ScreenOrientation::Normal,
            started_at: Instant::now(),
            duration: ORIENTATION_QUARTER_TURN_DURATION,
            source: vec![Rgb565Pixel(0); len],
            destination: vec![Rgb565Pixel(0); len],
            output: vec![Rgb565Pixel(0); len],
            destination_ready: false,
            active: false,
            completion: None,
            last_render_stats: OrientationTransitionRenderStats::default(),
        }
    }

    pub fn start(
        &mut self,
        from: ScreenOrientation,
        to: ScreenOrientation,
        source: &[Rgb565Pixel],
        now: Instant,
        reduce_motion: bool,
    ) -> bool {
        if reduce_motion || from == to || source.len() != self.source.len() {
            self.active = false;
            self.completion = Some(OrientationTransitionCompletion { from, to });
            return false;
        }
        self.from = from;
        self.to = to;
        self.started_at = now;
        self.duration = if from.is_portrait() && to.is_portrait() {
            ORIENTATION_OPPOSITE_TURN_DURATION
        } else {
            ORIENTATION_QUARTER_TURN_DURATION
        };
        self.source.copy_from_slice(source);
        self.destination.fill(Rgb565Pixel(0));
        self.output.copy_from_slice(source);
        self.destination_ready = false;
        self.active = true;
        self.completion = None;
        self.last_render_stats = OrientationTransitionRenderStats::default();
        true
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn capture_destination(&mut self, pixels: &[Rgb565Pixel]) -> bool {
        if !self.active || pixels.len() != self.destination.len() {
            return false;
        }
        if self.destination_ready {
            return true;
        }
        self.destination.copy_from_slice(pixels);
        self.destination_ready = true;
        true
    }

    pub fn destination_ready(&self) -> bool {
        self.destination_ready
    }

    pub const fn from(&self) -> ScreenOrientation {
        self.from
    }

    pub const fn to(&self) -> ScreenOrientation {
        self.to
    }

    pub const fn last_render_stats(&self) -> OrientationTransitionRenderStats {
        self.last_render_stats
    }

    pub fn render(
        &mut self,
        now: Instant,
    ) -> Option<(&[Rgb565Pixel], bool, OrientationTransitionRenderStats)> {
        if !self.active {
            return None;
        }
        if !self.destination_ready {
            self.last_render_stats = OrientationTransitionRenderStats::default();
            return Some((&self.source, false, self.last_render_stats));
        }
        let render_started = Instant::now();
        let progress = (now.saturating_duration_since(self.started_at).as_secs_f32()
            / self.duration.as_secs_f32())
        .clamp(0.0, 1.0);
        let fill_started = Instant::now();
        let fill_pmu = mister_magik_perf_events::sampled_span(orientation_pmu_label(
            self.from,
            self.to,
            OrientationPmuPhase::Fill,
        ));
        drop(fill_pmu);
        let fill_us = elapsed_us(fill_started);
        let crossfade_started = Instant::now();
        let crossfade_pmu = mister_magik_perf_events::sampled_span(orientation_pmu_label(
            self.from,
            self.to,
            OrientationPmuPhase::Crossfade,
        ));
        let blended_pixels = render_dithered_background(
            &self.source,
            &self.destination,
            &mut self.output,
            self.width,
            self.height,
            progress,
        );
        drop(crossfade_pmu);
        let crossfade_us = elapsed_us(crossfade_started);
        let (dialog, dialog_turns) = if progress < 0.5 {
            (
                self.source.as_slice(),
                transition_quarter_turns(self.from, self.to) as f32 * progress,
            )
        } else {
            (
                self.destination.as_slice(),
                transition_quarter_turns(self.from, self.to) as f32 * (progress - 1.0),
            )
        };
        let (map_us, mapped_pixels) = render_rotating_dialog(
            dialog,
            &mut self.output,
            self.width,
            self.height,
            dialog_turns,
            progress > 0.0 && progress < 1.0,
            orientation_pmu_label(self.from, self.to, OrientationPmuPhase::Map),
        );
        let done = progress >= 1.0;
        if done {
            self.active = false;
            self.completion = Some(OrientationTransitionCompletion {
                from: self.from,
                to: self.to,
            });
        }
        self.last_render_stats = OrientationTransitionRenderStats {
            fill_us,
            map_us,
            crossfade_us,
            total_us: elapsed_us(render_started),
            mapped_pixels,
            blended_pixels,
            progress_ppm: (progress * 1_000_000.0).round().clamp(0.0, 1_000_000.0) as u32,
        };
        Some((&self.output, done, self.last_render_stats))
    }

    pub fn take_completion(&mut self) -> Option<OrientationTransitionCompletion> {
        self.completion.take()
    }
}

fn orientation_turns(orientation: ScreenOrientation) -> i8 {
    match orientation {
        ScreenOrientation::Normal => 0,
        ScreenOrientation::MonitorClockwise => -1,
        ScreenOrientation::MonitorCounterclockwise => 1,
    }
}

fn transition_quarter_turns(from: ScreenOrientation, to: ScreenOrientation) -> i8 {
    orientation_turns(to) - orientation_turns(from)
}

fn render_dithered_background(
    source: &[Rgb565Pixel],
    destination: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    progress: f32,
) -> u64 {
    if progress <= 0.0 {
        output.copy_from_slice(source);
        return 0;
    }
    if progress >= 1.0 {
        output.copy_from_slice(destination);
        return output.len().min(u64::MAX as usize) as u64;
    }
    let (frame, visible) = if progress < 0.5 {
        (source, 1.0 - progress * 2.0)
    } else {
        (destination, (progress - 0.5) * 2.0)
    };
    let visible_levels = (visible * 64.0).round().clamp(0.0, 64.0) as u8;
    for y in 0..height {
        let row = y * width;
        let dither_row = (y & 7) * 8;
        for x in 0..width {
            output[row + x] = if ORDERED_DITHER_8X8[dither_row + (x & 7)] < visible_levels {
                frame[row + x]
            } else {
                Rgb565Pixel(0)
            };
        }
    }
    output.len().min(u64::MAX as usize) as u64
}

fn render_rotating_dialog(
    source: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    quarter_turns: f32,
    visible: bool,
    map_label: &'static str,
) -> (u64, u64) {
    let map_started = Instant::now();
    let map_pmu = mister_magik_perf_events::sampled_span(map_label);
    if !visible {
        drop(map_pmu);
        return (elapsed_us(map_started), 0);
    }
    let angle = quarter_turns * std::f32::consts::FRAC_PI_2;
    let (sin, cos) = angle.sin_cos();
    let rotated_width = cos.abs() * width as f32 + sin.abs() * height as f32;
    let rotated_height = sin.abs() * width as f32 + cos.abs() * height as f32;
    let scale = (width as f32 / rotated_width.max(1.0))
        .min(height as f32 / rotated_height.max(1.0))
        * ORIENTATION_DIALOG_SCALE;
    let source_cx = (width as f32 - 1.0) * 0.5;
    let source_cy = (height as f32 - 1.0) * 0.5;
    let dialog_width = rotated_width * scale;
    let dialog_height = rotated_height * scale;
    let x0 = (source_cx - dialog_width * 0.5).floor().max(0.0) as usize;
    let y0 = (source_cy - dialog_height * 0.5).floor().max(0.0) as usize;
    let x1 = (source_cx + dialog_width * 0.5).ceil().min(width as f32) as usize;
    let y1 = (source_cy + dialog_height * 0.5).ceil().min(height as f32) as usize;
    let inverse_scale = scale.recip();
    let source_x_step = cos * inverse_scale;
    let source_y_step = -sin * inverse_scale;
    let first_dx = (x0 as f32 - source_cx) * inverse_scale;
    let mut mapped_pixels = 0u64;
    for y in y0..y1 {
        let dy = (y as f32 - source_cy) * inverse_scale;
        let mut source_x = cos * first_dx + sin * dy + source_cx;
        let mut source_y = -sin * first_dx + cos * dy + source_cy;
        let row = y * width;
        for x in x0..x1 {
            if source_x >= 0.0
                && source_y >= 0.0
                && source_x < width as f32
                && source_y < height as f32
            {
                let source_row = ((source_y + 0.5) as usize).min(height - 1) * width;
                let source_column = ((source_x + 0.5) as usize).min(width - 1);
                output[row + x] = source[source_row + source_column];
                mapped_pixels = mapped_pixels.saturating_add(1);
            }
            source_x += source_x_step;
            source_y += source_y_step;
        }
    }
    drop(map_pmu);
    (elapsed_us(map_started), mapped_pixels)
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotating_dialog_maps_less_than_one_quarter_of_the_frame() {
        let width = 128;
        let height = 72;
        let source = vec![Rgb565Pixel(1); width * height];
        for eighth_turns in -16..=16 {
            let mut output = vec![Rgb565Pixel(0); source.len()];
            let (_, mapped) = render_rotating_dialog(
                &source,
                &mut output,
                width,
                height,
                eighth_turns as f32 / 8.0,
                true,
                "orientation.invalid",
            );
            assert!(mapped > 0);
            assert!(mapped < (width * height / 4) as u64, "turn={eighth_turns}");
        }
    }

    #[test]
    fn dithered_background_is_exact_at_endpoints_and_black_at_midpoint() {
        let source = [Rgb565Pixel(1); 64];
        let destination = [Rgb565Pixel(2); 64];
        let mut output = [Rgb565Pixel(3); 64];

        assert_eq!(
            render_dithered_background(&source, &destination, &mut output, 8, 8, 0.0),
            0
        );
        assert_eq!(output, source);
        assert_eq!(
            render_dithered_background(&source, &destination, &mut output, 8, 8, 0.5),
            64
        );
        assert_eq!(output, [Rgb565Pixel(0); 64]);
        assert_eq!(
            render_dithered_background(&source, &destination, &mut output, 8, 8, 1.0),
            64
        );
        assert_eq!(output, destination);
    }

    #[test]
    fn opposite_portrait_directions_use_the_longer_transition() {
        let start = Instant::now();
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        assert!(runtime.start(
            ScreenOrientation::MonitorClockwise,
            ScreenOrientation::MonitorCounterclockwise,
            &[Rgb565Pixel(1); 12],
            start,
            false,
        ));
        assert_eq!(runtime.duration, ORIENTATION_OPPOSITE_TURN_DURATION);
    }

    #[test]
    fn reduce_motion_completes_without_rendering_transition_frames() {
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        assert!(!runtime.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &[Rgb565Pixel(1); 12],
            Instant::now(),
            true,
        ));
        assert!(!runtime.is_active());
        assert_eq!(
            runtime.take_completion(),
            Some(OrientationTransitionCompletion {
                from: ScreenOrientation::Normal,
                to: ScreenOrientation::MonitorClockwise,
            })
        );
    }

    #[test]
    fn completed_frame_is_exact_destination() {
        let start = Instant::now();
        let source = [Rgb565Pixel(1); 12];
        let destination = [Rgb565Pixel(2); 12];
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        runtime.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &source,
            start,
            false,
        );
        assert!(runtime.capture_destination(&destination));
        let (frame, done, _) = runtime
            .render(start + ORIENTATION_QUARTER_TURN_DURATION)
            .expect("transition frame");
        assert!(done);
        assert_eq!(frame, destination);
    }

    #[test]
    fn render_stats_separate_mapping_and_crossfade_work() {
        let start = Instant::now();
        let source = [Rgb565Pixel(1); 12];
        let destination = [Rgb565Pixel(2); 12];
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        assert!(runtime.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &source,
            start,
            false,
        ));
        assert!(runtime.capture_destination(&destination));

        let (_, halfway_done, halfway) = runtime
            .render(start + ORIENTATION_QUARTER_TURN_DURATION / 2)
            .expect("halfway transition frame");
        assert!(!halfway_done);
        assert!(halfway.mapped_pixels > 0);
        assert_eq!(halfway.blended_pixels, 12);
        assert_eq!(halfway.progress_ppm, 500_000);

        let (_, final_done, final_stats) = runtime
            .render(start + ORIENTATION_QUARTER_TURN_DURATION)
            .expect("final transition frame");
        assert!(final_done);
        assert_eq!(final_stats.blended_pixels, 12);
        assert_eq!(final_stats.progress_ppm, 1_000_000);
        assert!(final_stats.total_us >= final_stats.fill_us);
        assert!(final_stats.total_us >= final_stats.map_us);
        assert!(final_stats.total_us >= final_stats.crossfade_us);
    }

    #[test]
    fn transition_reuses_preallocated_buffers() {
        let start = Instant::now();
        let source = [Rgb565Pixel(1); 12];
        let destination = [Rgb565Pixel(2); 12];
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        let source_ptr = runtime.source.as_ptr();
        let destination_ptr = runtime.destination.as_ptr();
        let output_ptr = runtime.output.as_ptr();
        let capacities = (
            runtime.source.capacity(),
            runtime.destination.capacity(),
            runtime.output.capacity(),
        );

        assert!(runtime.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &source,
            start,
            false,
        ));
        assert!(runtime.capture_destination(&destination));
        let _ = runtime.render(start + Duration::from_millis(150));
        let _ = runtime.render(start + ORIENTATION_QUARTER_TURN_DURATION);
        assert!(runtime.start(
            ScreenOrientation::MonitorClockwise,
            ScreenOrientation::MonitorCounterclockwise,
            &destination,
            start + ORIENTATION_QUARTER_TURN_DURATION,
            false,
        ));
        assert!(runtime.capture_destination(&source));
        let _ = runtime
            .render(start + ORIENTATION_QUARTER_TURN_DURATION + ORIENTATION_OPPOSITE_TURN_DURATION);

        assert_eq!(runtime.source.as_ptr(), source_ptr);
        assert_eq!(runtime.destination.as_ptr(), destination_ptr);
        assert_eq!(runtime.output.as_ptr(), output_ptr);
        assert_eq!(
            (
                runtime.source.capacity(),
                runtime.destination.capacity(),
                runtime.output.capacity(),
            ),
            capacities
        );
    }

    #[test]
    fn pmu_labels_cover_every_directed_leg_and_phase() {
        let phases = [
            OrientationPmuPhase::Destination,
            OrientationPmuPhase::Fill,
            OrientationPmuPhase::Map,
            OrientationPmuPhase::Crossfade,
            OrientationPmuPhase::CacheRestore,
        ];
        let mut labels = std::collections::BTreeSet::new();
        for from in ScreenOrientation::ALL {
            for to in ScreenOrientation::ALL {
                if from == to {
                    continue;
                }
                for phase in phases {
                    let label = orientation_pmu_label(from, to, phase);
                    assert_ne!(label, "orientation.invalid");
                    assert!(labels.insert(label));
                }
            }
        }
        assert_eq!(labels.len(), 30);
    }
}
