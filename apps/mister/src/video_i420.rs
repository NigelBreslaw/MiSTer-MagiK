// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

pub fn convert_i420_to_rgb565(
    src_y: &[u8],
    src_stride_y: usize,
    src_u: &[u8],
    src_stride_u: usize,
    src_v: &[u8],
    src_stride_v: usize,
    dst: &mut [u16],
    dst_stride: usize,
    width: usize,
    height: usize,
) -> Result<(), String> {
    validate_i420_to_rgb565_buffers(
        src_y,
        src_stride_y,
        src_u,
        src_stride_u,
        src_v,
        src_stride_v,
        dst,
        dst_stride,
        width,
        height,
    )?;
    convert_i420_to_rgb565_scalar(
        src_y,
        src_stride_y,
        src_u,
        src_stride_u,
        src_v,
        src_stride_v,
        dst,
        dst_stride,
        width,
        height,
    );
    Ok(())
}

pub fn convert_i420_to_rgb565_2x_rust_optimized(
    src_y: &[u8],
    src_stride_y: usize,
    src_u: &[u8],
    src_stride_u: usize,
    src_v: &[u8],
    src_stride_v: usize,
    dst: &mut [u16],
    dst_stride: usize,
    width: usize,
    height: usize,
) -> Result<(), String> {
    let geometry = validate_i420_to_rgb565_2x_buffers(
        src_y,
        src_stride_y,
        src_u,
        src_stride_u,
        src_v,
        src_stride_v,
        dst,
        dst_stride,
        width,
        height,
    )?;
    // SAFETY: the validation above proves every source read and destination write made by the
    // pointer kernel is within its corresponding slice. Source and destination slices cannot
    // alias because Rust's borrow rules require exclusive access to `dst`.
    unsafe {
        convert_i420_to_rgb565_2x_rust_optimized_unchecked(
            src_y.as_ptr(),
            src_stride_y,
            src_u.as_ptr(),
            src_stride_u,
            src_v.as_ptr(),
            src_stride_v,
            dst.as_mut_ptr(),
            dst_stride,
            width,
            height,
            geometry.output_width,
        );
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct I420DoubleGeometry {
    output_width: usize,
}

fn validate_i420_to_rgb565_2x_buffers(
    src_y: &[u8],
    src_stride_y: usize,
    src_u: &[u8],
    src_stride_u: usize,
    src_v: &[u8],
    src_stride_v: usize,
    dst: &[u16],
    dst_stride: usize,
    width: usize,
    height: usize,
) -> Result<I420DoubleGeometry, String> {
    if width == 0 || height == 0 {
        return Err("video frame has zero dimensions".into());
    }
    let output_width = width.checked_mul(2).ok_or("fused width overflow")?;
    let output_height = height.checked_mul(2).ok_or("fused height overflow")?;
    let chroma_w = width.div_ceil(2);
    let chroma_h = height.div_ceil(2);
    if src_stride_y < width
        || src_stride_u < chroma_w
        || src_stride_v < chroma_w
        || dst_stride < output_width
    {
        return Err("invalid fused I420 2x stride".into());
    }
    let plane_len = |stride: usize, rows: usize, cols: usize| {
        stride
            .checked_mul(rows - 1)
            .and_then(|n| n.checked_add(cols))
    };
    let y_len = plane_len(src_stride_y, height, width).ok_or("fused Y size overflow")?;
    let u_len = plane_len(src_stride_u, chroma_h, chroma_w).ok_or("fused U size overflow")?;
    let v_len = plane_len(src_stride_v, chroma_h, chroma_w).ok_or("fused V size overflow")?;
    let dst_len =
        plane_len(dst_stride, output_height, output_width).ok_or("fused output size overflow")?;
    if src_y.len() < y_len || src_u.len() < u_len || src_v.len() < v_len || dst.len() < dst_len {
        return Err("fused I420 2x buffer is shorter than its geometry".into());
    }
    Ok(I420DoubleGeometry { output_width })
}

#[inline(always)]
fn i420_luma_with_chroma_terms(y: u8, r_add: i32, g_add: i32, b_add: i32) -> u16 {
    let yc = i420_luma_terms()[usize::from(y)];
    let r = clamp_u8_i32((yc + r_add) >> 8);
    let g = clamp_u8_i32((yc + g_add) >> 8);
    let b = clamp_u8_i32((yc + b_add) >> 8);
    ((u16::from(r & 0xf8)) << 8) | ((u16::from(g & 0xfc)) << 3) | (u16::from(b) >> 3)
}

#[inline]
fn i420_luma_terms() -> &'static [i32; 256] {
    &I420_LUMA_TERMS
}

const fn i420_luma_terms_const() -> [i32; 256] {
    let mut terms = [0; 256];
    let mut y = 0;
    while y < terms.len() {
        let level = y as i32 - 16;
        terms[y] = 298 * if level > 0 { level } else { 0 };
        y += 1;
    }
    terms
}

static I420_LUMA_TERMS: [i32; 256] = i420_luma_terms_const();

#[inline(always)]
unsafe fn write_duplicated_rgb565(dst: *mut u16, pixel: u16) {
    let pair = u32::from(pixel) | (u32::from(pixel) << 16);
    // SAFETY: the caller validated room for two u16 values. `write_unaligned` supports padded
    // destination strides whose rows are not naturally aligned to four bytes.
    unsafe { (dst.cast::<u32>()).write_unaligned(pair) };
}

#[allow(clippy::too_many_arguments)]
unsafe fn convert_i420_to_rgb565_2x_rust_optimized_unchecked(
    src_y: *const u8,
    src_stride_y: usize,
    src_u: *const u8,
    src_stride_u: usize,
    src_v: *const u8,
    src_stride_v: usize,
    dst: *mut u16,
    dst_stride: usize,
    width: usize,
    height: usize,
    output_width: usize,
) {
    let mut source_y = 0;
    while source_y < height {
        // SAFETY: the safe wrapper validated every plane and stride for this geometry.
        let (y0, u, v, out0) = unsafe {
            (
                src_y.add(source_y * src_stride_y),
                src_u.add((source_y / 2) * src_stride_u),
                src_v.add((source_y / 2) * src_stride_v),
                dst.add(source_y * 2 * dst_stride),
            )
        };
        let has_second_source_row = source_y + 1 < height;
        let y1 = has_second_source_row.then(|| unsafe { y0.add(src_stride_y) });
        let out2 = has_second_source_row.then(|| unsafe { out0.add(dst_stride * 2) });

        let mut x = 0;
        while x + 1 < width {
            // SAFETY: x/2 is within the validated chroma width and x/x+1 are within luma rows.
            let (cu, cv, y00, y01) =
                unsafe { (*u.add(x / 2), *v.add(x / 2), *y0.add(x), *y0.add(x + 1)) };
            let d = i32::from(cu) - 128;
            let e = i32::from(cv) - 128;
            let r_add = 409 * e + 128;
            let g_add = -100 * d - 208 * e + 128;
            let b_add = 516 * d + 128;
            let p00 = i420_luma_with_chroma_terms(y00, r_add, g_add, b_add);
            let p01 = i420_luma_with_chroma_terms(y01, r_add, g_add, b_add);
            unsafe {
                write_duplicated_rgb565(out0.add(x * 2), p00);
                write_duplicated_rgb565(out0.add(x * 2 + 2), p01);
            }
            if let (Some(y1), Some(out2)) = (y1, out2) {
                let (y10, y11) = unsafe { (*y1.add(x), *y1.add(x + 1)) };
                let p10 = i420_luma_with_chroma_terms(y10, r_add, g_add, b_add);
                let p11 = i420_luma_with_chroma_terms(y11, r_add, g_add, b_add);
                unsafe {
                    write_duplicated_rgb565(out2.add(x * 2), p10);
                    write_duplicated_rgb565(out2.add(x * 2 + 2), p11);
                }
            }
            x += 2;
        }
        if x < width {
            let (cu, cv, y00) = unsafe { (*u.add(x / 2), *v.add(x / 2), *y0.add(x)) };
            let d = i32::from(cu) - 128;
            let e = i32::from(cv) - 128;
            let r_add = 409 * e + 128;
            let g_add = -100 * d - 208 * e + 128;
            let b_add = 516 * d + 128;
            let p00 = i420_luma_with_chroma_terms(y00, r_add, g_add, b_add);
            unsafe { write_duplicated_rgb565(out0.add(x * 2), p00) };
            if let (Some(y1), Some(out2)) = (y1, out2) {
                let p10 = i420_luma_with_chroma_terms(unsafe { *y1.add(x) }, r_add, g_add, b_add);
                unsafe { write_duplicated_rgb565(out2.add(x * 2), p10) };
            }
        }

        unsafe { std::ptr::copy_nonoverlapping(out0, out0.add(dst_stride), output_width) };
        if let Some(out2) = out2 {
            unsafe { std::ptr::copy_nonoverlapping(out2, out2.add(dst_stride), output_width) };
        }
        source_y += 2;
    }
}

fn validate_i420_to_rgb565_buffers(
    src_y: &[u8],
    src_stride_y: usize,
    src_u: &[u8],
    src_stride_u: usize,
    src_v: &[u8],
    src_stride_v: usize,
    dst: &[u16],
    dst_stride: usize,
    width: usize,
    height: usize,
) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("video frame has zero dimensions".into());
    }
    let chroma_w = width.div_ceil(2);
    let chroma_h = height.div_ceil(2);
    if src_stride_y < width || src_stride_u < chroma_w || src_stride_v < chroma_w {
        return Err(format!(
            "I420 strides too small for {width}x{height}: y={src_stride_y} u={src_stride_u} v={src_stride_v}"
        ));
    }
    if dst_stride < width {
        return Err(format!(
            "RGB565 stride {dst_stride} too small for width {width}"
        ));
    }
    let y_len = src_stride_y
        .checked_mul(height.saturating_sub(1))
        .and_then(|n| n.checked_add(width))
        .ok_or_else(|| "Y plane size overflow".to_string())?;
    let u_len = src_stride_u
        .checked_mul(chroma_h.saturating_sub(1))
        .and_then(|n| n.checked_add(chroma_w))
        .ok_or_else(|| "U plane size overflow".to_string())?;
    let v_len = src_stride_v
        .checked_mul(chroma_h.saturating_sub(1))
        .and_then(|n| n.checked_add(chroma_w))
        .ok_or_else(|| "V plane size overflow".to_string())?;
    let dst_len = dst_stride
        .checked_mul(height.saturating_sub(1))
        .and_then(|n| n.checked_add(width))
        .ok_or_else(|| "RGB565 plane size overflow".to_string())?;
    if src_y.len() < y_len || src_u.len() < u_len || src_v.len() < v_len || dst.len() < dst_len {
        return Err(format!(
            "I420 buffer too small for {width}x{height}: y={}/{} u={}/{} v={}/{} dst={}/{}",
            src_y.len(),
            y_len,
            src_u.len(),
            u_len,
            src_v.len(),
            v_len,
            dst.len(),
            dst_len
        ));
    }
    Ok(())
}

fn convert_i420_to_rgb565_scalar(
    src_y: &[u8],
    src_stride_y: usize,
    src_u: &[u8],
    src_stride_u: usize,
    src_v: &[u8],
    src_stride_v: usize,
    dst: &mut [u16],
    dst_stride: usize,
    width: usize,
    height: usize,
) {
    for row in 0..height {
        let y_row = &src_y[row * src_stride_y..];
        let u_row = &src_u[(row / 2) * src_stride_u..];
        let v_row = &src_v[(row / 2) * src_stride_v..];
        let dst_row = &mut dst[row * dst_stride..];
        for x in 0..width {
            dst_row[x] = i420_pixel_to_rgb565(y_row[x], u_row[x / 2], v_row[x / 2]);
        }
    }
}

fn i420_pixel_to_rgb565(y: u8, u: u8, v: u8) -> u16 {
    let c = (i32::from(y) - 16).max(0);
    let d = i32::from(u) - 128;
    let e = i32::from(v) - 128;
    let r = clamp_u8_i32((298 * c + 409 * e + 128) >> 8);
    let g = clamp_u8_i32((298 * c - 100 * d - 208 * e + 128) >> 8);
    let b = clamp_u8_i32((298 * c + 516 * d + 128) >> 8);
    ((u16::from(r & 0xf8)) << 8) | ((u16::from(g & 0xfc)) << 3) | (u16::from(b) >> 3)
}

fn clamp_u8_i32(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    struct I420Fixture {
        src_y: Vec<u8>,
        src_stride_y: usize,
        src_u: Vec<u8>,
        src_stride_u: usize,
        src_v: Vec<u8>,
        src_stride_v: usize,
        dst_stride: usize,
        width: usize,
        height: usize,
    }

    fn fixture(width: usize, height: usize, pad: usize) -> I420Fixture {
        let chroma_w = width.div_ceil(2);
        let chroma_h = height.div_ceil(2);
        let src_stride_y = width + pad;
        let src_stride_u = chroma_w + pad;
        let src_stride_v = chroma_w + pad;
        let dst_stride = width + pad;
        let mut src_y = vec![0; src_stride_y * height];
        let mut src_u = vec![0; src_stride_u * chroma_h];
        let mut src_v = vec![0; src_stride_v * chroma_h];
        for (idx, value) in src_y.iter_mut().enumerate() {
            *value = 16 + ((idx * 17 + width * 3 + height) % 220) as u8;
        }
        for (idx, value) in src_u.iter_mut().enumerate() {
            *value = 32 + ((idx * 29 + width) % 190) as u8;
        }
        for (idx, value) in src_v.iter_mut().enumerate() {
            *value = 48 + ((idx * 31 + height) % 170) as u8;
        }
        I420Fixture {
            src_y,
            src_stride_y,
            src_u,
            src_stride_u,
            src_v,
            src_stride_v,
            dst_stride,
            width,
            height,
        }
    }

    fn expected_exact(fixture: &I420Fixture) -> Vec<u16> {
        let mut expected = vec![0xdead; fixture.dst_stride * fixture.height];
        for row in 0..fixture.height {
            for x in 0..fixture.width {
                let y = fixture.src_y[row * fixture.src_stride_y + x];
                let u = fixture.src_u[(row / 2) * fixture.src_stride_u + x / 2];
                let v = fixture.src_v[(row / 2) * fixture.src_stride_v + x / 2];
                expected[row * fixture.dst_stride + x] = i420_pixel_to_rgb565(y, u, v);
            }
        }
        expected
    }

    fn expected_2x(
        src: &[u16],
        src_stride: usize,
        dst_stride: usize,
        width: usize,
        height: usize,
    ) -> Vec<u16> {
        let mut dst = vec![0xdead; dst_stride * height * 2];
        for y in 0..height {
            for x in 0..width {
                let pixel = src[y * src_stride + x];
                let dx = x * 2;
                let dy = y * 2;
                dst[dy * dst_stride + dx] = pixel;
                dst[dy * dst_stride + dx + 1] = pixel;
                dst[(dy + 1) * dst_stride + dx] = pixel;
                dst[(dy + 1) * dst_stride + dx + 1] = pixel;
            }
        }
        dst
    }

    #[test]
    fn i420_converter_rejects_zero_dimensions() {
        let mut dst = vec![0; 1];

        let err = convert_i420_to_rgb565(&[0], 1, &[0], 1, &[0], 1, &mut dst, 1, 0, 1)
            .expect_err("zero width must fail");

        assert!(err.contains("zero dimensions"), "unexpected error: {err}");
    }

    #[test]
    fn i420_converter_rejects_short_planes() {
        let mut dst = vec![0; 4];

        let err = convert_i420_to_rgb565(&[16, 17], 2, &[128], 1, &[128], 1, &mut dst, 2, 2, 2)
            .expect_err("short Y plane must fail");

        assert!(err.contains("buffer too small"), "unexpected error: {err}");
    }

    #[test]
    fn i420_converter_rejects_short_destination_stride() {
        let fixture = fixture(3, 2, 2);
        let mut dst = vec![0; fixture.width * fixture.height];

        let err = convert_i420_to_rgb565(
            &fixture.src_y,
            fixture.src_stride_y,
            &fixture.src_u,
            fixture.src_stride_u,
            &fixture.src_v,
            fixture.src_stride_v,
            &mut dst,
            fixture.width - 1,
            fixture.width,
            fixture.height,
        )
        .expect_err("short destination stride must fail");

        assert!(err.contains("stride"), "unexpected error: {err}");
    }

    #[test]
    fn i420_scalar_conversion_matches_exact_reference_for_padded_odd_even_frames() {
        for (width, height) in [(1, 1), (2, 1), (3, 2), (8, 3), (9, 4), (16, 2)] {
            let fixture = fixture(width, height, 3);
            let mut dst = vec![0xdead; fixture.dst_stride * fixture.height];

            validate_i420_to_rgb565_buffers(
                &fixture.src_y,
                fixture.src_stride_y,
                &fixture.src_u,
                fixture.src_stride_u,
                &fixture.src_v,
                fixture.src_stride_v,
                &dst,
                fixture.dst_stride,
                fixture.width,
                fixture.height,
            )
            .expect("validate I420 fixture");
            convert_i420_to_rgb565_scalar(
                &fixture.src_y,
                fixture.src_stride_y,
                &fixture.src_u,
                fixture.src_stride_u,
                &fixture.src_v,
                fixture.src_stride_v,
                &mut dst,
                fixture.dst_stride,
                fixture.width,
                fixture.height,
            );

            assert_eq!(dst, expected_exact(&fixture), "{width}x{height}");
        }
    }

    #[test]
    fn rust_optimized_fused_matches_separated_for_padded_odd_even_frames() {
        for (width, height, pad) in [
            (1, 1, 3),
            (2, 1, 2),
            (7, 3, 5),
            (8, 2, 4),
            (15, 4, 3),
            (16, 5, 1),
        ] {
            let fixture = fixture(width, height, pad);
            let mut converted = vec![0xdead; fixture.dst_stride * fixture.height];
            convert_i420_to_rgb565(
                &fixture.src_y,
                fixture.src_stride_y,
                &fixture.src_u,
                fixture.src_stride_u,
                &fixture.src_v,
                fixture.src_stride_v,
                &mut converted,
                fixture.dst_stride,
                width,
                height,
            )
            .unwrap();

            let output_stride = width * 2 + pad;
            let expected =
                expected_2x(&converted, fixture.dst_stride, output_stride, width, height);
            let mut fused = vec![0xdead; output_stride * height * 2];
            convert_i420_to_rgb565_2x_rust_optimized(
                &fixture.src_y,
                fixture.src_stride_y,
                &fixture.src_u,
                fixture.src_stride_u,
                &fixture.src_v,
                fixture.src_stride_v,
                &mut fused,
                output_stride,
                width,
                height,
            )
            .unwrap();

            assert_eq!(fused, expected, "fused {width}x{height} pad={pad}");
        }
    }

    #[test]
    fn rust_optimized_fused_rejects_invalid_geometry() {
        let fixture = fixture(3, 3, 2);
        let mut dst = vec![0; 6 * 6];
        let err = convert_i420_to_rgb565_2x_rust_optimized(
            &fixture.src_y,
            fixture.src_stride_y,
            &fixture.src_u,
            fixture.src_stride_u,
            &fixture.src_v,
            fixture.src_stride_v,
            &mut dst,
            5,
            fixture.width,
            fixture.height,
        )
        .expect_err("short doubled destination stride must fail");
        assert!(err.contains("stride"), "unexpected error: {err}");

        let err = convert_i420_to_rgb565_2x_rust_optimized(
            &fixture.src_y,
            fixture.src_stride_y,
            &fixture.src_u,
            fixture.src_stride_u,
            &fixture.src_v,
            fixture.src_stride_v,
            &mut dst,
            6,
            0,
            fixture.height,
        )
        .expect_err("zero width must fail");
        assert!(err.contains("zero dimensions"), "unexpected error: {err}");
    }
}
