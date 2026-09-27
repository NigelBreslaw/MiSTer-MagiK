// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic ordered-dithered RGB565 gradients.

use crate::Rgb565Pixel;

const BAYER_8X8: [[u8; 8]; 8] = [
    [0, 48, 12, 60, 3, 51, 15, 63],
    [32, 16, 44, 28, 35, 19, 47, 31],
    [8, 56, 4, 52, 11, 59, 7, 55],
    [40, 24, 36, 20, 43, 27, 39, 23],
    [2, 50, 14, 62, 1, 49, 13, 61],
    [34, 18, 46, 30, 33, 17, 45, 29],
    [10, 58, 6, 54, 9, 57, 5, 53],
    [42, 26, 38, 22, 41, 25, 37, 21],
];

const FRACTION_BITS: u32 = 16;
const FRACTION_ONE: u64 = 1 << FRACTION_BITS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb8Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Rgb8Color {
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HorizontalGradientStop {
    position: u16,
    color: Rgb8Color,
}

impl HorizontalGradientStop {
    #[must_use]
    pub const fn percent(position: u8, color: Rgb8Color) -> Self {
        assert!(
            position <= 100,
            "gradient stop percentage must be at most 100"
        );
        Self {
            position: ((position as u32 * u16::MAX as u32) / 100) as u16,
            color,
        }
    }

    #[must_use]
    pub const fn color(self) -> Rgb8Color {
        self.color
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DitheredGradientError {
    EmptyGeometry,
    TooFewStops,
    StopsOutOfOrder,
    MissingEndpoints,
    BufferSizeOverflow,
}

/// Renders an opaque horizontal sRGB gradient directly onto the RGB565 lattice.
///
/// Each channel is interpolated in encoded sRGB, matching Slint's opaque linear
/// gradient semantics. An 8x8 ordered threshold selects between the two nearest
/// representable RGB565 levels instead of truncating every sample toward zero.
pub fn horizontal_rgb565(
    width: usize,
    height: usize,
    stops: &[HorizontalGradientStop],
) -> Result<Vec<Rgb565Pixel>, DitheredGradientError> {
    if width == 0 || height == 0 {
        return Err(DitheredGradientError::EmptyGeometry);
    }
    if stops.len() < 2 {
        return Err(DitheredGradientError::TooFewStops);
    }
    if stops.first().is_none_or(|stop| stop.position != 0)
        || stops.last().is_none_or(|stop| stop.position != u16::MAX)
    {
        return Err(DitheredGradientError::MissingEndpoints);
    }
    if stops
        .windows(2)
        .any(|pair| pair[0].position >= pair[1].position)
    {
        return Err(DitheredGradientError::StopsOutOfOrder);
    }

    let pixel_count = width
        .checked_mul(height)
        .ok_or(DitheredGradientError::BufferSizeOverflow)?;
    let mut pixels = Vec::with_capacity(pixel_count);
    for y in 0..height {
        for x in 0..width {
            let position = normalized_position(x, width);
            let (left, right) = enclosing_stops(position, stops);
            let threshold = dither_threshold(x, y);
            let red = quantize_channel(
                interpolate_channel(position, left, right, |color| color.red),
                31,
                threshold,
            );
            let green = quantize_channel(
                interpolate_channel(position, left, right, |color| color.green),
                63,
                threshold,
            );
            let blue = quantize_channel(
                interpolate_channel(position, left, right, |color| color.blue),
                31,
                threshold,
            );
            pixels.push(Rgb565Pixel((red << 11) | (green << 5) | blue));
        }
    }
    Ok(pixels)
}

fn normalized_position(x: usize, width: usize) -> u16 {
    if width == 1 {
        return 0;
    }
    ((x as u64 * u16::MAX as u64) / (width - 1) as u64) as u16
}

fn enclosing_stops(
    position: u16,
    stops: &[HorizontalGradientStop],
) -> (HorizontalGradientStop, HorizontalGradientStop) {
    stops
        .windows(2)
        .find_map(|pair| (position <= pair[1].position).then_some((pair[0], pair[1])))
        .unwrap_or_else(|| {
            let last = stops[stops.len() - 1];
            (last, last)
        })
}

fn interpolate_channel(
    position: u16,
    left: HorizontalGradientStop,
    right: HorizontalGradientStop,
    channel: impl Fn(Rgb8Color) -> u8,
) -> u64 {
    if left.position == right.position {
        return u64::from(channel(left.color)) * FRACTION_ONE;
    }
    let span = u64::from(right.position - left.position);
    let offset = u64::from(position - left.position);
    let start = i64::from(channel(left.color));
    let delta = i64::from(channel(right.color)) - start;
    let interpolated =
        start * FRACTION_ONE as i64 + delta * offset as i64 * FRACTION_ONE as i64 / span as i64;
    interpolated.max(0) as u64
}

fn dither_threshold(x: usize, y: usize) -> u64 {
    let rank = u64::from(BAYER_8X8[y & 7][x & 7]);
    ((rank * 2 + 1) * FRACTION_ONE) / 128
}

fn quantize_channel(value_q16: u64, maximum: u16, threshold: u64) -> u16 {
    let scaled = value_q16 * u64::from(maximum) / 255;
    let base = (scaled >> FRACTION_BITS).min(u64::from(maximum));
    let fraction = scaled & (FRACTION_ONE - 1);
    let increment = u64::from(base < u64::from(maximum) && fraction >= threshold);
    (base + increment) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: Rgb8Color = Rgb8Color::new(0, 0, 0);
    const WHITE: Rgb8Color = Rgb8Color::new(255, 255, 255);

    #[test]
    fn rejects_invalid_contracts() {
        let valid = [
            HorizontalGradientStop::percent(0, BLACK),
            HorizontalGradientStop::percent(100, WHITE),
        ];
        assert_eq!(
            horizontal_rgb565(0, 1, &valid),
            Err(DitheredGradientError::EmptyGeometry)
        );
        assert_eq!(
            horizontal_rgb565(1, 1, &valid[..1]),
            Err(DitheredGradientError::TooFewStops)
        );
        assert_eq!(
            horizontal_rgb565(
                1,
                1,
                &[
                    HorizontalGradientStop::percent(1, BLACK),
                    HorizontalGradientStop::percent(100, WHITE),
                ],
            ),
            Err(DitheredGradientError::MissingEndpoints)
        );
        assert_eq!(
            horizontal_rgb565(
                1,
                1,
                &[
                    HorizontalGradientStop::percent(0, BLACK),
                    HorizontalGradientStop::percent(60, WHITE),
                    HorizontalGradientStop::percent(60, BLACK),
                    HorizontalGradientStop::percent(100, WHITE),
                ],
            ),
            Err(DitheredGradientError::StopsOutOfOrder)
        );
    }

    #[test]
    fn exact_lattice_endpoints_survive() {
        let red = Rgb8Color::new(255, 0, 0);
        let blue = Rgb8Color::new(0, 0, 255);
        let pixels = horizontal_rgb565(
            2,
            8,
            &[
                HorizontalGradientStop::percent(0, red),
                HorizontalGradientStop::percent(100, blue),
            ],
        )
        .unwrap();
        for row in pixels.as_chunks::<2>().0 {
            assert_eq!(*row, [Rgb565Pixel(0xf800), Rgb565Pixel(0x001f)]);
        }
    }

    #[test]
    fn flat_fraction_uses_the_expected_bayer_coverage() {
        let fractional_red_step = Rgb8Color::new(4, 0, 0);
        let pixels = horizontal_rgb565(
            8,
            8,
            &[
                HorizontalGradientStop::percent(0, fractional_red_step),
                HorizontalGradientStop::percent(100, fractional_red_step),
            ],
        )
        .unwrap();
        assert_eq!(pixels.iter().filter(|pixel| pixel.0 == 0x0800).count(), 31);
        assert_eq!(pixels.iter().filter(|pixel| pixel.0 == 0).count(), 33);
    }

    #[test]
    fn settings_gradient_breaks_up_rgb565_bands() {
        let pixels = horizontal_rgb565(
            638,
            36,
            &[
                HorizontalGradientStop::percent(0, Rgb8Color::new(0x22, 0x18, 0x43)),
                HorizontalGradientStop::percent(60, Rgb8Color::new(0x12, 0x0d, 0x24)),
                HorizontalGradientStop::percent(100, BLACK),
            ],
        )
        .unwrap();
        let row = &pixels[4 * 638..5 * 638];
        let longest_run = row
            .chunk_by(|left, right| left == right)
            .map(<[Rgb565Pixel]>::len)
            .max()
            .unwrap();
        assert!(
            longest_run <= 16,
            "longest flat run was {longest_run} pixels"
        );
    }
}
