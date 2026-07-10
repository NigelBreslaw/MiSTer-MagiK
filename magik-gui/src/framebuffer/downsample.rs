use slint::platform::software_renderer::Rgb565Pixel;
use std::sync::OnceLock;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownsampleImplementation {
    Scalar,
    Neon,
}

impl DownsampleImplementation {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            Self::Neon => "neon",
        }
    }
}

pub fn configured_implementation() -> DownsampleImplementation {
    static IMPLEMENTATION: OnceLock<DownsampleImplementation> = OnceLock::new();
    *IMPLEMENTATION.get_or_init(|| {
        if matches!(
            std::env::var("MISTER_FRAMEBUFFER_STREAM_SIMD").as_deref(),
            Ok("scalar") | Ok("SCALAR")
        ) {
            return DownsampleImplementation::Scalar;
        }
        #[cfg(all(target_arch = "arm", target_feature = "neon"))]
        {
            DownsampleImplementation::Neon
        }
        #[cfg(not(all(target_arch = "arm", target_feature = "neon")))]
        {
            DownsampleImplementation::Scalar
        }
    })
}

pub fn downsample_rgb565_2x(
    source: Rgb565FrameView<'_>,
    destination: &mut Vec<Rgb565Pixel>,
) -> Result<DownsampledGeometry, &'static str> {
    downsample_rgb565_2x_with(source, destination, configured_implementation())
}

fn downsample_rgb565_2x_with(
    source: Rgb565FrameView<'_>,
    destination: &mut Vec<Rgb565Pixel>,
    implementation: DownsampleImplementation,
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

    #[cfg(all(target_arch = "arm", target_feature = "neon"))]
    if implementation == DownsampleImplementation::Neon {
        // SAFETY: validate_source proves every selected source row contains
        // source.width pixels, destination was resized for every output row,
        // and the helper checks its 16-pixel vector tail before each load.
        unsafe { downsample_rgb565_2x_neon(source, destination, geometry) };
        return Ok(geometry);
    }

    let _ = implementation;
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

#[cfg(all(target_arch = "arm", target_feature = "neon"))]
unsafe fn downsample_rgb565_2x_neon(
    source: Rgb565FrameView<'_>,
    destination: &mut [Rgb565Pixel],
    geometry: DownsampledGeometry,
) {
    use core::arch::arm::{vld2q_u16, vst1q_u16};

    for output_y in 0..geometry.height {
        let source_y = output_y * 2;
        let source_row = source
            .pixels
            .as_ptr()
            .add(source_y * source.stride_pixels)
            .cast::<u16>();
        let destination_row = destination
            .as_mut_ptr()
            .add(output_y * geometry.width)
            .cast::<u16>();
        let mut output_x = 0usize;
        while output_x + 8 <= geometry.width && output_x * 2 + 16 <= source.width {
            let separated = vld2q_u16(source_row.add(output_x * 2));
            vst1q_u16(destination_row.add(output_x), separated.0);
            output_x += 8;
        }
        while output_x < geometry.width {
            *destination_row.add(output_x) = *source_row.add(output_x * 2);
            output_x += 1;
        }
    }
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
    fn scalar_downsample_keeps_top_left_pixel_of_each_two_by_two_block() {
        let pixels = (0..24).map(Rgb565Pixel).collect::<Vec<_>>();
        let mut output = Vec::new();

        let geometry = downsample_rgb565_2x_with(
            view(&pixels, 6, 4, 6),
            &mut output,
            DownsampleImplementation::Scalar,
        )
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
    fn scalar_downsample_handles_odd_dimensions_and_padding() {
        let pixels = (0..24).map(Rgb565Pixel).collect::<Vec<_>>();
        let mut output = Vec::new();

        let geometry = downsample_rgb565_2x_with(
            view(&pixels, 5, 3, 8),
            &mut output,
            DownsampleImplementation::Scalar,
        )
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
    fn downsample_rejects_invalid_geometry() {
        let pixels = vec![Rgb565Pixel(0); 8];
        let mut output = vec![Rgb565Pixel(9)];

        assert_eq!(
            downsample_rgb565_2x_with(
                view(&pixels, 4, 3, 4),
                &mut output,
                DownsampleImplementation::Scalar,
            ),
            Err("RGB565 source buffer is shorter than its geometry")
        );
    }

    #[cfg(all(target_arch = "arm", target_feature = "neon"))]
    #[test]
    fn neon_downsample_matches_scalar_for_pseudo_random_frames() {
        let width = 961usize;
        let height = 541usize;
        let stride = 968usize;
        let mut state = 0x1234_5678u32;
        let pixels = (0..stride * height)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                Rgb565Pixel((state >> 16) as u16)
            })
            .collect::<Vec<_>>();
        let mut scalar = Vec::new();
        let mut neon = Vec::new();

        let scalar_geometry = downsample_rgb565_2x_with(
            view(&pixels, width, height, stride),
            &mut scalar,
            DownsampleImplementation::Scalar,
        )
        .expect("scalar downsample");
        let neon_geometry = downsample_rgb565_2x_with(
            view(&pixels, width, height, stride),
            &mut neon,
            DownsampleImplementation::Neon,
        )
        .expect("NEON downsample");

        assert_eq!(neon_geometry, scalar_geometry);
        assert_eq!(neon, scalar);
    }
}
