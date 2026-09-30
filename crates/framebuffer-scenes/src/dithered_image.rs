// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Destination-space quantisation for explicit image quality experiments.
use crate::Rgb565Pixel;
const BAYER: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
#[inline]
pub fn quantise_rgb8(rgb: [u8; 3], x: usize, y: usize) -> Rgb565Pixel {
    let threshold = BAYER[y % 4][x % 4] * 16 + 8;
    let channel = |v: u8, levels: u32| {
        let scaled = u32::from(v) * levels;
        (scaled / 255 + u32::from(scaled % 255 * 256 > threshold * 255)).min(levels) as u16
    };
    Rgb565Pixel(channel(rgb[0], 31) << 11 | channel(rgb[1], 63) << 5 | channel(rgb[2], 31))
}
#[cfg(test)]
mod tests {
    use super::*;
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
