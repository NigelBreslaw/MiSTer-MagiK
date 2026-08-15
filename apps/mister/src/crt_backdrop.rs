// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral RGB565 composition for low-resolution CRT screenshot backdrops.

use crate::preview_transition::{blend_rgb565_bucket, blend_rgb565_rows_bucketed};
use crate::ui_display::{ResolvedOutputRoute, UiDisplay};
use crate::visual_composition::{PreviewFrame, PreviewPixels};
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::{Duration, Instant};

pub const CRT_BACKDROP_FADE_DURATION: Duration = Duration::from_millis(130);
pub const CRT_BACKDROP_DARK_RETAIN_PERCENT: u8 = 40;
pub const CRT_BACKDROP_BACKGROUND: Rgb565Pixel = rgb565_from_rgb888(0x02, 0x08, 0x17);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CrtBackdropWorkTrace {
    pub prepare_us: u64,
    pub prepare_pixels: u32,
    pub blend_us: u64,
    pub blend_pixels: u32,
    pub alpha_bucket: u8,
    pub active: bool,
}

pub struct CrtBackdropState {
    width: usize,
    height: usize,
    source: Vec<Rgb565Pixel>,
    target: Vec<Rgb565Pixel>,
    retarget: Vec<Rgb565Pixel>,
    x_map: Vec<usize>,
    y_map: Vec<usize>,
    source_row_repeats: Vec<bool>,
    target_row_repeats: Vec<bool>,
    retarget_row_repeats: Vec<bool>,
    target_is_plain: bool,
    retarget_is_plain: bool,
    transition_started: Option<Duration>,
    pending_prepare_us: u64,
    pending_prepare_pixels: u32,
}

impl CrtBackdropState {
    pub fn for_display(display: &UiDisplay) -> Option<Self> {
        matches!(
            display.output_route(),
            ResolvedOutputRoute::Crt240p60 | ResolvedOutputRoute::Crt288p50
        )
        .then(|| Self::new(display.render_w(), display.render_h()))
    }

    pub fn new(width: usize, height: usize) -> Self {
        let len = width.saturating_mul(height);
        Self {
            width,
            height,
            source: vec![CRT_BACKDROP_BACKGROUND; len],
            target: vec![CRT_BACKDROP_BACKGROUND; len],
            retarget: vec![CRT_BACKDROP_BACKGROUND; len],
            x_map: vec![0; width],
            y_map: vec![0; height],
            source_row_repeats: plain_row_repeats(height),
            target_row_repeats: plain_row_repeats(height),
            retarget_row_repeats: plain_row_repeats(height),
            target_is_plain: true,
            retarget_is_plain: true,
            transition_started: None,
            pending_prepare_us: 0,
            pending_prepare_pixels: 0,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn pixels(&self) -> &[Rgb565Pixel] {
        &self.retarget
    }

    pub fn is_transitioning(&self) -> bool {
        self.transition_started.is_some()
    }

    pub fn retarget_plain(&mut self, now: Duration) {
        self.retarget(None, now);
    }

    pub fn retarget(&mut self, frame: Option<PreviewFrame<'_>>, now: Duration) {
        self.resolve_current(now);
        self.source.copy_from_slice(&self.retarget);
        self.source_row_repeats
            .copy_from_slice(&self.retarget_row_repeats);

        let prepare_start = Instant::now();
        self.target_is_plain = match frame {
            Some(frame)
                if scale_dimmed_center_crop_mapped(
                    &mut self.target,
                    self.width,
                    self.height,
                    frame,
                    &mut self.x_map,
                    &mut self.y_map,
                    &mut self.target_row_repeats,
                ) =>
            {
                false
            }
            _ => {
                self.target.fill(CRT_BACKDROP_BACKGROUND);
                self.target_row_repeats.fill(true);
                true
            }
        };
        self.pending_prepare_us = duration_us(prepare_start.elapsed());
        self.pending_prepare_pixels = self.target.len().min(u32::MAX as usize) as u32;
        self.transition_started = (self.source != self.target).then_some(now);
        if self.transition_started.is_none() {
            self.retarget.copy_from_slice(&self.target);
            self.retarget_row_repeats
                .copy_from_slice(&self.target_row_repeats);
            self.retarget_is_plain = self.target_is_plain;
        }
    }

    pub fn compose(&mut self, now: Duration) -> CrtBackdropWorkTrace {
        let mut trace = CrtBackdropWorkTrace {
            prepare_us: std::mem::take(&mut self.pending_prepare_us),
            prepare_pixels: std::mem::take(&mut self.pending_prepare_pixels),
            ..CrtBackdropWorkTrace::default()
        };
        let Some(started) = self.transition_started else {
            trace.alpha_bucket = 32;
            return trace;
        };
        let elapsed = now.checked_sub(started).unwrap_or_default();
        let numerator = elapsed
            .as_micros()
            .min(CRT_BACKDROP_FADE_DURATION.as_micros());
        let denominator = CRT_BACKDROP_FADE_DURATION.as_micros().max(1);
        let alpha_bucket = ((numerator * 32 + denominator / 2) / denominator) as u16;
        let blend_start = Instant::now();
        let row_width = self.width.max(1);
        for row in 0..self.height {
            let start = row.saturating_mul(row_width);
            let end = start.saturating_add(row_width).min(self.retarget.len());
            if row > 0 && self.source_row_repeats[row] && self.target_row_repeats[row] {
                let (before, current) = self.retarget.split_at_mut(start);
                current[..end - start].copy_from_slice(&before[start - row_width..start]);
                continue;
            }
            let destination = &mut self.retarget[start..end];
            let previous = &self.source[start..end];
            let current = &self.target[start..end];
            let has_horizontal_repeat =
                previous
                    .windows(2)
                    .zip(current.windows(2))
                    .any(|(previous_pair, current_pair)| {
                        previous_pair[1] == previous_pair[0] && current_pair[1] == current_pair[0]
                    });
            if !has_horizontal_repeat {
                blend_rgb565_rows_bucketed(destination, previous, current, alpha_bucket);
                continue;
            }
            let mut previous_source = Rgb565Pixel(u16::MAX);
            let mut previous_current = Rgb565Pixel(u16::MAX);
            for index in 0..destination.len() {
                if index > 0
                    && previous[index] == previous_source
                    && current[index] == previous_current
                {
                    destination[index] = destination[index - 1];
                } else {
                    destination[index] =
                        blend_rgb565_bucket(previous[index], current[index], alpha_bucket);
                }
                previous_source = previous[index];
                previous_current = current[index];
            }
        }
        for row in 0..self.height {
            self.retarget_row_repeats[row] =
                row > 0 && self.source_row_repeats[row] && self.target_row_repeats[row];
        }
        trace.blend_us = duration_us(blend_start.elapsed());
        trace.blend_pixels = self.retarget.len().min(u32::MAX as usize) as u32;
        trace.alpha_bucket = alpha_bucket.min(32) as u8;
        trace.active = alpha_bucket < 32;
        if !trace.active {
            self.transition_started = None;
            self.retarget_is_plain = self.target_is_plain;
        } else {
            self.retarget_is_plain = false;
        }
        trace
    }

    fn resolve_current(&mut self, now: Duration) {
        if self.transition_started.is_some() {
            let _ = self.compose(now);
        }
    }
}

#[cfg(test)]
fn scale_dimmed_center_crop(
    destination: &mut [Rgb565Pixel],
    destination_width: usize,
    destination_height: usize,
    frame: PreviewFrame<'_>,
) -> bool {
    let mut x_map = vec![0; destination_width];
    let mut y_map = vec![0; destination_height];
    let mut row_repeats = vec![false; destination_height];
    scale_dimmed_center_crop_mapped(
        destination,
        destination_width,
        destination_height,
        frame,
        &mut x_map,
        &mut y_map,
        &mut row_repeats,
    )
}

fn scale_dimmed_center_crop_mapped(
    destination: &mut [Rgb565Pixel],
    destination_width: usize,
    destination_height: usize,
    frame: PreviewFrame<'_>,
    x_map: &mut [usize],
    y_map: &mut [usize],
    row_repeats: &mut [bool],
) -> bool {
    if destination_width == 0
        || destination_height == 0
        || destination.len() < destination_width.saturating_mul(destination_height)
        || x_map.len() < destination_width
        || y_map.len() < destination_height
        || row_repeats.len() < destination_height
        || frame.source_width == 0
        || frame.source_height == 0
    {
        return false;
    }
    let valid = match frame.pixels {
        PreviewPixels::Empty => return false,
        PreviewPixels::Rgb565 {
            pixels,
            stride_pixels,
        } => {
            stride_pixels >= frame.source_width
                && frame
                    .source_height
                    .saturating_sub(1)
                    .saturating_mul(stride_pixels)
                    .saturating_add(frame.source_width)
                    <= pixels.len()
        }
        PreviewPixels::Rgb8(pixels) => {
            frame
                .source_width
                .saturating_mul(frame.source_height)
                .saturating_mul(3)
                <= pixels.len()
        }
    };
    if !valid {
        return false;
    }

    let (crop_x, crop_y, crop_width, crop_height) =
        center_crop_4_3(frame.source_width, frame.source_height);
    for (destination_x, source_x) in x_map[..destination_width].iter_mut().enumerate() {
        *source_x = crop_x
            + (destination_x.saturating_mul(crop_width) / destination_width).min(crop_width - 1);
    }
    for destination_y in 0..destination_height {
        let source_y = crop_y
            + (destination_y.saturating_mul(crop_height) / destination_height).min(crop_height - 1);
        y_map[destination_y] = source_y;
        row_repeats[destination_y] = destination_y > 0 && y_map[destination_y - 1] == source_y;
        let destination_start = destination_y * destination_width;
        if row_repeats[destination_y] {
            let (before, current) = destination.split_at_mut(destination_start);
            current[..destination_width]
                .copy_from_slice(&before[destination_start - destination_width..destination_start]);
            continue;
        }
        let mut previous_source_x = usize::MAX;
        let mut previous_pixel = CRT_BACKDROP_BACKGROUND;
        for (destination_x, source_x) in x_map[..destination_width].iter().copied().enumerate() {
            if source_x == previous_source_x {
                destination[destination_start + destination_x] = previous_pixel;
                continue;
            }
            let source = match frame.pixels {
                PreviewPixels::Empty => CRT_BACKDROP_BACKGROUND,
                PreviewPixels::Rgb565 {
                    pixels,
                    stride_pixels,
                } => pixels[source_y * stride_pixels + source_x],
                PreviewPixels::Rgb8(pixels) => {
                    let index = (source_y * frame.source_width + source_x) * 3;
                    rgb565_from_rgb888(pixels[index], pixels[index + 1], pixels[index + 2])
                }
            };
            previous_pixel = darken_rgb565(source);
            previous_source_x = source_x;
            destination[destination_start + destination_x] = previous_pixel;
        }
    }
    true
}

fn center_crop_4_3(width: usize, height: usize) -> (usize, usize, usize, usize) {
    if width.saturating_mul(3) > height.saturating_mul(4) {
        let crop_width = (height.saturating_mul(4) / 3).max(1).min(width);
        ((width - crop_width) / 2, 0, crop_width, height)
    } else {
        let crop_height = (width.saturating_mul(3) / 4).max(1).min(height);
        (0, (height - crop_height) / 2, width, crop_height)
    }
}

fn darken_rgb565(pixel: Rgb565Pixel) -> Rgb565Pixel {
    let red = DARKEN_5[((pixel.0 >> 11) & 0x1f) as usize];
    let green = DARKEN_6[((pixel.0 >> 5) & 0x3f) as usize];
    let blue = DARKEN_5[(pixel.0 & 0x1f) as usize];
    Rgb565Pixel((red << 11) | (green << 5) | blue)
}

const DARKEN_5: [u16; 32] = darken_table_5();
const DARKEN_6: [u16; 64] = darken_table_6();

const fn darken_table_5() -> [u16; 32] {
    let mut table = [0; 32];
    let mut value = 0;
    while value < table.len() {
        table[value] = value as u16 * CRT_BACKDROP_DARK_RETAIN_PERCENT as u16 / 100;
        value += 1;
    }
    table
}

const fn darken_table_6() -> [u16; 64] {
    let mut table = [0; 64];
    let mut value = 0;
    while value < table.len() {
        table[value] = value as u16 * CRT_BACKDROP_DARK_RETAIN_PERCENT as u16 / 100;
        value += 1;
    }
    table
}

fn plain_row_repeats(height: usize) -> Vec<bool> {
    (0..height).map(|row| row > 0).collect()
}

const fn rgb565_from_rgb888(r: u8, g: u8, b: u8) -> Rgb565Pixel {
    Rgb565Pixel(((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3))
}

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_display::{UiDisplayPlan, UiFramebufferSizePolicy};
    use mister_magik_core::display::DisplayGeometry;

    fn frame<'a>(pixels: &'a [Rgb565Pixel], width: usize, height: usize) -> PreviewFrame<'a> {
        PreviewFrame {
            pixels: PreviewPixels::Rgb565 {
                pixels,
                stride_pixels: width,
            },
            source_width: width,
            source_height: height,
            display_width: width,
            display_height: height,
        }
    }

    #[test]
    fn center_crop_maps_wide_and_tall_sources_to_four_three() {
        assert_eq!(center_crop_4_3(1600, 900), (200, 0, 1200, 900));
        assert_eq!(center_crop_4_3(600, 800), (0, 175, 600, 450));
        assert_eq!(center_crop_4_3(640, 480), (0, 0, 640, 480));
    }

    #[test]
    fn scaling_retains_forty_percent_of_each_rgb565_channel() {
        let source = [Rgb565Pixel(0xffff)];
        let mut output = [Rgb565Pixel(0)];
        assert!(scale_dimmed_center_crop(
            &mut output,
            1,
            1,
            frame(&source, 1, 1)
        ));
        assert_eq!(output[0], Rgb565Pixel((12 << 11) | (25 << 5) | 12));
    }

    #[test]
    fn mapped_scaling_reuses_nearest_neighbour_rows_and_columns() {
        let source = [
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x07e0),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0xffff),
        ];
        let mut output = [Rgb565Pixel(0); 16];
        let mut x_map = [0; 4];
        let mut y_map = [0; 4];
        let mut row_repeats = [false; 4];
        assert!(scale_dimmed_center_crop_mapped(
            &mut output,
            4,
            4,
            frame(&source, 2, 2),
            &mut x_map,
            &mut y_map,
            &mut row_repeats,
        ));
        assert_eq!(x_map, [0, 0, 1, 1]);
        assert_eq!(y_map, [0, 0, 1, 1]);
        assert_eq!(row_repeats, [false, true, false, true]);
        assert_eq!(
            output,
            [
                darken_rgb565(source[0]),
                darken_rgb565(source[0]),
                darken_rgb565(source[1]),
                darken_rgb565(source[1]),
                darken_rgb565(source[0]),
                darken_rgb565(source[0]),
                darken_rgb565(source[1]),
                darken_rgb565(source[1]),
                darken_rgb565(source[2]),
                darken_rgb565(source[2]),
                darken_rgb565(source[3]),
                darken_rgb565(source[3]),
                darken_rgb565(source[2]),
                darken_rgb565(source[2]),
                darken_rgb565(source[3]),
                darken_rgb565(source[3]),
            ]
        );
    }

    #[test]
    fn low_resolution_routes_allocate_exact_composition_sizes() {
        for (route, geometry, expected) in [
            (
                ResolvedOutputRoute::Crt240p60,
                DisplayGeometry::new(640, 240),
                (640, 480),
            ),
            (
                ResolvedOutputRoute::Crt288p50,
                DisplayGeometry::new(640, 288),
                (640, 288),
            ),
        ] {
            let plan = UiDisplayPlan::from_geometry_with_route(
                geometry,
                route,
                "test-backdrop-route",
                UiFramebufferSizePolicy::Auto,
            );
            let display = UiDisplay::for_plan(plan);
            let backdrop = CrtBackdropState::for_display(&display).unwrap();
            assert_eq!((backdrop.width(), backdrop.height()), expected);
            assert_eq!(backdrop.pixels().len(), expected.0 * expected.1);
        }
    }

    #[test]
    fn fade_has_exact_endpoints_and_rapid_retarget_snapshots_current_blend() {
        let black = [Rgb565Pixel(0); 4];
        let white = [Rgb565Pixel(0xffff); 4];
        let red = [Rgb565Pixel(0xf800); 4];
        let mut backdrop = CrtBackdropState::new(2, 2);

        backdrop.retarget(Some(frame(&white, 2, 2)), Duration::ZERO);
        let start = backdrop.compose(Duration::ZERO);
        assert_eq!(start.alpha_bucket, 0);
        assert!(
            backdrop
                .pixels()
                .iter()
                .all(|pixel| *pixel == CRT_BACKDROP_BACKGROUND)
        );

        let halfway = Duration::from_millis(65);
        backdrop.compose(halfway);
        let snapshot = backdrop.pixels().to_vec();
        backdrop.retarget(Some(frame(&red, 2, 2)), halfway);
        assert_eq!(backdrop.source, snapshot);

        let end = backdrop.compose(halfway + CRT_BACKDROP_FADE_DURATION);
        assert_eq!(end.alpha_bucket, 32);
        assert!(!end.active);
        assert!(
            backdrop
                .pixels()
                .iter()
                .all(|pixel| *pixel == darken_rgb565(red[0]))
        );

        backdrop.retarget(Some(frame(&black, 2, 2)), Duration::from_secs(1));
        backdrop.compose(Duration::from_secs(1) + CRT_BACKDROP_FADE_DURATION);
        assert!(
            backdrop
                .pixels()
                .iter()
                .all(|pixel| *pixel == Rgb565Pixel(0))
        );
        backdrop.retarget_plain(Duration::from_secs(2));
        backdrop.compose(Duration::from_secs(2) + CRT_BACKDROP_FADE_DURATION);
        assert!(
            backdrop
                .pixels()
                .iter()
                .all(|pixel| *pixel == CRT_BACKDROP_BACKGROUND)
        );
    }
}
