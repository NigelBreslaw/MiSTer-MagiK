// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_catalog::preview_worker::PreviewPixels;
use mister_magik_framebuffer_scenes::Rgb565Pixel;

pub const PARADE_SUBPIXEL_ONE: i64 = 256;
const PARADE_MIN_TILE_SPEED: usize = 1;
const PARADE_SPEED_COUNT: usize = 5;
const PARADE_REFERENCE_HEIGHT: usize = 540;
const CRT_PHASE_COUNT: usize = 16;
const CRT_SHIFTED_PHASE_COUNT: usize = CRT_PHASE_COUNT - 1;
const CRT_PHASE_STEP: usize = PARADE_SUBPIXEL_ONE as usize / CRT_PHASE_COUNT;
const LANCZOS_RADIUS: f64 = 3.0;
const LANCZOS_WEIGHT_ONE: i32 = 1 << 14;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenshotSamplingProfile {
    HdmiLegacyHalf,
    CrtSixteenth,
}

impl ScreenshotSamplingProfile {
    #[must_use]
    pub const fn for_layer(self, layer: usize) -> Self {
        if matches!(self, Self::CrtSixteenth) || layer == PARADE_MIN_TILE_SPEED {
            Self::CrtSixteenth
        } else {
            Self::HdmiLegacyHalf
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenshotImage {
    pub pixels: Vec<Rgb565Pixel>,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

impl ScreenshotImage {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            pixels: Vec::new(),
            width: 0,
            height: 0,
            stride: 0,
        }
    }

    #[must_use]
    pub fn from_preview(pixels: PreviewPixels) -> Self {
        match pixels {
            PreviewPixels::Rgb565 {
                width,
                height,
                stride_bytes,
                words,
            } => Self {
                pixels: words.iter().copied().map(Rgb565Pixel).collect(),
                width: width as usize,
                height: height as usize,
                stride: stride_bytes as usize / size_of::<u16>(),
            },
        }
    }
}

#[derive(Clone)]
enum ParadePhaseSet {
    LegacyHalf(ScreenshotImage),
    CrtSixteenth(Box<[ScreenshotImage; CRT_SHIFTED_PHASE_COUNT]>),
}

impl ParadePhaseSet {
    fn prepare(image: &ScreenshotImage, profile: ScreenshotSamplingProfile) -> Self {
        match profile {
            ScreenshotSamplingProfile::HdmiLegacyHalf => {
                Self::LegacyHalf(prepare_fractional_shifted(image, 128))
            }
            ScreenshotSamplingProfile::CrtSixteenth => {
                let phases = std::array::from_fn(|index| {
                    prepare_fractional_shifted(image, ((index + 1) * CRT_PHASE_STEP) as u8)
                });
                Self::CrtSixteenth(Box::new(phases))
            }
        }
    }

    fn legacy_half(&self) -> &ScreenshotImage {
        match self {
            Self::LegacyHalf(image) => image,
            Self::CrtSixteenth(phases) => &phases[CRT_PHASE_COUNT / 2 - 1],
        }
    }

    fn crt_phase(&self, phase: usize) -> Option<&ScreenshotImage> {
        if phase == 0 || phase >= CRT_PHASE_COUNT {
            return None;
        }
        match self {
            Self::CrtSixteenth(phases) => phases.get(phase - 1),
            Self::LegacyHalf(_) => None,
        }
    }

    fn resident_bytes(&self) -> usize {
        match self {
            Self::LegacyHalf(image) => image.pixels.len() * size_of::<Rgb565Pixel>(),
            Self::CrtSixteenth(phases) => phases
                .iter()
                .map(|image| image.pixels.len() * size_of::<Rgb565Pixel>())
                .sum(),
        }
    }
}

#[derive(Clone)]
pub struct PreparedScreenshotCard {
    image: ScreenshotImage,
    phases: ParadePhaseSet,
    corner_insets: Vec<u8>,
}

impl PreparedScreenshotCard {
    #[must_use]
    pub fn prepare(
        source: &ScreenshotImage,
        speed: usize,
        screen_height: usize,
        profile: ScreenshotSamplingProfile,
    ) -> Self {
        Self::prepare_timed(source, speed, screen_height, profile).0
    }

    pub(crate) fn prepare_timed(
        source: &ScreenshotImage,
        speed: usize,
        screen_height: usize,
        profile: ScreenshotSamplingProfile,
    ) -> (Self, u128) {
        if source.width == 0 || source.height == 0 {
            let image = ScreenshotImage::empty();
            return (
                Self {
                    phases: ParadePhaseSet::prepare(&image, profile),
                    image,
                    corner_insets: Vec::new(),
                },
                0,
            );
        }
        let (width, height, tint) = scaled_style(source, speed, screen_height);
        let mut image = scale_lanczos3_rgb565_tinted(source, width, height, tint);
        apply_depth_cues(&mut image, speed);
        let corner_insets = prepare_corner_insets(image.width, image.height);
        let depth = speed
            .saturating_sub(PARADE_MIN_TILE_SPEED)
            .min(PARADE_SPEED_COUNT - 1);
        if depth >= 3 {
            rim_card(&mut image, &corner_insets);
        }
        let phase_started = std::time::Instant::now();
        let phases = ParadePhaseSet::prepare(&image, profile);
        let phase_us = phase_started.elapsed().as_micros();
        (
            Self {
                image,
                phases,
                corner_insets,
            },
            phase_us,
        )
    }

    #[must_use]
    pub const fn width(&self) -> usize {
        self.image.width
    }

    #[must_use]
    pub const fn height(&self) -> usize {
        self.image.height
    }

    #[must_use]
    pub fn max_corner_inset(&self) -> usize {
        self.corner_insets.iter().copied().max().unwrap_or_default() as usize
    }

    #[must_use]
    pub fn resident_bytes(&self) -> usize {
        self.image_resident_bytes() + self.phase_resident_bytes()
    }

    #[must_use]
    pub fn image_resident_bytes(&self) -> usize {
        self.image.pixels.len() * size_of::<Rgb565Pixel>()
    }

    #[must_use]
    pub fn phase_resident_bytes(&self) -> usize {
        self.phases.resident_bytes()
    }

    pub fn blit(
        &self,
        dst: &mut [Rgb565Pixel],
        screen_width: usize,
        screen_height: usize,
        profile: ScreenshotSamplingProfile,
        x_fp: i64,
        y: isize,
    ) {
        match profile {
            ScreenshotSamplingProfile::HdmiLegacyHalf => blit_half_phase(
                dst,
                screen_width,
                screen_height,
                &self.image,
                self.phases.legacy_half(),
                &self.corner_insets,
                x_fp,
                y,
            ),
            ScreenshotSamplingProfile::CrtSixteenth => blit_sixteenth_phase(
                dst,
                screen_width,
                screen_height,
                &self.image,
                &self.phases,
                &self.corner_insets,
                x_fp,
                y,
            ),
        }
    }
}

struct LanczosFilter {
    start: usize,
    weights: Vec<i16>,
}

fn color565(r: u8, g: u8, b: u8) -> Rgb565Pixel {
    Rgb565Pixel((u16::from(r) >> 3) << 11 | (u16::from(g) >> 2) << 5 | (u16::from(b) >> 3))
}

fn blend_565(from: Rgb565Pixel, to: Rgb565Pixel, alpha: u8) -> Rgb565Pixel {
    let from = u32::from(from.0);
    let to = u32::from(to.0);
    let alpha = ((u32::from(alpha) + 4) >> 3).min(32);
    if alpha == 0 {
        return Rgb565Pixel(from as u16);
    }
    if alpha >= 32 {
        return Rgb565Pixel(to as u16);
    }
    let inverse = 32 - alpha;
    let rb = (((from & 0xf81f) * inverse + (to & 0xf81f) * alpha) >> 5) & 0xf81f;
    let g = (((from & 0x07e0) * inverse + (to & 0x07e0) * alpha) >> 5) & 0x07e0;
    Rgb565Pixel((rb | g) as u16)
}

fn scale_dimension(reference: usize, screen_height: usize) -> usize {
    reference
        .saturating_mul(screen_height)
        .saturating_add(PARADE_REFERENCE_HEIGHT / 2)
        .checked_div(PARADE_REFERENCE_HEIGHT)
        .unwrap_or(1)
        .max(1)
}

pub(crate) fn depth_style(speed: usize, screen_height: usize) -> (usize, usize, u8) {
    let depth = speed
        .saturating_sub(PARADE_MIN_TILE_SPEED)
        .min(PARADE_SPEED_COUNT - 1);
    let reference = 160 * (depth + PARADE_MIN_TILE_SPEED) / PARADE_SPEED_COUNT;
    (
        scale_dimension(reference, screen_height),
        scale_dimension(reference, screen_height),
        [145, 170, 198, 226, 255][depth],
    )
}

fn scaled_style(image: &ScreenshotImage, speed: usize, screen_height: usize) -> (usize, usize, u8) {
    let (box_width, box_height, tint) = depth_style(speed, screen_height);
    if image.width * box_height > image.height * box_width {
        (
            box_width,
            (box_width * image.height + image.width / 2) / image.width,
            tint,
        )
    } else {
        (
            (box_height * image.width + image.height / 2) / image.height,
            box_height,
            tint,
        )
    }
}

fn lanczos3(value: f64) -> f64 {
    let value = value.abs();
    if value < f64::EPSILON {
        return 1.0;
    }
    if value >= LANCZOS_RADIUS {
        return 0.0;
    }
    let pi_value = std::f64::consts::PI * value;
    (pi_value.sin() / pi_value) * ((pi_value / LANCZOS_RADIUS).sin() / (pi_value / LANCZOS_RADIUS))
}

fn lanczos_filters(source_len: usize, target_len: usize) -> Vec<LanczosFilter> {
    let scale = target_len as f64 / source_len as f64;
    let filter_scale = scale.min(1.0);
    let support = LANCZOS_RADIUS / filter_scale;
    (0..target_len)
        .map(|target| {
            let center = (target as f64 + 0.5) / scale - 0.5;
            let first = (center - support).ceil() as isize;
            let last = (center + support).floor() as isize;
            let start = first.max(0) as usize;
            let end = last.min(source_len as isize - 1).max(first.max(0)) as usize;
            let float_weights = (start..=end)
                .map(|source| lanczos3((source as f64 - center) * filter_scale) * filter_scale)
                .collect::<Vec<_>>();
            let sum = float_weights.iter().sum::<f64>();
            let mut weights = float_weights
                .iter()
                .map(|weight| (weight / sum * f64::from(LANCZOS_WEIGHT_ONE)).round() as i16)
                .collect::<Vec<_>>();
            let fixed_sum = weights.iter().map(|weight| i32::from(*weight)).sum::<i32>();
            let center_tap = weights.len() / 2;
            weights[center_tap] =
                (i32::from(weights[center_tap]) + LANCZOS_WEIGHT_ONE - fixed_sum) as i16;
            LanczosFilter { start, weights }
        })
        .collect()
}

fn scale_lanczos3_rgb565_tinted(
    image: &ScreenshotImage,
    out_width: usize,
    out_height: usize,
    tint: u8,
) -> ScreenshotImage {
    if out_width == 0 || out_height == 0 || image.width == 0 || image.height == 0 {
        return ScreenshotImage {
            pixels: Vec::new(),
            width: out_width,
            height: out_height,
            stride: out_width,
        };
    }
    let x_filters = lanczos_filters(image.width, out_width);
    let y_filters = lanczos_filters(image.height, out_height);
    let mut horizontal = vec![0_u32; out_width * image.height];
    for source_y in 0..image.height {
        let source_row = source_y * image.stride;
        let target_row = source_y * out_width;
        for (target_x, filter) in x_filters.iter().enumerate() {
            let mut r = 0_i32;
            let mut g = 0_i32;
            let mut b = 0_i32;
            for (tap, weight) in filter.weights.iter().enumerate() {
                let pixel = image.pixels[source_row + filter.start + tap].0;
                let weight = i32::from(*weight);
                r += (i32::from((pixel >> 11) & 0x1f) * 255 / 31) * weight;
                g += (i32::from((pixel >> 5) & 0x3f) * 255 / 63) * weight;
                b += (i32::from(pixel & 0x1f) * 255 / 31) * weight;
            }
            let r = ((r + LANCZOS_WEIGHT_ONE / 2) >> 14).clamp(0, 255) as u32;
            let g = ((g + LANCZOS_WEIGHT_ONE / 2) >> 14).clamp(0, 255) as u32;
            let b = ((b + LANCZOS_WEIGHT_ONE / 2) >> 14).clamp(0, 255) as u32;
            horizontal[target_row + target_x] = (r << 16) | (g << 8) | b;
        }
    }

    let mut pixels = vec![Rgb565Pixel(0); out_width * out_height];
    for (target_y, filter) in y_filters.iter().enumerate() {
        for target_x in 0..out_width {
            let mut r = 0_i32;
            let mut g = 0_i32;
            let mut b = 0_i32;
            for (tap, weight) in filter.weights.iter().enumerate() {
                let pixel = horizontal[(filter.start + tap) * out_width + target_x];
                let weight = i32::from(*weight);
                r += i32::try_from((pixel >> 16) & 0xff).unwrap_or_default() * weight;
                g += i32::try_from((pixel >> 8) & 0xff).unwrap_or_default() * weight;
                b += i32::try_from(pixel & 0xff).unwrap_or_default() * weight;
            }
            let tint = i32::from(tint);
            let r =
                (((((r + LANCZOS_WEIGHT_ONE / 2) >> 14).clamp(0, 255)) * tint + 127) / 255) as u8;
            let g =
                (((((g + LANCZOS_WEIGHT_ONE / 2) >> 14).clamp(0, 255)) * tint + 127) / 255) as u8;
            let b =
                (((((b + LANCZOS_WEIGHT_ONE / 2) >> 14).clamp(0, 255)) * tint + 127) / 255) as u8;
            pixels[target_y * out_width + target_x] = color565(r, g, b);
        }
    }
    ScreenshotImage {
        pixels,
        width: out_width,
        height: out_height,
        stride: out_width,
    }
}

fn apply_depth_cues(image: &mut ScreenshotImage, speed: usize) {
    let depth = speed
        .saturating_sub(PARADE_MIN_TILE_SPEED)
        .min(PARADE_SPEED_COUNT - 1);
    let atmosphere = [20_u32, 14, 8, 3, 0][depth];
    let desaturation = [25_u32, 16, 8, 3, 0][depth];
    for pixel in &mut image.pixels {
        let packed = pixel.0;
        let mut r = u32::from((packed >> 11) & 0x1f) * 255 / 31;
        let mut g = u32::from((packed >> 5) & 0x3f) * 255 / 63;
        let mut b = u32::from(packed & 0x1f) * 255 / 31;
        let luminance = (77 * r + 150 * g + 29 * b + 128) >> 8;
        r = (r * (100 - desaturation) + luminance * desaturation + 50) / 100;
        g = (g * (100 - desaturation) + luminance * desaturation + 50) / 100;
        b = (b * (100 - desaturation) + luminance * desaturation + 50) / 100;
        r = (r * (100 - atmosphere) + 50) / 100;
        g = (g * (100 - atmosphere) + 50) / 100;
        b = (b * (100 - atmosphere) + 10 * atmosphere + 50) / 100;
        *pixel = color565(r as u8, g as u8, b as u8);
    }
}

fn prepare_corner_insets(width: usize, height: usize) -> Vec<u8> {
    let radius = (width.min(height) / 10).clamp(2, 10);
    let mut insets = vec![0_u8; height];
    for y in 0..radius.min(height / 2) {
        let distance = radius.saturating_sub(y + 1) as f64;
        let inside = ((radius * radius) as f64 - distance * distance)
            .max(0.0)
            .sqrt() as usize;
        let inset = radius.saturating_sub(inside).min(usize::from(u8::MAX)) as u8;
        insets[y] = inset;
        insets[height - 1 - y] = inset;
    }
    insets
}

fn rim_card(image: &mut ScreenshotImage, corner_insets: &[u8]) {
    if image.width == 0 || image.height == 0 {
        return;
    }
    let highlight = color565(210, 225, 255);
    let shadow = color565(0, 0, 8);
    for y in 0..image.height {
        let inset = corner_insets.get(y).copied().unwrap_or(0) as usize;
        let end = image.width.saturating_sub(inset);
        if inset >= end {
            continue;
        }
        let row = y * image.stride;
        for (offset, alpha) in [48_u8, 24].into_iter().enumerate() {
            if inset + offset < end {
                let left = row + inset + offset;
                image.pixels[left] = blend_565(image.pixels[left], highlight, alpha);
            }
            if end > inset + offset {
                let right = row + end - 1 - offset;
                image.pixels[right] = blend_565(image.pixels[right], shadow, alpha + 8);
            }
        }
        let horizontal_cue = if y < 2 {
            Some((highlight, [40_u8, 20][y]))
        } else if image.height - 1 - y < 2 {
            let edge = image.height - 1 - y;
            Some((shadow, [56_u8, 28][edge]))
        } else {
            None
        };
        if let Some((color, alpha)) = horizontal_cue {
            for pixel in &mut image.pixels[row + inset..row + end] {
                *pixel = blend_565(*pixel, color, alpha);
            }
        }
    }
}

fn prepare_fractional_shifted(image: &ScreenshotImage, phase_alpha: u8) -> ScreenshotImage {
    debug_assert!(phase_alpha > 0);
    let width = image.width + 1;
    let mut pixels = vec![Rgb565Pixel(0); width * image.height];
    for y in 0..image.height {
        let source = y * image.stride;
        let target = y * width;
        for x in 1..image.width {
            pixels[target + x] = blend_565(
                image.pixels[source + x - 1],
                image.pixels[source + x],
                255 - phase_alpha,
            );
        }
    }
    ScreenshotImage {
        pixels,
        width,
        height: image.height,
        stride: width,
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_half_phase(
    dst: &mut [Rgb565Pixel],
    screen_width: usize,
    screen_height: usize,
    image: &ScreenshotImage,
    half_shifted: &ScreenshotImage,
    corner_insets: &[u8],
    x_fp: i64,
    y: isize,
) {
    let x = x_fp.div_euclid(PARADE_SUBPIXEL_ONE) as isize;
    let fraction = x_fp.rem_euclid(PARADE_SUBPIXEL_ONE) as u8;
    if fraction == 0 {
        blit_rounded(dst, screen_width, screen_height, image, corner_insets, x, y);
        return;
    }
    if fraction == 128 {
        blit_fractional(
            dst,
            screen_width,
            screen_height,
            image,
            half_shifted,
            corner_insets,
            x,
            y,
            128,
        );
        return;
    }
    let snapped_x = if fraction < 128 { x } else { x + 1 };
    blit_rounded(
        dst,
        screen_width,
        screen_height,
        image,
        corner_insets,
        snapped_x,
        y,
    );
}

#[derive(Clone, Copy)]
struct QuantizedPhase {
    x: isize,
    phase: usize,
}

fn quantize_phase(x_fp: i64) -> QuantizedPhase {
    let mut x = x_fp.div_euclid(PARADE_SUBPIXEL_ONE) as isize;
    let fraction = x_fp.rem_euclid(PARADE_SUBPIXEL_ONE) as usize;
    let mut phase = (fraction + CRT_PHASE_STEP / 2) / CRT_PHASE_STEP;
    if phase == CRT_PHASE_COUNT {
        x += 1;
        phase = 0;
    }
    QuantizedPhase { x, phase }
}

#[allow(clippy::too_many_arguments)]
fn blit_sixteenth_phase(
    dst: &mut [Rgb565Pixel],
    screen_width: usize,
    screen_height: usize,
    image: &ScreenshotImage,
    phase_set: &ParadePhaseSet,
    corner_insets: &[u8],
    x_fp: i64,
    y: isize,
) {
    let quantized = quantize_phase(x_fp);
    if quantized.phase == 0 {
        blit_rounded(
            dst,
            screen_width,
            screen_height,
            image,
            corner_insets,
            quantized.x,
            y,
        );
        return;
    }
    let Some(shifted) = phase_set.crt_phase(quantized.phase) else {
        debug_assert!(false, "CRT card missing sixteenth-pixel phases");
        blit_half_phase(
            dst,
            screen_width,
            screen_height,
            image,
            phase_set.legacy_half(),
            corner_insets,
            x_fp,
            y,
        );
        return;
    };
    blit_fractional(
        dst,
        screen_width,
        screen_height,
        image,
        shifted,
        corner_insets,
        quantized.x,
        y,
        (quantized.phase * CRT_PHASE_STEP) as u8,
    );
}

#[allow(clippy::too_many_arguments)]
fn blit_fractional(
    dst: &mut [Rgb565Pixel],
    screen_width: usize,
    screen_height: usize,
    image: &ScreenshotImage,
    shifted: &ScreenshotImage,
    corner_insets: &[u8],
    x: isize,
    y: isize,
    phase_alpha: u8,
) {
    for source_y in 0..image.height {
        let target_y = y + source_y as isize;
        if target_y < 0 || target_y >= screen_height as isize {
            continue;
        }
        let target_row = target_y as usize * screen_width;
        let source_row = source_y * image.stride;
        let shifted_row = source_y * shifted.stride;
        let inset = corner_insets.get(source_y).copied().unwrap_or(0) as usize;
        let source_end = image.width.saturating_sub(inset);
        if inset >= source_end {
            continue;
        }
        let left = x + inset as isize;
        if left >= 0 && left < screen_width as isize {
            dst[target_row + left as usize] = blend_565(
                dst[target_row + left as usize],
                image.pixels[source_row + inset],
                255 - phase_alpha,
            );
        }
        let copy_x0 = (left + 1).max(0) as usize;
        let copy_x1 = (x + source_end as isize).clamp(0, screen_width as isize) as usize;
        if copy_x1 > copy_x0 {
            let source_x0 = (copy_x0 as isize - x) as usize;
            dst[target_row + copy_x0..target_row + copy_x1].copy_from_slice(
                &shifted.pixels
                    [shifted_row + source_x0..shifted_row + source_x0 + copy_x1 - copy_x0],
            );
        }
        let right = x + source_end as isize;
        if right >= 0 && right < screen_width as isize {
            dst[target_row + right as usize] = blend_565(
                dst[target_row + right as usize],
                image.pixels[source_row + source_end - 1],
                phase_alpha,
            );
        }
    }
}

fn blit_rounded(
    dst: &mut [Rgb565Pixel],
    screen_width: usize,
    screen_height: usize,
    image: &ScreenshotImage,
    corner_insets: &[u8],
    x: isize,
    y: isize,
) {
    for source_y in 0..image.height {
        let target_y = y + source_y as isize;
        if target_y < 0 || target_y >= screen_height as isize {
            continue;
        }
        let inset = corner_insets.get(source_y).copied().unwrap_or(0) as usize;
        let source_end = image.width.saturating_sub(inset);
        if inset >= source_end {
            continue;
        }
        let target_x0 = (x + inset as isize).max(0) as usize;
        let target_x1 = (x + source_end as isize).clamp(0, screen_width as isize) as usize;
        if target_x1 <= target_x0 {
            continue;
        }
        let source_x0 = (target_x0 as isize - x) as usize;
        let source_row = source_y * image.stride + source_x0;
        let target_row = target_y as usize * screen_width + target_x0;
        let copy_len = target_x1 - target_x0;
        dst[target_row..target_row + copy_len]
            .copy_from_slice(&image.pixels[source_row..source_row + copy_len]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_image(width: usize, height: usize) -> ScreenshotImage {
        let pixels = (0..width * height)
            .map(|index| {
                color565(
                    (index * 37) as u8,
                    (index * 61 + 80) as u8,
                    (index * 17 + 160) as u8,
                )
            })
            .collect();
        ScreenshotImage {
            pixels,
            width,
            height,
            stride: width,
        }
    }

    #[test]
    fn scaling_preserves_landscape_and_portrait_aspect_ratios() {
        let landscape = test_image(320, 240);
        let portrait = test_image(240, 320);
        assert_eq!(scaled_style(&landscape, 5, 540), (160, 120, 255));
        assert_eq!(scaled_style(&portrait, 5, 540), (120, 160, 255));
    }

    #[test]
    fn prepared_card_has_all_crt_phases_and_bounded_memory() {
        let source = test_image(8, 6);
        let card = PreparedScreenshotCard::prepare(
            &source,
            5,
            540,
            ScreenshotSamplingProfile::CrtSixteenth,
        );
        assert_eq!(card.width(), 160);
        assert_eq!(card.height(), 120);
        assert!(card.resident_bytes() >= card.width() * card.height() * 2 * CRT_PHASE_COUNT);
    }

    #[test]
    fn integer_blit_preserves_pixels_and_rounded_corners() {
        let source = test_image(8, 6);
        let card = PreparedScreenshotCard::prepare(
            &source,
            1,
            135,
            ScreenshotSamplingProfile::HdmiLegacyHalf,
        );
        let background = color565(4, 8, 12);
        let mut frame = vec![background; 32 * 24];
        card.blit(
            &mut frame,
            32,
            24,
            ScreenshotSamplingProfile::HdmiLegacyHalf,
            3 * PARADE_SUBPIXEL_ONE,
            2,
        );
        assert_eq!(frame[2 * 32 + 3], background);
        assert_ne!(frame[3 * 32 + 4], background);
    }

    #[test]
    fn fractional_blits_do_not_paint_outside_the_card_rows() {
        let source = test_image(8, 6);
        let card = PreparedScreenshotCard::prepare(
            &source,
            1,
            135,
            ScreenshotSamplingProfile::CrtSixteenth,
        );
        let background = color565(4, 8, 12);
        let mut frame = vec![background; 32 * 24];
        card.blit(
            &mut frame,
            32,
            24,
            ScreenshotSamplingProfile::CrtSixteenth,
            3 * PARADE_SUBPIXEL_ONE + PARADE_SUBPIXEL_ONE / 2,
            2,
        );
        assert!(frame[..2 * 32].iter().all(|pixel| *pixel == background));
        assert!(
            frame[(2 + card.height()) * 32..]
                .iter()
                .all(|pixel| *pixel == background)
        );
    }
}
