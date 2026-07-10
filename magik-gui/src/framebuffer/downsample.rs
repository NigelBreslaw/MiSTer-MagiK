use slint::platform::software_renderer::Rgb565Pixel;

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

    #[cfg(mister_arm_scalar_decimator)]
    {
        // SAFETY: validate_source proves every selected source row contains
        // source.width pixels, and destination was resized for every output row.
        unsafe { downsample_rgb565_2x_fixed_scalar(source, destination, geometry) };
    }
    #[cfg(not(mister_arm_scalar_decimator))]
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

#[cfg(not(mister_arm_scalar_decimator))]
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

#[cfg(mister_arm_scalar_decimator)]
unsafe fn downsample_rgb565_2x_fixed_scalar(
    source: Rgb565FrameView<'_>,
    destination: &mut [Rgb565Pixel],
    geometry: DownsampledGeometry,
) {
    unsafe extern "C" {
        fn mister_magik_downsample_rgb565_2x_scalar(
            source: *const u16,
            source_height: usize,
            source_stride: usize,
            destination: *mut u16,
            destination_width: usize,
        );
    }

    mister_magik_downsample_rgb565_2x_scalar(
        source.pixels.as_ptr().cast::<u16>(),
        source.height,
        source.stride_pixels,
        destination.as_mut_ptr().cast::<u16>(),
        geometry.width,
    );
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
    fn downsample_rejects_invalid_geometry() {
        let pixels = vec![Rgb565Pixel(0); 8];
        let mut output = vec![Rgb565Pixel(9)];

        assert_eq!(
            downsample_rgb565_2x(view(&pixels, 4, 3, 4), &mut output),
            Err("RGB565 source buffer is shorter than its geometry")
        );
    }
}
