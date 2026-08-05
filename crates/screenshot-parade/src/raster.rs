// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_catalog::preview_worker::PreviewPixels;
use mister_magik_framebuffer_scenes::Rgb565Pixel;
use std::sync::OnceLock;

pub const PARADE_SUBPIXEL_ONE: i64 = 256;
const PARADE_MIN_TILE_SPEED: usize = 1;
const PARADE_SPEED_COUNT: usize = 5;
const PARADE_REFERENCE_HEIGHT: usize = 540;
const CRT_PHASE_COUNT: usize = 16;
const CRT_SHIFTED_PHASE_COUNT: usize = CRT_PHASE_COUNT - 1;
const CRT_PHASE_STEP: usize = PARADE_SUBPIXEL_ONE as usize / CRT_PHASE_COUNT;
const LANCZOS_RADIUS: f64 = 3.0;
const LANCZOS_WEIGHT_ONE: i32 = 1 << 14;
const COVERAGE_SAMPLES_PER_AXIS: usize = 8;
const COVERAGE_SAMPLE_COUNT: usize = COVERAGE_SAMPLES_PER_AXIS * COVERAGE_SAMPLES_PER_AXIS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenshotSamplingProfile {
    HdmiLegacyHalf,
    CrtSixteenth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenshotPhaseGeneration {
    Rgb565TwoTap,
    LinearLanczos3,
    LinearLanczos3Neon,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinearPhaseKernel {
    Scalar,
    Neon,
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
    SixteenthTwoTap(Box<[ScreenshotImage; CRT_SHIFTED_PHASE_COUNT]>),
    SixteenthLinear {
        base_coverage: CoveragePlane,
        shifted: Box<[PreparedLinearPhase; CRT_SHIFTED_PHASE_COUNT]>,
    },
}

impl ParadePhaseSet {
    fn prepare_two_tap(image: &ScreenshotImage, profile: ScreenshotSamplingProfile) -> Self {
        match profile {
            ScreenshotSamplingProfile::HdmiLegacyHalf => {
                Self::LegacyHalf(prepare_fractional_shifted(image, 128))
            }
            ScreenshotSamplingProfile::CrtSixteenth => {
                let phases = std::array::from_fn(|index| {
                    prepare_fractional_shifted(image, ((index + 1) * CRT_PHASE_STEP) as u8)
                });
                Self::SixteenthTwoTap(Box::new(phases))
            }
        }
    }

    fn legacy_half(&self) -> &ScreenshotImage {
        match self {
            Self::LegacyHalf(image) => image,
            Self::SixteenthTwoTap(phases) => &phases[CRT_PHASE_COUNT / 2 - 1],
            Self::SixteenthLinear { shifted, .. } => &shifted[CRT_PHASE_COUNT / 2 - 1].image,
        }
    }

    fn two_tap_phase(&self, phase: usize) -> Option<&ScreenshotImage> {
        if phase == 0 || phase >= CRT_PHASE_COUNT {
            return None;
        }
        match self {
            Self::SixteenthTwoTap(phases) => phases.get(phase - 1),
            Self::LegacyHalf(_) | Self::SixteenthLinear { .. } => None,
        }
    }

    fn linear_phase(&self, phase: usize) -> Option<LinearPhaseRef<'_>> {
        let Self::SixteenthLinear {
            base_coverage: _,
            shifted,
        } = self
        else {
            return None;
        };
        if phase == 0 {
            None
        } else {
            shifted.get(phase - 1).map(|phase| LinearPhaseRef {
                image: &phase.image,
                coverage: &phase.coverage,
            })
        }
    }

    fn base_coverage(&self) -> Option<&CoveragePlane> {
        match self {
            Self::SixteenthLinear { base_coverage, .. } => Some(base_coverage),
            Self::LegacyHalf(_) | Self::SixteenthTwoTap(_) => None,
        }
    }

    fn resident_bytes(&self) -> usize {
        match self {
            Self::LegacyHalf(image) => image.pixels.len() * size_of::<Rgb565Pixel>(),
            Self::SixteenthTwoTap(phases) => phases
                .iter()
                .map(|image| image.pixels.len() * size_of::<Rgb565Pixel>())
                .sum(),
            Self::SixteenthLinear {
                base_coverage,
                shifted,
            } => {
                base_coverage.resident_bytes()
                    + shifted
                        .iter()
                        .map(PreparedLinearPhase::resident_bytes)
                        .sum::<usize>()
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct OpaqueSpan {
    start: u16,
    end: u16,
}

#[derive(Clone)]
struct CoveragePlane {
    values: Vec<u8>,
    opaque_spans: Vec<OpaqueSpan>,
    stride: usize,
}

impl CoveragePlane {
    fn resident_bytes(&self) -> usize {
        self.values.len() * size_of::<u8>() + self.opaque_spans.len() * size_of::<OpaqueSpan>()
    }
}

#[derive(Clone)]
struct PreparedLinearPhase {
    image: ScreenshotImage,
    coverage: CoveragePlane,
}

impl PreparedLinearPhase {
    fn resident_bytes(&self) -> usize {
        self.image.pixels.len() * size_of::<Rgb565Pixel>() + self.coverage.resident_bytes()
    }
}

#[derive(Clone, Copy)]
struct LinearPhaseRef<'a> {
    image: &'a ScreenshotImage,
    coverage: &'a CoveragePlane,
}

#[derive(Clone, Copy, Debug, Default)]
struct LinearRgb {
    r: u16,
    g: u16,
    b: u16,
}

#[derive(Clone)]
struct LinearImage {
    pixels: Vec<LinearRgb>,
    width: usize,
    height: usize,
    stride: usize,
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
        Self::prepare_with_generation(
            source,
            speed,
            screen_height,
            profile,
            ScreenshotPhaseGeneration::Rgb565TwoTap,
        )
    }

    #[must_use]
    pub fn prepare_with_generation(
        source: &ScreenshotImage,
        speed: usize,
        screen_height: usize,
        profile: ScreenshotSamplingProfile,
        phase_generation: ScreenshotPhaseGeneration,
    ) -> Self {
        Self::prepare_timed(source, speed, screen_height, profile, phase_generation).0
    }

    pub(crate) fn prepare_timed(
        source: &ScreenshotImage,
        speed: usize,
        screen_height: usize,
        profile: ScreenshotSamplingProfile,
        phase_generation: ScreenshotPhaseGeneration,
    ) -> (Self, u128) {
        if source.width == 0 || source.height == 0 {
            let image = ScreenshotImage::empty();
            return (
                Self {
                    phases: ParadePhaseSet::prepare_two_tap(&image, profile),
                    image,
                    corner_insets: Vec::new(),
                },
                0,
            );
        }
        let (width, height, tint) = scaled_style(source, speed, screen_height);
        if matches!(
            phase_generation,
            ScreenshotPhaseGeneration::LinearLanczos3
                | ScreenshotPhaseGeneration::LinearLanczos3Neon
        ) && matches!(profile, ScreenshotSamplingProfile::CrtSixteenth)
        {
            let kernel = match phase_generation {
                ScreenshotPhaseGeneration::LinearLanczos3Neon => LinearPhaseKernel::Neon,
                ScreenshotPhaseGeneration::LinearLanczos3
                | ScreenshotPhaseGeneration::Rgb565TwoTap => LinearPhaseKernel::Scalar,
            };
            if matches!(kernel, LinearPhaseKernel::Neon) {
                validate_neon_phase_kernel();
            }
            let mut styled = scale_lanczos3_linear_tinted(source, width, height, tint);
            apply_depth_cues_linear(&mut styled, speed);
            let coverage = prepare_rounded_coverage(width, height);
            let corner_insets = coverage_corner_insets(&coverage, width, height);
            let depth = speed
                .saturating_sub(PARADE_MIN_TILE_SPEED)
                .min(PARADE_SPEED_COUNT - 1);
            if depth >= 3 {
                rim_card_linear(&mut styled, &corner_insets);
            }
            let phase_started = std::time::Instant::now();
            let premultiplied = premultiply_linear_source(&styled, &coverage);
            let base = prepare_linear_phase(&styled, &coverage, &premultiplied, 0, kernel);
            let shifted = std::array::from_fn(|index| {
                prepare_linear_phase(&styled, &coverage, &premultiplied, index + 1, kernel)
            });
            let phase_us = phase_started.elapsed().as_micros();
            return (
                Self {
                    image: base.image,
                    phases: ParadePhaseSet::SixteenthLinear {
                        base_coverage: base.coverage,
                        shifted: Box::new(shifted),
                    },
                    corner_insets,
                },
                phase_us,
            );
        }
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
        let phases = ParadePhaseSet::prepare_two_tap(&image, profile);
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

fn srgb_to_linear_table() -> &'static [u16; 256] {
    static TABLE: OnceLock<[u16; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|index| {
            let encoded = index as f64 / 255.0;
            let linear = if encoded <= 0.04045 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            };
            (linear * 65535.0).round() as u16
        })
    })
}

fn linear_to_srgb_table() -> &'static [u8; 4097] {
    static TABLE: OnceLock<[u8; 4097]> = OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|index| {
            let linear = index as f64 / 4096.0;
            let encoded = if linear <= 0.003_130_8 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            };
            (encoded * 255.0).round().clamp(0.0, 255.0) as u8
        })
    })
}

fn srgb8_to_linear16(value: u8) -> u16 {
    srgb_to_linear_table()[usize::from(value)]
}

fn rgb565_to_linear(pixel: Rgb565Pixel) -> LinearRgb {
    rgb565_to_linear_with_table(pixel, srgb_to_linear_table())
}

fn rgb565_to_linear_with_table(pixel: Rgb565Pixel, srgb_to_linear: &[u16; 256]) -> LinearRgb {
    let packed = pixel.0;
    let r = (((packed >> 11) & 0x1f) * 255 / 31) as u8;
    let g = (((packed >> 5) & 0x3f) * 255 / 63) as u8;
    let b = ((packed & 0x1f) * 255 / 31) as u8;
    LinearRgb {
        r: srgb_to_linear[usize::from(r)],
        g: srgb_to_linear[usize::from(g)],
        b: srgb_to_linear[usize::from(b)],
    }
}

fn linear_to_rgb565(pixel: LinearRgb) -> Rgb565Pixel {
    linear_to_rgb565_with_table(pixel, linear_to_srgb_table())
}

fn linear_to_rgb565_with_table(pixel: LinearRgb, linear_to_srgb: &[u8; 4097]) -> Rgb565Pixel {
    let convert = |value: u16| {
        let index = (usize::from(value) + 8).min(65_535) >> 4;
        linear_to_srgb[index.min(4096)]
    };
    color565(convert(pixel.r), convert(pixel.g), convert(pixel.b))
}

fn scale_lanczos3_linear_tinted(
    image: &ScreenshotImage,
    out_width: usize,
    out_height: usize,
    tint: u8,
) -> LinearImage {
    if out_width == 0 || out_height == 0 || image.width == 0 || image.height == 0 {
        return LinearImage {
            pixels: Vec::new(),
            width: out_width,
            height: out_height,
            stride: out_width,
        };
    }
    let x_filters = lanczos_filters(image.width, out_width);
    let y_filters = lanczos_filters(image.height, out_height);
    let mut horizontal = vec![LinearRgb::default(); out_width * image.height];
    for source_y in 0..image.height {
        let source_row = source_y * image.stride;
        let target_row = source_y * out_width;
        for (target_x, filter) in x_filters.iter().enumerate() {
            let mut channels = [0_i64; 3];
            for (tap, weight) in filter.weights.iter().enumerate() {
                let pixel = rgb565_to_linear(image.pixels[source_row + filter.start + tap]);
                let weight = i64::from(*weight);
                channels[0] += i64::from(pixel.r) * weight;
                channels[1] += i64::from(pixel.g) * weight;
                channels[2] += i64::from(pixel.b) * weight;
            }
            horizontal[target_row + target_x] = LinearRgb {
                r: fixed_channel(channels[0]),
                g: fixed_channel(channels[1]),
                b: fixed_channel(channels[2]),
            };
        }
    }

    let tint = u64::from(srgb8_to_linear16(tint));
    let mut pixels = vec![LinearRgb::default(); out_width * out_height];
    for (target_y, filter) in y_filters.iter().enumerate() {
        for target_x in 0..out_width {
            let mut channels = [0_i64; 3];
            for (tap, weight) in filter.weights.iter().enumerate() {
                let pixel = horizontal[(filter.start + tap) * out_width + target_x];
                let weight = i64::from(*weight);
                channels[0] += i64::from(pixel.r) * weight;
                channels[1] += i64::from(pixel.g) * weight;
                channels[2] += i64::from(pixel.b) * weight;
            }
            let tinted = |value: i64| {
                let value = u64::from(fixed_channel(value));
                ((value * tint + 32_767) / 65_535).min(65_535) as u16
            };
            pixels[target_y * out_width + target_x] = LinearRgb {
                r: tinted(channels[0]),
                g: tinted(channels[1]),
                b: tinted(channels[2]),
            };
        }
    }
    LinearImage {
        pixels,
        width: out_width,
        height: out_height,
        stride: out_width,
    }
}

fn fixed_channel(value: i64) -> u16 {
    ((value + i64::from(LANCZOS_WEIGHT_ONE / 2)) >> 14).clamp(0, 65_535) as u16
}

fn apply_depth_cues_linear(image: &mut LinearImage, speed: usize) {
    let depth = speed
        .saturating_sub(PARADE_MIN_TILE_SPEED)
        .min(PARADE_SPEED_COUNT - 1);
    let atmosphere = [20_u64, 14, 8, 3, 0][depth];
    let desaturation = [25_u64, 16, 8, 3, 0][depth];
    let blue_haze = u64::from(srgb8_to_linear16(10));
    for pixel in &mut image.pixels {
        let mut r = u64::from(pixel.r);
        let mut g = u64::from(pixel.g);
        let mut b = u64::from(pixel.b);
        let luminance = (77 * r + 150 * g + 29 * b + 128) >> 8;
        r = (r * (100 - desaturation) + luminance * desaturation + 50) / 100;
        g = (g * (100 - desaturation) + luminance * desaturation + 50) / 100;
        b = (b * (100 - desaturation) + luminance * desaturation + 50) / 100;
        r = (r * (100 - atmosphere) + 50) / 100;
        g = (g * (100 - atmosphere) + 50) / 100;
        b = (b * (100 - atmosphere) + blue_haze * atmosphere + 50) / 100;
        pixel.r = r.min(65_535) as u16;
        pixel.g = g.min(65_535) as u16;
        pixel.b = b.min(65_535) as u16;
    }
}

fn blend_linear(from: LinearRgb, to: LinearRgb, alpha: u8) -> LinearRgb {
    let alpha = u64::from(alpha);
    let inverse = 255 - alpha;
    let blend = |from: u16, to: u16| {
        ((u64::from(from) * inverse + u64::from(to) * alpha + 127) / 255) as u16
    };
    LinearRgb {
        r: blend(from.r, to.r),
        g: blend(from.g, to.g),
        b: blend(from.b, to.b),
    }
}

fn rim_card_linear(image: &mut LinearImage, corner_insets: &[u8]) {
    if image.width == 0 || image.height == 0 {
        return;
    }
    let highlight = LinearRgb {
        r: srgb8_to_linear16(210),
        g: srgb8_to_linear16(225),
        b: srgb8_to_linear16(255),
    };
    let shadow = LinearRgb {
        r: 0,
        g: 0,
        b: srgb8_to_linear16(8),
    };
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
                image.pixels[left] = blend_linear(image.pixels[left], highlight, alpha);
            }
            if end > inset + offset {
                let right = row + end - 1 - offset;
                image.pixels[right] =
                    blend_linear(image.pixels[right], shadow, alpha.saturating_add(8));
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
                *pixel = blend_linear(*pixel, color, alpha);
            }
        }
    }
}

fn prepare_rounded_coverage(width: usize, height: usize) -> Vec<u8> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let radius = (width.min(height) / 10).clamp(2, 10) as f64;
    let mut coverage = vec![0_u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut inside = 0usize;
            for sample_y in 0..COVERAGE_SAMPLES_PER_AXIS {
                let py = y as f64 + (sample_y as f64 + 0.5) / COVERAGE_SAMPLES_PER_AXIS as f64;
                for sample_x in 0..COVERAGE_SAMPLES_PER_AXIS {
                    let px = x as f64 + (sample_x as f64 + 0.5) / COVERAGE_SAMPLES_PER_AXIS as f64;
                    let corner_x = if px < radius {
                        Some(radius)
                    } else if px > width as f64 - radius {
                        Some(width as f64 - radius)
                    } else {
                        None
                    };
                    let corner_y = if py < radius {
                        Some(radius)
                    } else if py > height as f64 - radius {
                        Some(height as f64 - radius)
                    } else {
                        None
                    };
                    let sample_inside = match (corner_x, corner_y) {
                        (Some(center_x), Some(center_y)) => {
                            let dx = px - center_x;
                            let dy = py - center_y;
                            dx * dx + dy * dy <= radius * radius
                        }
                        _ => true,
                    };
                    inside += usize::from(sample_inside);
                }
            }
            coverage[y * width + x] =
                ((inside * 255 + COVERAGE_SAMPLE_COUNT / 2) / COVERAGE_SAMPLE_COUNT) as u8;
        }
    }
    coverage
}

fn coverage_corner_insets(coverage: &[u8], width: usize, height: usize) -> Vec<u8> {
    (0..height)
        .map(|y| {
            coverage[y * width..(y + 1) * width]
                .iter()
                .position(|value| *value == 255)
                .unwrap_or(width / 2)
                .min(usize::from(u8::MAX)) as u8
        })
        .collect()
}

fn coverage_plane(values: Vec<u8>, stride: usize, height: usize) -> CoveragePlane {
    let opaque_spans = (0..height)
        .map(|y| {
            let row = &values[y * stride..(y + 1) * stride];
            let start = row.iter().position(|value| *value == 255).unwrap_or(0);
            let end = row
                .iter()
                .rposition(|value| *value == 255)
                .map_or(start, |index| index + 1);
            OpaqueSpan {
                start: start.min(usize::from(u16::MAX)) as u16,
                end: end.min(usize::from(u16::MAX)) as u16,
            }
        })
        .collect();
    CoveragePlane {
        values,
        opaque_spans,
        stride,
    }
}

fn fractional_delay_weights(phase: usize) -> [i32; 6] {
    debug_assert!((1..CRT_PHASE_COUNT).contains(&phase));
    let delay = phase as f64 / CRT_PHASE_COUNT as f64;
    let float_weights = std::array::from_fn::<_, 6, _>(|tap| lanczos3(tap as f64 - 3.0 + delay));
    let sum = float_weights.iter().sum::<f64>();
    let mut weights =
        float_weights.map(|weight| (weight / sum * f64::from(LANCZOS_WEIGHT_ONE)).round() as i32);
    let fixed_sum = weights.iter().sum::<i32>();
    weights[3] += LANCZOS_WEIGHT_ONE - fixed_sum;
    weights
}

fn premultiply_linear_source(image: &LinearImage, coverage: &[u8]) -> Vec<[u16; 4]> {
    image
        .pixels
        .iter()
        .zip(coverage)
        .map(|(pixel, coverage)| {
            let alpha = u16::from(*coverage) * 257;
            let premultiply = |channel: u16| {
                if alpha == 65_535 {
                    channel
                } else {
                    ((u64::from(channel) * u64::from(alpha) + 32_767) / 65_535) as u16
                }
            };
            [
                premultiply(pixel.r),
                premultiply(pixel.g),
                premultiply(pixel.b),
                alpha,
            ]
        })
        .collect()
}

fn prepare_linear_phase(
    image: &LinearImage,
    source_coverage: &[u8],
    premultiplied_source: &[[u16; 4]],
    phase: usize,
    kernel: LinearPhaseKernel,
) -> PreparedLinearPhase {
    let width = image.width + usize::from(phase != 0);
    let mut pixels = vec![Rgb565Pixel(0); width * image.height];
    let mut coverage = vec![0_u8; width * image.height];
    if phase == 0 {
        for y in 0..image.height {
            for x in 0..image.width {
                let index = y * image.width + x;
                coverage[index] = source_coverage[index];
                pixels[index] = linear_to_rgb565(image.pixels[y * image.stride + x]);
            }
        }
    } else {
        let weights = fractional_delay_weights(phase);
        let neon_reconstruction = reconstruct_linear_phase_neon_if_selected(
            premultiplied_source,
            image.width,
            image.height,
            width,
            weights,
            kernel,
        );
        for y in 0..image.height {
            for out_x in 0..width {
                let target = y * width + out_x;
                let reconstructed = neon_reconstruction.as_ref().map_or_else(
                    || {
                        let samples = std::array::from_fn(|tap| {
                            let source_x = out_x as isize + tap as isize - 3;
                            if (0..image.width as isize).contains(&source_x) {
                                let source_x = source_x as usize;
                                premultiplied_source[y * image.width + source_x]
                            } else {
                                [0; 4]
                            }
                        });
                        reconstruct_six_tap_scalar(&samples, weights)
                    },
                    |reconstruction| reconstruction[target],
                );
                let premultiplied = [reconstructed[0], reconstructed[1], reconstructed[2]];
                let alpha = reconstructed[3];
                coverage[target] = ((u32::from(alpha) + 128) / 257).min(255) as u8;
                if alpha > 0 {
                    let unpremultiply = |channel: u16| {
                        if alpha == 65_535 {
                            channel
                        } else {
                            ((u64::from(channel) * 65_535 + u64::from(alpha) / 2)
                                / u64::from(alpha))
                            .min(65_535) as u16
                        }
                    };
                    pixels[target] = linear_to_rgb565(LinearRgb {
                        r: unpremultiply(premultiplied[0]),
                        g: unpremultiply(premultiplied[1]),
                        b: unpremultiply(premultiplied[2]),
                    });
                }
            }
        }
    }
    PreparedLinearPhase {
        image: ScreenshotImage {
            pixels,
            width,
            height: image.height,
            stride: width,
        },
        coverage: coverage_plane(coverage, width, image.height),
    }
}

#[inline(always)]
fn reconstruct_six_tap_scalar(samples: &[[u16; 4]; 6], weights: [i32; 6]) -> [u16; 4] {
    let mut sums = [0_i32; 4];
    let mut minima = [u16::MAX; 4];
    let mut maxima = [0_u16; 4];
    for (sample, weight) in samples.iter().zip(weights) {
        for channel in 0..4 {
            sums[channel] += i32::from(sample[channel]) * weight;
            minima[channel] = minima[channel].min(sample[channel]);
            maxima[channel] = maxima[channel].max(sample[channel]);
        }
    }
    std::array::from_fn(|channel| {
        let value = (i64::from(sums[channel]) + i64::from(LANCZOS_WEIGHT_ONE / 2)) >> 14;
        value.clamp(i64::from(minima[channel]), i64::from(maxima[channel])) as u16
    })
}

fn reconstruct_linear_phase_neon_if_selected(
    source: &[[u16; 4]],
    source_width: usize,
    height: usize,
    output_width: usize,
    weights: [i32; 6],
    kernel: LinearPhaseKernel,
) -> Option<Vec<[u16; 4]>> {
    if !matches!(kernel, LinearPhaseKernel::Neon) {
        return None;
    }
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    {
        let mut output = vec![[0_u16; 4]; output_width * height];
        // SAFETY: MiSTer hardware is Cortex-A9 with NEON. Both slices describe
        // the complete source and destination planes passed to the kernel.
        unsafe {
            reconstruct_linear_phase_neon(
                source,
                source_width,
                height,
                output_width,
                weights,
                &mut output,
            );
        }
        return Some(output);
    }
    #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
    {
        let _ = (source, source_width, height, output_width, weights);
        None
    }
}

fn validate_neon_phase_kernel() {
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    {
        use std::sync::OnceLock;

        static VALIDATED: OnceLock<()> = OnceLock::new();
        VALIDATED.get_or_init(|| {
            let source_width = 9;
            let height = 2;
            let source = (0..source_width * height)
                .map(|index| {
                    let value = index as u16;
                    [
                        value.wrapping_mul(3_641),
                        value.wrapping_mul(7_919).wrapping_add(65_535),
                        value.wrapping_mul(13_337).wrapping_add(257),
                        value.wrapping_mul(4_093),
                    ]
                })
                .collect::<Vec<_>>();
            let output_width = source_width + 1;
            for phase in 1..CRT_PHASE_COUNT {
                let weights = fractional_delay_weights(phase);
                let mut actual = vec![[0_u16; 4]; output_width * height];
                // SAFETY: this validation runs only on the MiSTer ARM target,
                // whose Cortex-A9 provides NEON, with complete source/output planes.
                unsafe {
                    reconstruct_linear_phase_neon(
                        &source,
                        source_width,
                        height,
                        output_width,
                        weights,
                        &mut actual,
                    );
                }
                let expected = (0..height)
                    .flat_map(|y| {
                        let source = &source;
                        (0..output_width).map(move |out_x| {
                            let samples = std::array::from_fn(|tap| {
                                let source_x = out_x as isize + tap as isize - 3;
                                if (0..source_width as isize).contains(&source_x) {
                                    source[y * source_width + source_x as usize]
                                } else {
                                    [0; 4]
                                }
                            });
                            reconstruct_six_tap_scalar(&samples, weights)
                        })
                    })
                    .collect::<Vec<_>>();
                assert_eq!(actual, expected, "NEON phase {phase} differs from scalar");
            }
        });
    }
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
unsafe fn reconstruct_linear_phase_neon(
    source: &[[u16; 4]],
    source_width: usize,
    height: usize,
    output_width: usize,
    weights: [i32; 6],
    output: &mut [[u16; 4]],
) {
    unsafe extern "C" {
        fn mister_magik_screenshot_phase_neon(
            source: *const u16,
            source_width: usize,
            height: usize,
            output_width: usize,
            weights: *const i32,
            output: *mut u16,
        );
    }

    // SAFETY: callers provide complete, non-overlapping source and output
    // planes. The C kernel reads six weights and writes four lanes per output.
    unsafe {
        mister_magik_screenshot_phase_neon(
            source.as_ptr().cast(),
            source_width,
            height,
            output_width,
            weights.as_ptr(),
            output.as_mut_ptr().cast(),
        );
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
    if let Some(base_coverage) = phase_set.base_coverage() {
        if quantized.phase == 0 {
            blit_coverage_phase(
                dst,
                screen_width,
                screen_height,
                image,
                base_coverage,
                quantized.x,
                y,
            );
            return;
        }
        if let Some(shifted) = phase_set.linear_phase(quantized.phase) {
            blit_coverage_phase(
                dst,
                screen_width,
                screen_height,
                shifted.image,
                shifted.coverage,
                quantized.x,
                y,
            );
            return;
        }
        debug_assert!(false, "linear card missing sixteenth-pixel phase");
    }
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
    let Some(shifted) = phase_set.two_tap_phase(quantized.phase) else {
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
fn blit_coverage_phase(
    dst: &mut [Rgb565Pixel],
    screen_width: usize,
    screen_height: usize,
    image: &ScreenshotImage,
    coverage: &CoveragePlane,
    x: isize,
    y: isize,
) {
    let srgb_to_linear = srgb_to_linear_table();
    let linear_to_srgb = linear_to_srgb_table();
    for source_y in 0..image.height {
        let target_y = y + source_y as isize;
        if target_y < 0 || target_y >= screen_height as isize {
            continue;
        }
        let source_x0 = (-x).max(0) as usize;
        let source_x1 = (screen_width as isize - x).clamp(0, image.width as isize) as usize;
        if source_x1 <= source_x0 {
            continue;
        }
        let source_row = source_y * image.stride;
        let coverage_row = source_y * coverage.stride;
        let target_row = target_y as usize * screen_width;
        let span = coverage
            .opaque_spans
            .get(source_y)
            .copied()
            .unwrap_or_default();
        let opaque_start = usize::from(span.start).clamp(source_x0, source_x1);
        let opaque_end = usize::from(span.end).clamp(opaque_start, source_x1);
        for source_x in source_x0..opaque_start {
            composite_coverage_pixel(
                dst,
                target_row + (x + source_x as isize) as usize,
                image.pixels[source_row + source_x],
                coverage.values[coverage_row + source_x],
                srgb_to_linear,
                linear_to_srgb,
            );
        }
        if opaque_end > opaque_start {
            let target_start = target_row + (x + opaque_start as isize) as usize;
            let copy_len = opaque_end - opaque_start;
            dst[target_start..target_start + copy_len]
                .copy_from_slice(&image.pixels[source_row + opaque_start..source_row + opaque_end]);
        }
        for source_x in opaque_end..source_x1 {
            composite_coverage_pixel(
                dst,
                target_row + (x + source_x as isize) as usize,
                image.pixels[source_row + source_x],
                coverage.values[coverage_row + source_x],
                srgb_to_linear,
                linear_to_srgb,
            );
        }
    }
}

fn composite_coverage_pixel(
    dst: &mut [Rgb565Pixel],
    target: usize,
    foreground: Rgb565Pixel,
    coverage: u8,
    srgb_to_linear: &[u16; 256],
    linear_to_srgb: &[u8; 4097],
) {
    if coverage == 0 {
        return;
    }
    if coverage == 255 {
        dst[target] = foreground;
        return;
    }
    let background = rgb565_to_linear_with_table(dst[target], srgb_to_linear);
    let foreground = rgb565_to_linear_with_table(foreground, srgb_to_linear);
    let alpha = u64::from(coverage);
    let inverse = 255 - alpha;
    let composite = |background: u16, foreground: u16| {
        ((u64::from(background) * inverse + u64::from(foreground) * alpha + 127) / 255).min(65_535)
            as u16
    };
    dst[target] = linear_to_rgb565_with_table(
        LinearRgb {
            r: composite(background.r, foreground.r),
            g: composite(background.g, foreground.g),
            b: composite(background.b, foreground.b),
        },
        linear_to_srgb,
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

    fn linear_phases(card: &PreparedScreenshotCard) -> (&CoveragePlane, &[PreparedLinearPhase]) {
        let ParadePhaseSet::SixteenthLinear {
            base_coverage,
            shifted,
        } = &card.phases
        else {
            panic!("card did not use linear Lanczos phases");
        };
        (base_coverage, shifted.as_slice())
    }

    fn coverage_centroid(coverage: &CoveragePlane, width: usize, height: usize) -> f64 {
        let mut weighted = 0_f64;
        let mut total = 0_f64;
        for y in 0..height {
            for x in 0..width {
                let value = f64::from(coverage.values[y * coverage.stride + x]);
                weighted += (x as f64 + 0.5) * value;
                total += value;
            }
        }
        weighted / total
    }

    fn total_coverage(coverage: &CoveragePlane) -> u64 {
        coverage.values.iter().map(|value| u64::from(*value)).sum()
    }

    fn horizontal_edge_energy(coverage: &CoveragePlane, width: usize, height: usize) -> u64 {
        let mut energy = 0_u64;
        for y in 0..height {
            let mut previous = 0_i32;
            for x in 0..width {
                let current = i32::from(coverage.values[y * coverage.stride + x]);
                energy += u64::from(current.abs_diff(previous));
                previous = current;
            }
            energy += previous.unsigned_abs() as u64;
        }
        energy
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

    #[test]
    fn linear_lanczos_phases_keep_coverage_centroid_within_one_thirty_second_pixel() {
        let source = test_image(32, 24);
        let card = PreparedScreenshotCard::prepare_with_generation(
            &source,
            5,
            135,
            ScreenshotSamplingProfile::CrtSixteenth,
            ScreenshotPhaseGeneration::LinearLanczos3,
        );
        let (base, shifted) = linear_phases(&card);
        let base_centroid = coverage_centroid(base, card.width(), card.height());
        for phase in 1..CRT_PHASE_COUNT {
            let phase_coverage = &shifted[phase - 1].coverage;
            let centroid = coverage_centroid(
                phase_coverage,
                shifted[phase - 1].image.width,
                card.height(),
            );
            let expected = base_centroid + phase as f64 / CRT_PHASE_COUNT as f64;
            assert!(
                (centroid - expected).abs() <= 1.0 / 32.0,
                "phase={phase} centroid={centroid} expected={expected}"
            );
        }
    }

    #[test]
    fn linear_lanczos_phases_preserve_coverage_and_edge_energy() {
        let source = test_image(32, 24);
        let card = PreparedScreenshotCard::prepare_with_generation(
            &source,
            5,
            135,
            ScreenshotSamplingProfile::CrtSixteenth,
            ScreenshotPhaseGeneration::LinearLanczos3,
        );
        let (base, shifted) = linear_phases(&card);
        let base_coverage = total_coverage(base);
        let base_energy = horizontal_edge_energy(base, card.width(), card.height());
        for (index, phase) in shifted.iter().enumerate() {
            let coverage = total_coverage(&phase.coverage);
            let energy = horizontal_edge_energy(&phase.coverage, phase.image.width, card.height());
            assert!(
                coverage.abs_diff(base_coverage) <= card.height() as u64 * 8,
                "phase={} coverage={coverage} base={base_coverage}",
                index + 1
            );
            assert!(
                energy.abs_diff(base_energy) * 100 <= base_energy * 5,
                "phase={} energy={energy} base={base_energy}",
                index + 1
            );
        }
    }

    #[test]
    fn linear_lanczos_card_has_antialiased_corners_and_bounded_storage() {
        let source = test_image(32, 24);
        let card = PreparedScreenshotCard::prepare_with_generation(
            &source,
            5,
            540,
            ScreenshotSamplingProfile::CrtSixteenth,
            ScreenshotPhaseGeneration::LinearLanczos3,
        );
        let (base, shifted) = linear_phases(&card);
        assert!(
            base.values
                .iter()
                .any(|coverage| (1..255).contains(coverage))
        );
        assert_eq!(
            base.values[(card.height() / 2) * base.stride + card.width() / 2],
            255
        );
        assert!(base.opaque_spans.iter().all(|span| span.end > span.start));
        assert_eq!(shifted.len(), CRT_SHIFTED_PHASE_COUNT);
        assert!(card.phase_resident_bytes() < 1_000_000);
    }

    #[test]
    fn linear_lanczos_phase_generation_is_deterministic() {
        let source = test_image(32, 24);
        let prepare = || {
            PreparedScreenshotCard::prepare_with_generation(
                &source,
                4,
                270,
                ScreenshotSamplingProfile::CrtSixteenth,
                ScreenshotPhaseGeneration::LinearLanczos3,
            )
        };
        let first = prepare();
        let second = prepare();
        assert_eq!(first.image, second.image);
        let (first_base, first_shifted) = linear_phases(&first);
        let (second_base, second_shifted) = linear_phases(&second);
        assert_eq!(first_base.values, second_base.values);
        assert_eq!(first_base.opaque_spans, second_base.opaque_spans);
        for (first, second) in first_shifted.iter().zip(second_shifted) {
            assert_eq!(first.image, second.image);
            assert_eq!(first.coverage.values, second.coverage.values);
            assert_eq!(first.coverage.opaque_spans, second.coverage.opaque_spans);
        }
    }

    #[test]
    fn neon_linear_lanczos_backend_is_pixel_identical_to_scalar() {
        let source = test_image(32, 24);
        let prepare = |generation| {
            PreparedScreenshotCard::prepare_with_generation(
                &source,
                4,
                270,
                ScreenshotSamplingProfile::CrtSixteenth,
                generation,
            )
        };
        let scalar = prepare(ScreenshotPhaseGeneration::LinearLanczos3);
        let neon = prepare(ScreenshotPhaseGeneration::LinearLanczos3Neon);
        assert_eq!(scalar.image, neon.image);
        let (scalar_base, scalar_shifted) = linear_phases(&scalar);
        let (neon_base, neon_shifted) = linear_phases(&neon);
        assert_eq!(scalar_base.values, neon_base.values);
        assert_eq!(scalar_base.opaque_spans, neon_base.opaque_spans);
        for (scalar, neon) in scalar_shifted.iter().zip(neon_shifted) {
            assert_eq!(scalar.image, neon.image);
            assert_eq!(scalar.coverage.values, neon.coverage.values);
            assert_eq!(scalar.coverage.opaque_spans, neon.coverage.opaque_spans);
        }
    }
}
