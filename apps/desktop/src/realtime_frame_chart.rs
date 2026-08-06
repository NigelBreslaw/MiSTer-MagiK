// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

pub const DEFAULT_WIDTH: u32 = 1_024;
pub const DEFAULT_HEIGHT: u32 = 122;
pub const MAX_WIDTH: u32 = 4_096;
pub const MAX_HEIGHT: u32 = 512;
pub const MAX_US: u64 = 33_334;

const TRANSPARENT: Rgba8Pixel = rgba(0, 0, 0, 0);
const PREPARE: Rgba8Pixel = rgba(0x6b, 0x72, 0x80, 0xff);
const RENDER: Rgba8Pixel = rgba(0x25, 0x63, 0xeb, 0xff);
const CUSTOM: Rgba8Pixel = rgba(0xa8, 0x55, 0xf7, 0xff);
const VSYNC: Rgba8Pixel = rgba(0xf5, 0x9e, 0x0b, 0xff);
const PRESENT: Rgba8Pixel = rgba(0x06, 0xb6, 0xd4, 0xff);
const CPU: Rgba8Pixel = rgba(0xff, 0xff, 0xff, 115);
const PROCESS_CPU: Rgba8Pixel = rgba(0x11, 0x18, 0x27, 0xff);
const IDLE: Rgba8Pixel = rgba(0x1a, 0x7f, 0x37, 0xff);
const OVER_BUDGET: Rgba8Pixel = rgba(0xcf, 0x22, 0x2e, 0xff);

const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Rgba8Pixel {
    Rgba8Pixel { r, g, b, a }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameSample {
    pub frame: u64,
    pub wall_us: u64,
    pub prepare_us: u64,
    pub render_us: u64,
    pub custom_draw_us: u64,
    pub vsync_us: u64,
    pub present_us: u64,
    pub cpu_prepare_us: u64,
    pub cpu_render_us: u64,
    pub cpu_custom_draw_us: u64,
    pub cpu_vsync_us: u64,
    pub cpu_present_us: u64,
    pub process_cpu_us: u64,
    pub over_budget: bool,
    pub idle: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FrameColumn {
    wall_us: u64,
    phases_us: [u64; 5],
    cpu_phases_us: [u64; 5],
    process_cpu_us: u64,
    over_budget: bool,
    has_idle: bool,
    has_active: bool,
}

impl FrameColumn {
    fn include(&mut self, sample: &FrameSample) {
        self.wall_us = self.wall_us.max(sample.wall_us);
        for (maximum, value) in self.phases_us.iter_mut().zip([
            sample.prepare_us,
            sample.render_us,
            sample.custom_draw_us,
            sample.vsync_us,
            sample.present_us,
        ]) {
            *maximum = (*maximum).max(value);
        }
        for (maximum, value) in self.cpu_phases_us.iter_mut().zip([
            sample.cpu_prepare_us,
            sample.cpu_render_us,
            sample.cpu_custom_draw_us,
            sample.cpu_vsync_us,
            sample.cpu_present_us,
        ]) {
            *maximum = (*maximum).max(value);
        }
        self.process_cpu_us = self.process_cpu_us.max(sample.process_cpu_us);
        self.over_budget |= sample.over_budget;
        self.has_idle |= sample.idle;
        self.has_active |= !sample.idle;
    }
}

#[derive(Clone)]
pub struct RenderedFrameChart {
    pub image: Image,
    pub has_data: bool,
}

pub struct FrameChartState {
    samples: Vec<FrameSample>,
    width: u32,
    height: u32,
}

impl Default for FrameChartState {
    fn default() -> Self {
        Self {
            samples: Vec::new(),
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
        }
    }
}

impl FrameChartState {
    pub fn set_samples(&mut self, samples: &[FrameSample]) -> RenderedFrameChart {
        self.samples.clear();
        self.samples.extend_from_slice(samples);
        self.render()
    }

    pub fn resize(&mut self, width: i32, height: i32) -> Option<RenderedFrameChart> {
        let width = normalize_dimension(width, MAX_WIDTH);
        let height = normalize_dimension(height, MAX_HEIGHT);
        if self.width == width && self.height == height {
            return None;
        }
        self.width = width;
        self.height = height;
        Some(self.render())
    }

    fn render(&self) -> RenderedFrameChart {
        let has_data = !self.samples.is_empty();
        let width = self.width;
        let height = self.height;
        let image = rasterize(&self.samples, width, height)
            .map(Image::from_rgba8)
            .unwrap_or_default();
        RenderedFrameChart {
            image,
            has_data,
        }
    }
}

fn normalize_dimension(value: i32, maximum: u32) -> u32 {
    u32::try_from(value).unwrap_or(0).min(maximum)
}

fn aggregate(samples: &[FrameSample], width: u32) -> Vec<FrameColumn> {
    let column_count = samples.len().min(width as usize);
    if column_count == 0 {
        return Vec::new();
    }

    (0..column_count)
        .map(|column| {
            let start = column * samples.len() / column_count;
            let end = (column + 1) * samples.len() / column_count;
            let mut aggregate = FrameColumn::default();
            for sample in &samples[start..end] {
                aggregate.include(sample);
            }
            aggregate
        })
        .collect()
}

fn rasterize(
    samples: &[FrameSample],
    width: u32,
    height: u32,
) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    if samples.is_empty() || width == 0 || height == 0 {
        return None;
    }
    let width = width.min(MAX_WIDTH);
    let height = height.min(MAX_HEIGHT);
    let columns = aggregate(samples, width);
    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    pixels.make_mut_slice().fill(TRANSPARENT);
    let x_offset = width as usize - columns.len();
    let stride = width as usize;
    let bytes = pixels.make_mut_slice();

    for (column_index, column) in columns.iter().enumerate() {
        let x = x_offset + column_index;
        if column.has_active {
            draw_stack(
                bytes,
                stride,
                height,
                x,
                &column.phases_us,
                &[PREPARE, RENDER, CUSTOM, VSYNC, PRESENT],
                false,
            );
            draw_stack(
                bytes,
                stride,
                height,
                x,
                &column.cpu_phases_us,
                &[CPU; 5],
                true,
            );
            let process_height = scaled_height(column.process_cpu_us, height, false);
            if process_height > 0 {
                let y = height.saturating_sub(process_height).min(height - 1);
                draw_vertical(bytes, stride, height, x, y, 2, PROCESS_CPU, false);
            }
        }
        if column.has_idle {
            set_pixel(bytes, stride, height, x, height - 1, IDLE, false);
        }
        if column.over_budget {
            let wall_height = scaled_height(column.wall_us, height, true);
            let y = height.saturating_sub(wall_height).min(height - 1);
            draw_vertical(bytes, stride, height, x, y, 2, OVER_BUDGET, false);
        }
    }

    Some(pixels)
}

fn draw_stack(
    pixels: &mut [Rgba8Pixel],
    stride: usize,
    height: u32,
    x: usize,
    values: &[u64; 5],
    colors: &[Rgba8Pixel; 5],
    blend: bool,
) {
    let mut bottom = height;
    for (&value, &color) in values.iter().zip(colors) {
        let segment_height = scaled_height(value, height, !blend);
        if segment_height == 0 || bottom == 0 {
            continue;
        }
        let segment_height = segment_height.min(bottom);
        bottom -= segment_height;
        draw_vertical(
            pixels,
            stride,
            height,
            x,
            bottom,
            segment_height,
            color,
            blend,
        );
    }
}

fn scaled_height(value: u64, height: u32, minimum_one: bool) -> u32 {
    if value == 0 {
        return u32::from(minimum_one);
    }
    let scaled = value
        .saturating_mul(u64::from(height))
        .div_ceil(MAX_US)
        .min(u64::from(height)) as u32;
    scaled.max(u32::from(minimum_one))
}

fn draw_vertical(
    pixels: &mut [Rgba8Pixel],
    stride: usize,
    height: u32,
    x: usize,
    y: u32,
    length: u32,
    color: Rgba8Pixel,
    blend: bool,
) {
    for row in y..y.saturating_add(length).min(height) {
        set_pixel(pixels, stride, height, x, row, color, blend);
    }
}

fn set_pixel(
    pixels: &mut [Rgba8Pixel],
    stride: usize,
    height: u32,
    x: usize,
    y: u32,
    color: Rgba8Pixel,
    blend: bool,
) {
    if y >= height {
        return;
    }
    let pixel = &mut pixels[y as usize * stride + x];
    *pixel = if blend { blend_over(*pixel, color) } else { color };
}

fn blend_over(base: Rgba8Pixel, overlay: Rgba8Pixel) -> Rgba8Pixel {
    if base.a == 0 {
        return overlay;
    }
    let alpha = u16::from(overlay.a);
    let inverse = 255 - alpha;
    let blend_channel = |below: u8, above: u8| {
        ((u16::from(above) * alpha + u16::from(below) * inverse + 127) / 255) as u8
    };
    Rgba8Pixel {
        r: blend_channel(base.r, overlay.r),
        g: blend_channel(base.g, overlay.g),
        b: blend_channel(base.b, overlay.b),
        a: base.a.max(overlay.a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(frame: u64, value: u64) -> FrameSample {
        FrameSample {
            frame,
            wall_us: value,
            prepare_us: value,
            render_us: value / 2,
            custom_draw_us: value / 3,
            vsync_us: value / 4,
            present_us: value / 5,
            cpu_prepare_us: value / 6,
            cpu_render_us: value / 7,
            cpu_custom_draw_us: value / 8,
            cpu_vsync_us: value / 9,
            cpu_present_us: value / 10,
            process_cpu_us: value / 2,
            over_budget: value > 16_667,
            idle: false,
        }
    }

    #[test]
    fn aggregation_is_bounded_and_preserves_channel_maxima() {
        let mut samples = (0..12)
            .map(|frame| sample(frame, 1_000 + frame * 100))
            .collect::<Vec<_>>();
        samples[4].render_us = 31_000;
        samples[5].cpu_vsync_us = 29_000;
        samples[5].over_budget = true;
        samples[6].idle = true;

        let columns = aggregate(&samples, 3);

        assert_eq!(columns.len(), 3);
        assert_eq!(columns[1].phases_us[1], 31_000);
        assert_eq!(columns[1].cpu_phases_us[3], 29_000);
        assert!(columns[1].over_budget);
        assert!(columns[1].has_idle);
        assert!(columns[1].has_active);
    }

    #[test]
    fn aggregation_buckets_cover_input_once_in_order() {
        let samples = (0..10)
            .map(|frame| sample(frame, frame))
            .collect::<Vec<_>>();

        let columns = aggregate(&samples, 3);

        assert_eq!(
            columns
                .iter()
                .map(|column| column.wall_us)
                .collect::<Vec<_>>(),
            [2, 5, 9]
        );
    }

    #[test]
    fn raster_size_is_viewport_bounded_and_empty_inputs_allocate_nothing() {
        assert!(rasterize(&[], 40, 20).is_none());
        assert!(rasterize(&[sample(0, 1_000)], 0, 20).is_none());
        let raster = rasterize(&[sample(0, 1_000)], MAX_WIDTH + 10, MAX_HEIGHT + 10)
            .expect("bounded raster");
        assert_eq!(raster.width(), MAX_WIDTH);
        assert_eq!(raster.height(), MAX_HEIGHT);
    }

    #[test]
    fn viewport_state_ignores_repeated_sizes_and_normalizes_negative_dimensions() {
        let mut state = FrameChartState::default();
        assert!(state
            .resize(DEFAULT_WIDTH as i32, DEFAULT_HEIGHT as i32)
            .is_none());
        let rendered = state.resize(-1, i32::MAX).expect("normalized resize");
        assert!(!rendered.has_data);
        assert_eq!(state.width, 0);
        assert_eq!(state.height, MAX_HEIGHT);
    }

    #[test]
    fn sparse_samples_are_right_aligned() {
        let raster = rasterize(&[sample(0, 5_000), sample(1, 6_000)], 8, 16)
            .expect("sparse raster");
        let pixels = raster.as_slice();
        assert!((0..16).all(|row| pixels[row * 8..row * 8 + 6]
            .iter()
            .all(|pixel| *pixel == TRANSPARENT)));
        assert_ne!(pixels[15 * 8 + 6], TRANSPARENT);
        assert_ne!(pixels[15 * 8 + 7], TRANSPARENT);
    }

    #[test]
    fn spike_over_budget_cpu_and_idle_markers_survive_downsampling() {
        let mut samples = (0..1_000)
            .map(|frame| sample(frame, 1_000))
            .collect::<Vec<_>>();
        samples[511].custom_draw_us = MAX_US;
        samples[511].process_cpu_us = MAX_US;
        samples[511].wall_us = MAX_US;
        samples[511].over_budget = true;
        samples[512].idle = true;

        let columns = aggregate(&samples, 10);
        let spike = &columns[5];
        assert_eq!(spike.phases_us[2], MAX_US);
        assert_eq!(spike.process_cpu_us, MAX_US);
        assert!(spike.over_budget);
        assert!(spike.has_idle);

        let raster = rasterize(&samples, 10, 32).expect("spike raster");
        let pixels = raster.as_slice();
        assert_eq!(pixels[5], OVER_BUDGET);
        assert_eq!(pixels[31 * 10 + 5], IDLE);
    }

    #[test]
    fn cpu_overlay_is_rasterized_over_phase_color() {
        let sample = FrameSample {
            render_us: MAX_US,
            cpu_render_us: MAX_US,
            ..sample(0, 0)
        };

        let raster = rasterize(&[sample], 1, 16).expect("CPU overlay raster");

        assert_eq!(raster.as_slice()[0], blend_over(RENDER, CPU));
    }

    #[test]
    fn idle_only_data_has_no_active_phase_pixels() {
        let mut idle = sample(0, 0);
        idle.idle = true;
        let raster = rasterize(&[idle], 4, 8).expect("idle raster");
        let pixels = raster.as_slice();
        assert_eq!(pixels[7 * 4 + 3], IDLE);
        assert!(pixels[..7 * 4 + 3]
            .iter()
            .all(|pixel| *pixel == TRANSPARENT));
    }

    #[test]
    fn slint_api_has_no_input_sized_frame_sample_model() {
        let api = include_str!("../ui/api.slint");
        let chart = include_str!("../ui/views/debug.slint");

        assert!(!api.contains("RealtimeFrameSample"));
        assert!(!api.contains("frame-samples"));
        assert!(!chart.contains("for sample"));
        assert!(chart.contains("frame-budget-raster := Image"));
    }
}
