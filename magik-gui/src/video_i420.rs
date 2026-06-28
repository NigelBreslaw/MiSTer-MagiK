pub(crate) fn convert_i420_to_rgb565(
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
    #[cfg(all(target_arch = "arm", target_feature = "neon"))]
    {
        // SAFETY: buffer extents and strides were validated above; the NEON
        // implementation only reads within the I420 planes and writes within dst.
        unsafe {
            convert_i420_to_rgb565_neon(
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
            )
        }
        Ok(())
    }
    #[cfg(not(all(target_arch = "arm", target_feature = "neon")))]
    {
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

#[cfg(all(target_arch = "arm", target_feature = "neon"))]
unsafe fn convert_i420_to_rgb565_neon(
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
    use core::arch::arm::{
        vaddq_s16, vdupq_n_s16, vld1_u8, vmaxq_s16, vmovl_u8, vorrq_u16, vqmovun_s16,
        vreinterpretq_s16_u16, vshlq_n_u16, vshr_n_u8, vshrq_n_s16, vst1q_u16, vsubq_s16,
    };

    debug_assert!(width > 0);
    debug_assert!(height > 0);
    debug_assert!(src_stride_y >= width);
    debug_assert!(src_stride_u >= width.div_ceil(2));
    debug_assert!(src_stride_v >= width.div_ceil(2));
    debug_assert!(dst_stride >= width);
    debug_assert!(
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
        )
        .is_ok(),
        "NEON I420 converter requires validated plane and destination extents"
    );

    for row in 0..height {
        let y_row = &src_y[row * src_stride_y..];
        let u_row = &src_u[(row / 2) * src_stride_u..];
        let v_row = &src_v[(row / 2) * src_stride_v..];
        let dst_row = &mut dst[row * dst_stride..];
        let mut x = 0usize;

        while x + 8 <= width {
            let y_u8 = vld1_u8(y_row.as_ptr().add(x));
            let chroma = x >> 1;
            let u_dup_bytes = [
                u_row[chroma],
                u_row[chroma],
                u_row[chroma + 1],
                u_row[chroma + 1],
                u_row[chroma + 2],
                u_row[chroma + 2],
                u_row[chroma + 3],
                u_row[chroma + 3],
            ];
            let v_dup_bytes = [
                v_row[chroma],
                v_row[chroma],
                v_row[chroma + 1],
                v_row[chroma + 1],
                v_row[chroma + 2],
                v_row[chroma + 2],
                v_row[chroma + 3],
                v_row[chroma + 3],
            ];
            let u_dup = vld1_u8(u_dup_bytes.as_ptr());
            let v_dup = vld1_u8(v_dup_bytes.as_ptr());

            let mut y = vreinterpretq_s16_u16(vmovl_u8(y_u8));
            let mut u = vreinterpretq_s16_u16(vmovl_u8(u_dup));
            let mut v = vreinterpretq_s16_u16(vmovl_u8(v_dup));
            y = vmaxq_s16(vsubq_s16(y, vdupq_n_s16(16)), vdupq_n_s16(0));
            u = vsubq_s16(u, vdupq_n_s16(128));
            v = vsubq_s16(v, vdupq_n_s16(128));

            let y_base = vaddq_s16(y, vaddq_s16(vshrq_n_s16(y, 3), vshrq_n_s16(y, 5)));
            let r16 = vaddq_s16(
                y_base,
                vaddq_s16(
                    v,
                    vaddq_s16(
                        vshrq_n_s16(v, 2),
                        vaddq_s16(vshrq_n_s16(v, 3), vshrq_n_s16(v, 5)),
                    ),
                ),
            );
            let g16 = vsubq_s16(
                vsubq_s16(
                    y_base,
                    vaddq_s16(
                        vshrq_n_s16(u, 2),
                        vaddq_s16(vshrq_n_s16(u, 4), vshrq_n_s16(u, 5)),
                    ),
                ),
                vaddq_s16(
                    vshrq_n_s16(v, 1),
                    vaddq_s16(vshrq_n_s16(v, 2), vshrq_n_s16(v, 5)),
                ),
            );
            let b16 = vaddq_s16(
                y_base,
                vaddq_s16(
                    u,
                    vaddq_s16(
                        vshrq_n_s16(u, 1),
                        vaddq_s16(vshrq_n_s16(u, 2), vshrq_n_s16(u, 4)),
                    ),
                ),
            );

            let r = vqmovun_s16(r16);
            let g = vqmovun_s16(g16);
            let b = vqmovun_s16(b16);
            let r565 = vshlq_n_u16(vmovl_u8(vshr_n_u8(r, 3)), 11);
            let g565 = vshlq_n_u16(vmovl_u8(vshr_n_u8(g, 2)), 5);
            let b565 = vmovl_u8(vshr_n_u8(b, 3));
            let rgb565 = vorrq_u16(vorrq_u16(r565, g565), b565);
            vst1q_u16(dst_row.as_mut_ptr().add(x), rgb565);
            x += 8;
        }

        while x < width {
            dst_row[x] = i420_pixel_to_rgb565(y_row[x], u_row[x / 2], v_row[x / 2]);
            x += 1;
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

    #[cfg(all(target_arch = "arm", target_feature = "neon"))]
    #[test]
    fn i420_neon_conversion_matches_fast_reference_for_padded_odd_even_frames() {
        for (width, height) in [(8, 1), (9, 2), (15, 3), (16, 4)] {
            let fixture = fixture(width, height, 5);
            let mut dst = vec![0xdead; fixture.dst_stride * fixture.height];

            // SAFETY: the fixture constructor allocates each plane and the
            // destination to the declared validated strides and dimensions.
            unsafe {
                convert_i420_to_rgb565_neon(
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
            }

            assert_eq!(
                dst,
                expected_fast_neon_port_reference(&fixture),
                "{width}x{height}"
            );
        }
    }

    #[cfg(all(target_arch = "arm", target_feature = "neon"))]
    #[test]
    fn i420_safe_wrapper_matches_fast_reference_for_neon_and_tail_cases() {
        for (width, height) in [(1, 1), (7, 3), (8, 1), (9, 2), (16, 4)] {
            let fixture = fixture(width, height, 5);
            let mut dst = vec![0xdead; fixture.dst_stride * fixture.height];

            convert_i420_to_rgb565(
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
            )
            .expect("convert I420 through safe wrapper");

            assert_eq!(
                dst,
                expected_fast_neon_port_reference(&fixture),
                "{width}x{height}"
            );
        }
    }

    #[cfg(all(target_arch = "arm", target_feature = "neon"))]
    fn expected_fast_neon_port_reference(fixture: &I420Fixture) -> Vec<u16> {
        let mut expected = vec![0xdead; fixture.dst_stride * fixture.height];
        for row in 0..fixture.height {
            let mut x = 0usize;
            while x + 8 <= fixture.width {
                for lane in 0..8 {
                    let sx = x + lane;
                    let y = fixture.src_y[row * fixture.src_stride_y + sx];
                    let u = fixture.src_u[(row / 2) * fixture.src_stride_u + sx / 2];
                    let v = fixture.src_v[(row / 2) * fixture.src_stride_v + sx / 2];
                    expected[row * fixture.dst_stride + sx] =
                        i420_pixel_to_rgb565_fast_approx(y, u, v);
                }
                x += 8;
            }
            while x < fixture.width {
                let y = fixture.src_y[row * fixture.src_stride_y + x];
                let u = fixture.src_u[(row / 2) * fixture.src_stride_u + x / 2];
                let v = fixture.src_v[(row / 2) * fixture.src_stride_v + x / 2];
                expected[row * fixture.dst_stride + x] = i420_pixel_to_rgb565(y, u, v);
                x += 1;
            }
        }
        expected
    }

    #[cfg(all(target_arch = "arm", target_feature = "neon"))]
    fn i420_pixel_to_rgb565_fast_approx(y: u8, u: u8, v: u8) -> u16 {
        let y = (i32::from(y) - 16).max(0);
        let u = i32::from(u) - 128;
        let v = i32::from(v) - 128;
        let y_base = y + (y >> 3) + (y >> 5);
        let r = clamp_u8_i32(y_base + v + (v >> 2) + (v >> 3) + (v >> 5));
        let g = clamp_u8_i32(
            y_base - ((u >> 2) + (u >> 4) + (u >> 5)) - ((v >> 1) + (v >> 2) + (v >> 5)),
        );
        let b = clamp_u8_i32(y_base + u + (u >> 1) + (u >> 2) + (u >> 4));
        ((u16::from(r & 0xf8)) << 8) | ((u16::from(g & 0xfc)) << 3) | (u16::from(b) >> 3)
    }
}
