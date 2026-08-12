// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use slint::platform::software_renderer::Rgb565Pixel;
#[cfg(feature = "bench-tools")]
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct Rgb565FrameView<'a> {
    pub pixels: &'a [Rgb565Pixel],
    pub width: usize,
    pub height: usize,
    pub stride_pixels: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownsampledGeometry {
    pub width: usize,
    pub height: usize,
}

pub fn downsample_rgb565_2x(
    source: Rgb565FrameView<'_>,
    destination: &mut Vec<Rgb565Pixel>,
) -> Result<DownsampledGeometry, &'static str> {
    validate_source(source)?;
    let geometry = DownsampledGeometry {
        width: source.width.div_ceil(2),
        height: source.height.div_ceil(2),
    };
    let output_len = geometry
        .width
        .checked_mul(geometry.height)
        .ok_or("downsampled RGB565 geometry overflows")?;
    destination.resize(output_len, Rgb565Pixel(0));

    #[cfg(mister_arm_neon_decimator)]
    {
        // SAFETY: validate_source proves every selected source row contains
        // source.width pixels, and destination was resized for every output row.
        unsafe { downsample_rgb565_2x_neon(source, destination, geometry) };
    }
    #[cfg(not(mister_arm_neon_decimator))]
    downsample_rgb565_2x_scalar(source, destination, geometry);

    Ok(geometry)
}

fn validate_source(source: Rgb565FrameView<'_>) -> Result<(), &'static str> {
    if source.width == 0 || source.height == 0 || source.stride_pixels < source.width {
        return Err("invalid RGB565 source geometry");
    }
    let required = source
        .stride_pixels
        .checked_mul(source.height)
        .ok_or("RGB565 source geometry overflows")?;
    if source.pixels.len() < required {
        return Err("RGB565 source buffer is shorter than its geometry");
    }
    Ok(())
}

#[cfg(any(not(mister_arm_neon_decimator), test))]
fn downsample_rgb565_2x_scalar(
    source: Rgb565FrameView<'_>,
    destination: &mut [Rgb565Pixel],
    geometry: DownsampledGeometry,
) {
    for output_y in 0..geometry.height {
        let source_y = output_y * 2;
        let source_start = source_y * source.stride_pixels;
        let source_row = &source.pixels[source_start..source_start + source.width];
        let destination_start = output_y * geometry.width;
        let destination_row =
            &mut destination[destination_start..destination_start + geometry.width];

        for (output_x, pixel) in destination_row.iter_mut().enumerate() {
            *pixel = source_row[output_x * 2];
        }
    }
}

#[cfg(mister_arm_neon_decimator)]
unsafe fn downsample_rgb565_2x_neon(
    source: Rgb565FrameView<'_>,
    destination: &mut [Rgb565Pixel],
    geometry: DownsampledGeometry,
) {
    unsafe extern "C" {
        fn mister_magik_downsample_rgb565_2x_neon(
            source: *const u16,
            source_width: usize,
            source_height: usize,
            source_stride: usize,
            destination: *mut u16,
            destination_width: usize,
        );
    }

    // SAFETY: the safe caller validates the source and destination lengths,
    // strides, and fixed 2x geometry before selecting the scalar FFI path.
    unsafe {
        mister_magik_downsample_rgb565_2x_neon(
            source.pixels.as_ptr().cast::<u16>(),
            source.width,
            source.height,
            source.stride_pixels,
            destination.as_mut_ptr().cast::<u16>(),
            geometry.width,
        );
    }
}

pub const fn downsample_implementation() -> &'static str {
    if cfg!(mister_arm_neon_decimator) {
        "neon-even-lanes"
    } else {
        "scalar"
    }
}

#[cfg(feature = "bench-tools")]
const SCALAR_BENCH_DEFAULT_SAMPLES: usize = 200;

/// Measure the exact production scalar decimator without starting the Slint UI.
#[cfg(feature = "bench-tools")]
pub fn run_scalar_bench() -> bool {
    let samples = std::env::var("MISTER_STREAM_SCALAR_BENCH_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(SCALAR_BENCH_DEFAULT_SAMPLES);
    let cases = [
        (
            "full_960x540",
            960usize,
            540usize,
            960usize,
            0xf812_3960_21e9_0488u64,
        ),
        (
            "padded_960x540",
            960usize,
            540usize,
            976usize,
            0x5cdc_6b4d_35d5_c5f5u64,
        ),
        (
            "odd_959x539",
            959usize,
            539usize,
            967usize,
            0x877c_ca3e_815d_92cbu64,
        ),
    ];

    let mut valid = true;
    for (name, width, height, stride_pixels, expected_checksum) in cases {
        let pixels = deterministic_pixels(stride_pixels * height);
        let source = Rgb565FrameView {
            pixels: &pixels,
            width,
            height,
            stride_pixels,
        };
        let mut destination = Vec::new();
        for _ in 0..20 {
            downsample_rgb565_2x(source, &mut destination).expect("valid scalar benchmark");
        }
        let mut durations = Vec::with_capacity(samples);
        for _ in 0..samples {
            let started = Instant::now();
            downsample_rgb565_2x(source, &mut destination).expect("valid scalar benchmark");
            durations.push(started.elapsed());
            std::hint::black_box(&destination);
        }
        durations.sort_unstable();
        let actual_checksum = checksum_rgb565(&destination);
        valid &= actual_checksum == expected_checksum;
        crate::ui_logln!(
            "framebuffer_stream_scalar_bench_tsv\tcase={name}\twidth={width}\theight={height}\tstride_pixels={stride_pixels}\tsamples={samples}\tchecksum={actual_checksum:016x}\texpected_checksum={expected_checksum:016x}\tp50_us={}\tp95_us={}\tmax_us={}",
            percentile_duration(&durations, 50).as_micros(),
            percentile_duration(&durations, 95).as_micros(),
            durations.last().copied().unwrap_or_default().as_micros(),
        );
    }
    valid
}

#[cfg(any(feature = "bench-tools", test))]
fn deterministic_pixels(len: usize) -> Vec<Rgb565Pixel> {
    let mut state = 0x1234_5678u32;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            Rgb565Pixel((state >> 16) as u16)
        })
        .collect()
}

#[cfg(any(feature = "bench-tools", test))]
fn checksum_rgb565(pixels: &[Rgb565Pixel]) -> u64 {
    pixels.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, pixel| {
        let hash = hash ^ u64::from(pixel.0 & 0xff);
        let hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        let hash = hash ^ u64::from(pixel.0 >> 8);
        hash.wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(feature = "bench-tools")]
fn percentile_duration(sorted: &[Duration], percentile: usize) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = percentile
        .saturating_mul(sorted.len())
        .div_ceil(100)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[rank]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(
        pixels: &[Rgb565Pixel],
        width: usize,
        height: usize,
        stride_pixels: usize,
    ) -> Rgb565FrameView<'_> {
        Rgb565FrameView {
            pixels,
            width,
            height,
            stride_pixels,
        }
    }

    #[test]
    fn downsample_keeps_top_left_pixel_of_each_two_by_two_block() {
        let pixels = (0..24).map(Rgb565Pixel).collect::<Vec<_>>();
        let mut output = Vec::new();

        let geometry = downsample_rgb565_2x(view(&pixels, 6, 4, 6), &mut output)
            .expect("downsample even frame");

        assert_eq!(
            geometry,
            DownsampledGeometry {
                width: 3,
                height: 2
            }
        );
        assert_eq!(
            output,
            vec![
                Rgb565Pixel(0),
                Rgb565Pixel(2),
                Rgb565Pixel(4),
                Rgb565Pixel(12),
                Rgb565Pixel(14),
                Rgb565Pixel(16),
            ]
        );
    }

    #[test]
    fn downsample_handles_odd_dimensions_and_padding() {
        let pixels = (0..24).map(Rgb565Pixel).collect::<Vec<_>>();
        let mut output = Vec::new();

        let geometry = downsample_rgb565_2x(view(&pixels, 5, 3, 8), &mut output)
            .expect("downsample padded frame");

        assert_eq!(
            geometry,
            DownsampledGeometry {
                width: 3,
                height: 2
            }
        );
        assert_eq!(
            output,
            vec![
                Rgb565Pixel(0),
                Rgb565Pixel(2),
                Rgb565Pixel(4),
                Rgb565Pixel(16),
                Rgb565Pixel(18),
                Rgb565Pixel(20),
            ]
        );
    }

    #[test]
    fn downsample_checksums_match_for_packed_padded_and_odd_geometry() {
        for (width, height, stride_pixels, expected_checksum) in [
            (960, 540, 960, 0xf812_3960_21e9_0488),
            (960, 540, 976, 0x5cdc_6b4d_35d5_c5f5),
            (959, 539, 967, 0x877c_ca3e_815d_92cb),
        ] {
            let pixels = deterministic_pixels(stride_pixels * height);
            let mut output = Vec::new();

            downsample_rgb565_2x(view(&pixels, width, height, stride_pixels), &mut output)
                .expect("valid deterministic geometry");

            assert_eq!(checksum_rgb565(&output), expected_checksum);
        }
    }

    #[test]
    fn downsample_rejects_invalid_geometry() {
        let pixels = vec![Rgb565Pixel(0); 8];
        let mut output = vec![Rgb565Pixel(9)];

        assert_eq!(
            downsample_rgb565_2x(view(&pixels, 4, 3, 4), &mut output),
            Err("RGB565 source buffer is shorter than its geometry")
        );
    }
}
