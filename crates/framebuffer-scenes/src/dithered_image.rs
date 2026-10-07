// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Destination-space RGB565 quantisation for images and card projection.
use crate::Rgb565Pixel;
const BAYER: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
/// Nearest representable RGB565 colour, independent of screen coordinates.
#[inline]
pub(crate) fn quantise_nearest_rgb8(rgb: [u8; 3]) -> Rgb565Pixel {
    let channel = |value: u8, levels: u32| ((u32::from(value) * levels + 127) / 255) as u16;
    Rgb565Pixel(channel(rgb[0], 31) << 11 | channel(rgb[1], 63) << 5 | channel(rgb[2], 31))
}

#[inline]
pub fn quantise_rgb8(rgb: [u8; 3], x: usize, y: usize) -> Rgb565Pixel {
    let threshold = BAYER[y % 4][x % 4] * 16 + 8;
    let channel = |v: u8, levels: u32| {
        let scaled = u32::from(v) * levels;
        (scaled / 255 + u32::from(scaled % 255 * 256 > threshold * 255)).min(levels) as u16
    };
    Rgb565Pixel(channel(rgb[0], 31) << 11 | channel(rgb[1], 63) << 5 | channel(rgb[2], 31))
}
/// Experimental centred Bayer noise and saturated RGB565 bit quantisation.
#[cfg(feature = "card-fast-quantisation")]
#[inline]
pub(crate) fn quantise_fast_rgb8(rgb: [u8; 3], x: usize, y: usize) -> Rgb565Pixel {
    let rank = BAYER[y & 3][x & 3] as i16;
    let channel = |value: u8, shift: u32, bias: i16, maximum: i16| {
        ((i16::from(value) + bias).max(0) >> shift).min(maximum) as u16
    };
    Rgb565Pixel(
        channel(rgb[0], 3, (rank >> 1) - 4, 31) << 11
            | channel(rgb[1], 2, (rank >> 2) - 2, 63) << 5
            | channel(rgb[2], 3, (rank >> 1) - 4, 31),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nearest_conversion_minimises_channel_error_and_preserves_native_colours() {
        for value in 0..=255u8 {
            let actual = quantise_nearest_rgb8([value; 3]).0;
            for (shift, maximum) in [(11, 31), (5, 63), (0, 31)] {
                let expected = (f64::from(value) * f64::from(maximum) / 255.0).round() as u16;
                assert_eq!((actual >> shift) & maximum, expected);
            }
        }
        for pixel in 0..=u16::MAX {
            let (r, g, b) = (pixel >> 11, (pixel >> 5) & 63, pixel & 31);
            let rgb = [r * 255 / 31, g * 255 / 63, b * 255 / 31].map(|v| v as u8);
            assert_eq!(quantise_nearest_rgb8(rgb).0, pixel);
        }
    }

    #[cfg(feature = "card-fast-quantisation")]
    #[test]
    fn candidate_preserves_endpoints_and_monotonic_channel_response() {
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(quantise_fast_rgb8([0; 3], x, y).0, 0);
                assert_eq!(quantise_fast_rgb8([255; 3], x, y).0, 0xffff);
                for c in 0..3 {
                    let mut previous = 0;
                    for value in 0..=255u8 {
                        let mut rgb = [0; 3];
                        rgb[c] = value;
                        let p = quantise_fast_rgb8(rgb, x, y).0;
                        let actual = match c {
                            0 => p >> 11,
                            1 => (p >> 5) & 63,
                            _ => p & 31,
                        };
                        assert!(actual >= previous);
                        previous = actual;
                    }
                }
            }
        }
    }

    #[test]
    fn arm_reciprocal_quantisation_matches_division_for_every_channel() {
        for levels in [31, 63] {
            for v in 0..=255 {
                let scaled = v * levels;
                let biased = scaled + 1;
                let q = (biased + (biased >> 8)) >> 8;
                assert_eq!(q, scaled / 255);
                for t in 0..16 {
                    let threshold = t * 16 + 8;
                    assert_eq!(
                        q + u32::from((scaled - q * 255) * 256 > threshold * 255),
                        scaled / 255 + u32::from(scaled % 255 * 256 > threshold * 255)
                    );
                }
            }
        }
    }
    #[test]
    fn collapsed_ordered_rounding_is_exact_for_every_channel_and_phase() {
        for levels in [31, 63] {
            for value in 0..=255 {
                for phase in 0..16 {
                    let threshold = phase * 16 + 8;
                    let scaled = value * levels;
                    let expected = scaled / 255 + u32::from((scaled % 255) * 256 > threshold * 255);
                    let n = scaled + 256 - threshold;
                    assert_eq!((n + (n >> 8)) >> 8, expected);
                    assert!(n + (n >> 8) <= u16::MAX as u32);
                }
            }
        }
    }
    #[test]
    fn endpoints_and_spatial_average_are_preserved() {
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(quantise_rgb8([0; 3], x, y).0, 0);
                assert_eq!(quantise_rgb8([255; 3], x, y).0, 0xffff);
            }
        }
        for v in 1..255u8 {
            let sum: u32 = (0..16)
                .map(|i| u32::from(quantise_rgb8([v; 3], i % 4, i / 4).0 >> 11))
                .sum();
            assert!((sum as f64 / 16.0 - f64::from(v) * 31.0 / 255.0).abs() <= 1.0 / 16.0);
        }
    }
}
