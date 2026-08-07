// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral RGB565 monitor-orientation transition compositor.

use crate::settings::ScreenOrientation;
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::{Duration, Instant};

pub const ORIENTATION_WAVE_PHASE_DURATION: Duration = Duration::from_millis(1_500);
pub const ORIENTATION_WAVE_TOTAL_DURATION: Duration = Duration::from_millis(3_000);
const ORIENTATION_GRID_COLUMNS: usize = 16;
const ORIENTATION_GRID_ROWS: usize = 9;
const ORIENTATION_TILE_DELAY_US: u64 = 40_000;
const ORIENTATION_TILE_FADE_US: u64 = 500_000;
const RGB565_OPACITY_LEVELS: u8 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrientationTransitionEffect {
    BrightnessFade,
    CenterPixelZoom,
}

impl OrientationTransitionEffect {
    pub const fn id(self) -> &'static str {
        match self {
            Self::BrightnessFade => "brightness-fade",
            Self::CenterPixelZoom => "center-pixel-zoom",
        }
    }
}

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
    effect: OrientationTransitionEffect,
    from: ScreenOrientation,
    to: ScreenOrientation,
    phase: OrientationPmuPhase,
) -> &'static str {
    const FADE_LABELS: [[&str; 5]; 6] = [
        [
            "orientation.fade.normal-clockwise.destination",
            "orientation.fade.normal-clockwise.fill",
            "orientation.fade.normal-clockwise.map",
            "orientation.fade.normal-clockwise.crossfade",
            "orientation.fade.normal-clockwise.cache-restore",
        ],
        [
            "orientation.fade.clockwise-counterclockwise.destination",
            "orientation.fade.clockwise-counterclockwise.fill",
            "orientation.fade.clockwise-counterclockwise.map",
            "orientation.fade.clockwise-counterclockwise.crossfade",
            "orientation.fade.clockwise-counterclockwise.cache-restore",
        ],
        [
            "orientation.fade.counterclockwise-normal.destination",
            "orientation.fade.counterclockwise-normal.fill",
            "orientation.fade.counterclockwise-normal.map",
            "orientation.fade.counterclockwise-normal.crossfade",
            "orientation.fade.counterclockwise-normal.cache-restore",
        ],
        [
            "orientation.fade.normal-counterclockwise.destination",
            "orientation.fade.normal-counterclockwise.fill",
            "orientation.fade.normal-counterclockwise.map",
            "orientation.fade.normal-counterclockwise.crossfade",
            "orientation.fade.normal-counterclockwise.cache-restore",
        ],
        [
            "orientation.fade.counterclockwise-clockwise.destination",
            "orientation.fade.counterclockwise-clockwise.fill",
            "orientation.fade.counterclockwise-clockwise.map",
            "orientation.fade.counterclockwise-clockwise.crossfade",
            "orientation.fade.counterclockwise-clockwise.cache-restore",
        ],
        [
            "orientation.fade.clockwise-normal.destination",
            "orientation.fade.clockwise-normal.fill",
            "orientation.fade.clockwise-normal.map",
            "orientation.fade.clockwise-normal.crossfade",
            "orientation.fade.clockwise-normal.cache-restore",
        ],
    ];
    const ZOOM_LABELS: [[&str; 5]; 6] = [
        [
            "orientation.zoom.normal-clockwise.destination",
            "orientation.zoom.normal-clockwise.fill",
            "orientation.zoom.normal-clockwise.map",
            "orientation.zoom.normal-clockwise.crossfade",
            "orientation.zoom.normal-clockwise.cache-restore",
        ],
        [
            "orientation.zoom.clockwise-counterclockwise.destination",
            "orientation.zoom.clockwise-counterclockwise.fill",
            "orientation.zoom.clockwise-counterclockwise.map",
            "orientation.zoom.clockwise-counterclockwise.crossfade",
            "orientation.zoom.clockwise-counterclockwise.cache-restore",
        ],
        [
            "orientation.zoom.counterclockwise-normal.destination",
            "orientation.zoom.counterclockwise-normal.fill",
            "orientation.zoom.counterclockwise-normal.map",
            "orientation.zoom.counterclockwise-normal.crossfade",
            "orientation.zoom.counterclockwise-normal.cache-restore",
        ],
        [
            "orientation.zoom.normal-counterclockwise.destination",
            "orientation.zoom.normal-counterclockwise.fill",
            "orientation.zoom.normal-counterclockwise.map",
            "orientation.zoom.normal-counterclockwise.crossfade",
            "orientation.zoom.normal-counterclockwise.cache-restore",
        ],
        [
            "orientation.zoom.counterclockwise-clockwise.destination",
            "orientation.zoom.counterclockwise-clockwise.fill",
            "orientation.zoom.counterclockwise-clockwise.map",
            "orientation.zoom.counterclockwise-clockwise.crossfade",
            "orientation.zoom.counterclockwise-clockwise.cache-restore",
        ],
        [
            "orientation.zoom.clockwise-normal.destination",
            "orientation.zoom.clockwise-normal.fill",
            "orientation.zoom.clockwise-normal.map",
            "orientation.zoom.clockwise-normal.crossfade",
            "orientation.zoom.clockwise-normal.cache-restore",
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
    match effect {
        OrientationTransitionEffect::BrightnessFade => FADE_LABELS[leg][phase],
        OrientationTransitionEffect::CenterPixelZoom => ZOOM_LABELS[leg][phase],
    }
}

pub struct OrientationTransitionRuntime {
    width: usize,
    height: usize,
    from: ScreenOrientation,
    to: ScreenOrientation,
    started_at: Instant,
    duration: Duration,
    effect: OrientationTransitionEffect,
    source: Vec<Rgb565Pixel>,
    destination: Vec<Rgb565Pixel>,
    destination_ready: bool,
    active: bool,
    completion: Option<OrientationTransitionCompletion>,
    last_render_stats: OrientationTransitionRenderStats,
}

impl OrientationTransitionRuntime {
    pub fn new(width: usize, height: usize) -> Self {
        Self::new_with_effect(width, height, OrientationTransitionEffect::CenterPixelZoom)
    }

    pub fn new_with_effect(
        width: usize,
        height: usize,
        effect: OrientationTransitionEffect,
    ) -> Self {
        let len = width.saturating_mul(height);
        Self {
            width,
            height,
            from: ScreenOrientation::Normal,
            to: ScreenOrientation::Normal,
            started_at: Instant::now(),
            duration: ORIENTATION_WAVE_TOTAL_DURATION,
            effect,
            source: vec![Rgb565Pixel(0); len],
            destination: vec![Rgb565Pixel(0); len],
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
        self.duration = ORIENTATION_WAVE_TOTAL_DURATION;
        self.source.copy_from_slice(source);
        self.destination.fill(Rgb565Pixel(0));
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

    pub const fn effect(&self) -> OrientationTransitionEffect {
        self.effect
    }

    pub fn set_effect(&mut self, effect: OrientationTransitionEffect) -> bool {
        if self.active {
            return false;
        }
        self.effect = effect;
        true
    }

    pub const fn to(&self) -> ScreenOrientation {
        self.to
    }

    pub const fn last_render_stats(&self) -> OrientationTransitionRenderStats {
        self.last_render_stats
    }

    pub fn render_into(
        &mut self,
        output: &mut [Rgb565Pixel],
        now: Instant,
    ) -> Option<(bool, OrientationTransitionRenderStats)> {
        if !self.active || output.len() != self.source.len() {
            return None;
        }
        if !self.destination_ready {
            output.copy_from_slice(&self.source);
            self.last_render_stats = OrientationTransitionRenderStats::default();
            return Some((false, self.last_render_stats));
        }
        let render_started = Instant::now();
        let elapsed = now
            .saturating_duration_since(self.started_at)
            .min(self.duration);
        let fill_started = Instant::now();
        let fill_pmu = mister_magik_perf_events::sampled_span(orientation_pmu_label(
            self.effect,
            self.from,
            self.to,
            OrientationPmuPhase::Fill,
        ));
        drop(fill_pmu);
        let fill_us = elapsed_us(fill_started);
        let map_started = Instant::now();
        let map_pmu = mister_magik_perf_events::sampled_span(orientation_pmu_label(
            self.effect,
            self.from,
            self.to,
            OrientationPmuPhase::Map,
        ));
        drop(map_pmu);
        let map_us = elapsed_us(map_started);
        let crossfade_started = Instant::now();
        let crossfade_pmu = mister_magik_perf_events::sampled_span(orientation_pmu_label(
            self.effect,
            self.from,
            self.to,
            OrientationPmuPhase::Crossfade,
        ));
        let blended_pixels = match self.effect {
            OrientationTransitionEffect::BrightnessFade => render_brightness_wave(
                &self.source,
                &self.destination,
                output,
                self.width,
                self.height,
                elapsed,
            ),
            OrientationTransitionEffect::CenterPixelZoom => render_center_pixel_zoom_wave(
                &self.source,
                &self.destination,
                output,
                self.width,
                self.height,
                elapsed,
            ),
        };
        drop(crossfade_pmu);
        let crossfade_us = elapsed_us(crossfade_started);
        let done = elapsed >= self.duration;
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
            mapped_pixels: 0,
            blended_pixels,
            progress_ppm: duration_progress_ppm(elapsed, self.duration),
        };
        Some((done, self.last_render_stats))
    }

    pub fn take_completion(&mut self) -> Option<OrientationTransitionCompletion> {
        self.completion.take()
    }
}

fn render_brightness_wave(
    source: &[Rgb565Pixel],
    destination: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    elapsed: Duration,
) -> u64 {
    if elapsed >= ORIENTATION_WAVE_TOTAL_DURATION {
        output.copy_from_slice(destination);
        return output.len().min(u64::MAX as usize) as u64;
    }
    let (frame, phase_elapsed_us, revealing) = if elapsed < ORIENTATION_WAVE_PHASE_DURATION {
        (source, duration_us(elapsed), false)
    } else {
        (
            destination,
            duration_us(elapsed - ORIENTATION_WAVE_PHASE_DURATION),
            true,
        )
    };
    for tile_row in 0..ORIENTATION_GRID_ROWS {
        let y0 = tile_row * height / ORIENTATION_GRID_ROWS;
        let y1 = (tile_row + 1) * height / ORIENTATION_GRID_ROWS;
        for tile_column in 0..ORIENTATION_GRID_COLUMNS {
            let x0 = tile_column * width / ORIENTATION_GRID_COLUMNS;
            let x1 = (tile_column + 1) * width / ORIENTATION_GRID_COLUMNS;
            let eased = orientation_tile_eased_level(phase_elapsed_us, tile_row, tile_column);
            let level = match (eased, revealing) {
                (Some(level), true) => level,
                (Some(level), false) => RGB565_OPACITY_LEVELS.saturating_sub(level),
                (None, true) => 0,
                (None, false) => RGB565_OPACITY_LEVELS,
            };
            render_dimmed_tile(frame, output, width, x0, x1, y0, y1, level);
        }
    }
    output.len().min(u64::MAX as usize) as u64
}

fn render_center_pixel_zoom_wave(
    source: &[Rgb565Pixel],
    destination: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    elapsed: Duration,
) -> u64 {
    if elapsed >= ORIENTATION_WAVE_TOTAL_DURATION {
        output.copy_from_slice(destination);
        return output.len().min(u64::MAX as usize) as u64;
    }
    let (frame, phase_elapsed_us, revealing) = if elapsed < ORIENTATION_WAVE_PHASE_DURATION {
        (source, duration_us(elapsed), false)
    } else {
        (
            destination,
            duration_us(elapsed - ORIENTATION_WAVE_PHASE_DURATION),
            true,
        )
    };
    output.copy_from_slice(frame);
    for tile_row in 0..ORIENTATION_GRID_ROWS {
        let tile_y0 = tile_row * height / ORIENTATION_GRID_ROWS;
        let tile_y1 = (tile_row + 1) * height / ORIENTATION_GRID_ROWS;
        for tile_column in 0..ORIENTATION_GRID_COLUMNS {
            let tile_x0 = tile_column * width / ORIENTATION_GRID_COLUMNS;
            let tile_x1 = (tile_column + 1) * width / ORIENTATION_GRID_COLUMNS;
            let eased = orientation_tile_eased_level(phase_elapsed_us, tile_row, tile_column);
            let black_level = match (eased, revealing) {
                (Some(level), true) => RGB565_OPACITY_LEVELS.saturating_sub(level),
                (Some(level), false) => level,
                (None, true) => RGB565_OPACITY_LEVELS,
                (None, false) => continue,
            };
            if revealing && black_level == 0 {
                continue;
            }
            let (x0, x1) = centered_span(tile_x0, tile_x1, black_level);
            let (y0, y1) = centered_span(tile_y0, tile_y1, black_level);
            fill_black_rect(output, width, x0, x1, y0, y1);
        }
    }
    output.len().min(u64::MAX as usize) as u64
}

fn orientation_tile_eased_level(phase_elapsed_us: u64, row: usize, column: usize) -> Option<u8> {
    let delay_us = u64::try_from(row.saturating_add(column))
        .unwrap_or(u64::MAX)
        .saturating_mul(ORIENTATION_TILE_DELAY_US);
    if phase_elapsed_us < delay_us {
        return None;
    }
    let local_us = phase_elapsed_us
        .saturating_sub(delay_us)
        .min(ORIENTATION_TILE_FADE_US);
    let fade_squared = ORIENTATION_TILE_FADE_US.saturating_mul(ORIENTATION_TILE_FADE_US);
    let eased = local_us
        .saturating_mul(local_us)
        .saturating_mul(u64::from(RGB565_OPACITY_LEVELS))
        .saturating_add(fade_squared / 2)
        / fade_squared;
    Some(u8::try_from(eased).unwrap_or(RGB565_OPACITY_LEVELS))
}

fn centered_span(start: usize, end: usize, level: u8) -> (usize, usize) {
    let span = end.saturating_sub(start);
    if span == 0 {
        return (start, start);
    }
    let scaled = span
        .saturating_sub(1)
        .saturating_mul(usize::from(level))
        .saturating_add(usize::from(RGB565_OPACITY_LEVELS / 2))
        / usize::from(RGB565_OPACITY_LEVELS);
    let visible_span = 1usize.saturating_add(scaled).min(span);
    let center = start + span.saturating_sub(1) / 2;
    let centered_start = center.saturating_sub((visible_span - 1) / 2).max(start);
    (
        centered_start,
        centered_start.saturating_add(visible_span).min(end),
    )
}

fn fill_black_rect(
    output: &mut [Rgb565Pixel],
    stride: usize,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
) {
    for y in y0..y1 {
        let start = y * stride + x0;
        let end = y * stride + x1;
        output[start..end].fill(Rgb565Pixel(0));
    }
}

#[allow(clippy::too_many_arguments)]
fn render_dimmed_tile(
    frame: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    stride: usize,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    opacity_level: u8,
) {
    for y in y0..y1 {
        let start = y * stride + x0;
        let end = y * stride + x1;
        if opacity_level == 0 {
            output[start..end].fill(Rgb565Pixel(0));
        } else if opacity_level >= RGB565_OPACITY_LEVELS {
            output[start..end].copy_from_slice(&frame[start..end]);
        } else {
            for (pixel, source) in output[start..end].iter_mut().zip(&frame[start..end]) {
                *pixel = dim_565(*source, opacity_level);
            }
        }
    }
}

fn dim_565(pixel: Rgb565Pixel, opacity_level: u8) -> Rgb565Pixel {
    let pixel = u32::from(pixel.0);
    let opacity = u32::from(opacity_level);
    let red_blue = (((pixel & 0xf81f) * opacity) >> 5) & 0xf81f;
    let green = (((pixel & 0x07e0) * opacity) >> 5) & 0x07e0;
    Rgb565Pixel((red_blue | green) as u16)
}

fn duration_progress_ppm(elapsed: Duration, duration: Duration) -> u32 {
    let duration_us = duration.as_micros().max(1);
    let progress = elapsed
        .as_micros()
        .saturating_mul(1_000_000)
        .saturating_add(duration_us / 2)
        / duration_us;
    u32::try_from(progress.min(1_000_000)).unwrap_or(1_000_000)
}

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn elapsed_us(started: Instant) -> u64 {
    duration_us(started.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_uses_the_supplied_delay_duration_and_quadratic_easing() {
        assert_eq!(orientation_tile_eased_level(0, 0, 0), Some(0));
        assert_eq!(orientation_tile_eased_level(125_000, 0, 0), Some(2));
        assert_eq!(orientation_tile_eased_level(250_000, 0, 0), Some(8));
        assert_eq!(orientation_tile_eased_level(375_000, 0, 0), Some(18));
        assert_eq!(
            orientation_tile_eased_level(500_000, 0, 0),
            Some(RGB565_OPACITY_LEVELS)
        );
        assert_eq!(orientation_tile_eased_level(919_999, 8, 15), None);
        assert_eq!(orientation_tile_eased_level(920_000, 8, 15), Some(0));
        assert_eq!(
            orientation_tile_eased_level(1_420_000, 8, 15),
            Some(RGB565_OPACITY_LEVELS)
        );
    }

    #[test]
    fn wave_fades_old_frame_to_black_then_reveals_new_frame() {
        let width = 16;
        let height = 9;
        let source = vec![Rgb565Pixel(u16::MAX); width * height];
        let destination = vec![Rgb565Pixel(0x07e0); width * height];
        let mut output = vec![Rgb565Pixel(0); width * height];

        render_brightness_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            Duration::ZERO,
        );
        assert_eq!(output, source);

        render_brightness_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            Duration::from_millis(500),
        );
        assert_eq!(output[0], Rgb565Pixel(0));
        assert_eq!(output[width * height - 1], source[width * height - 1]);

        render_brightness_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            Duration::from_millis(1_420),
        );
        assert_eq!(output, vec![Rgb565Pixel(0); width * height]);

        render_brightness_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            ORIENTATION_WAVE_PHASE_DURATION + Duration::from_millis(500),
        );
        assert_eq!(output[0], destination[0]);
        assert_eq!(output[width * height - 1], Rgb565Pixel(0));

        render_brightness_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            ORIENTATION_WAVE_TOTAL_DURATION,
        );
        assert_eq!(output, destination);
    }

    #[test]
    fn center_pixel_zoom_expands_black_then_shrinks_over_destination() {
        let width = 160;
        let height = 90;
        let source = vec![Rgb565Pixel(u16::MAX); width * height];
        let destination = vec![Rgb565Pixel(0x07e0); width * height];
        let mut output = vec![Rgb565Pixel(0); width * height];
        let first_tile_center = 4 * width + 4;

        render_center_pixel_zoom_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            Duration::ZERO,
        );
        assert_eq!(output[first_tile_center], Rgb565Pixel(0));
        assert_eq!(
            output.iter().filter(|pixel| pixel.0 == 0).count(),
            1,
            "only the first tile's center pixel starts black"
        );

        render_center_pixel_zoom_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            Duration::from_millis(500),
        );
        assert!((0..10).all(|y| {
            output[y * width..y * width + 10]
                .iter()
                .all(|pixel| pixel.0 == 0)
        }));
        assert_eq!(output[width * height - 1], source[width * height - 1]);

        render_center_pixel_zoom_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            Duration::from_millis(1_420),
        );
        assert!(output.iter().all(|pixel| pixel.0 == 0));

        render_center_pixel_zoom_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            ORIENTATION_WAVE_PHASE_DURATION + Duration::from_millis(500),
        );
        assert!((0..10).all(|y| {
            output[y * width..y * width + 10]
                .iter()
                .all(|pixel| *pixel == destination[0])
        }));
        assert_eq!(output[width * height - 1], Rgb565Pixel(0));

        render_center_pixel_zoom_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            ORIENTATION_WAVE_TOTAL_DURATION,
        );
        assert_eq!(output, destination);
    }

    #[test]
    fn center_pixel_zoom_is_the_default_effect() {
        let runtime = OrientationTransitionRuntime::new(16, 9);
        assert_eq!(runtime.effect, OrientationTransitionEffect::CenterPixelZoom);
        let fade = OrientationTransitionRuntime::new_with_effect(
            16,
            9,
            OrientationTransitionEffect::BrightnessFade,
        );
        assert_eq!(fade.effect, OrientationTransitionEffect::BrightnessFade);
    }

    #[test]
    fn every_orientation_pair_uses_the_two_phase_wave_duration() {
        let start = Instant::now();
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        assert!(runtime.start(
            ScreenOrientation::MonitorClockwise,
            ScreenOrientation::MonitorCounterclockwise,
            &[Rgb565Pixel(1); 12],
            start,
            false,
        ));
        assert_eq!(runtime.duration, ORIENTATION_WAVE_TOTAL_DURATION);
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
        let mut output = [Rgb565Pixel(0); 12];
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        runtime.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &source,
            start,
            false,
        );
        assert!(runtime.capture_destination(&destination));
        let (done, _) = runtime
            .render_into(&mut output, start + ORIENTATION_WAVE_TOTAL_DURATION)
            .expect("transition frame");
        assert!(done);
        assert_eq!(output, destination);
    }

    #[test]
    fn render_stats_separate_mapping_and_crossfade_work() {
        let start = Instant::now();
        let source = [Rgb565Pixel(1); 12];
        let destination = [Rgb565Pixel(2); 12];
        let mut output = [Rgb565Pixel(0); 12];
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        assert!(runtime.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &source,
            start,
            false,
        ));
        assert!(runtime.capture_destination(&destination));

        let (halfway_done, halfway) = runtime
            .render_into(&mut output, start + ORIENTATION_WAVE_PHASE_DURATION)
            .expect("halfway transition frame");
        assert!(!halfway_done);
        assert_eq!(output, [Rgb565Pixel(0); 12]);
        assert_eq!(halfway.mapped_pixels, 0);
        assert_eq!(halfway.blended_pixels, 12);
        assert_eq!(halfway.progress_ppm, 500_000);

        let (final_done, final_stats) = runtime
            .render_into(&mut output, start + ORIENTATION_WAVE_TOTAL_DURATION)
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
        let mut output = [Rgb565Pixel(0); 12];
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        let source_ptr = runtime.source.as_ptr();
        let destination_ptr = runtime.destination.as_ptr();
        let capacities = (runtime.source.capacity(), runtime.destination.capacity());

        assert!(runtime.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &source,
            start,
            false,
        ));
        assert!(runtime.capture_destination(&destination));
        let _ = runtime.render_into(&mut output, start + Duration::from_millis(750));
        let _ = runtime.render_into(&mut output, start + ORIENTATION_WAVE_TOTAL_DURATION);
        assert!(runtime.start(
            ScreenOrientation::MonitorClockwise,
            ScreenOrientation::MonitorCounterclockwise,
            &destination,
            start + ORIENTATION_WAVE_TOTAL_DURATION,
            false,
        ));
        assert!(runtime.capture_destination(&source));
        let _ = runtime.render_into(&mut output, start + ORIENTATION_WAVE_TOTAL_DURATION * 2);

        assert_eq!(runtime.source.as_ptr(), source_ptr);
        assert_eq!(runtime.destination.as_ptr(), destination_ptr);
        assert_eq!(
            (runtime.source.capacity(), runtime.destination.capacity()),
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
                    for effect in [
                        OrientationTransitionEffect::BrightnessFade,
                        OrientationTransitionEffect::CenterPixelZoom,
                    ] {
                        let label = orientation_pmu_label(effect, from, to, phase);
                        assert_ne!(label, "orientation.invalid");
                        assert!(labels.insert(label));
                    }
                }
            }
        }
        assert_eq!(labels.len(), 60);
    }
}
