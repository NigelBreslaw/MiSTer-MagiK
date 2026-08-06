// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

/// Copy a source rectangle into a destination buffer, nearest-neighbor scaled.
// Flat rectangle parameters keep framebuffer call sites allocation-free and easy
// to inline in copy-heavy paths.
#[allow(clippy::too_many_arguments)]
pub fn copy_rect_scaled_to<T: Copy>(
    dst: &mut [T],
    dst_w: usize,
    dst_h: usize,
    dst_x: usize,
    dst_y: usize,
    scale: usize,
    src: &[T],
    src_w: usize,
    src_x0: usize,
    src_y0: usize,
    src_x1: usize,
    src_y1: usize,
) {
    if scale == 0 || src_x1 <= src_x0 || src_y1 <= src_y0 || dst_x >= dst_w || dst_y >= dst_h {
        return;
    }

    for sy in src_y0..src_y1 {
        let src_row = &src[sy * src_w..(sy + 1) * src_w];
        let py0 = dst_y + (sy - src_y0) * scale;
        for dy in 0..scale {
            let py = py0 + dy;
            if py >= dst_h {
                break;
            }
            let dst_row = &mut dst[py * dst_w..(py + 1) * dst_w];
            for (sx, &color) in src_row[src_x0..src_x1].iter().enumerate() {
                let px0 = dst_x + sx * scale;
                for dx in 0..scale {
                    let px = px0 + dx;
                    if px < dst_w {
                        dst_row[px] = color;
                    }
                }
            }
        }
    }
}

/// Copy a source rectangle into a destination buffer with a specialized 2x path.
// Flat rectangle parameters keep framebuffer call sites allocation-free and easy
// to inline in copy-heavy paths.
#[allow(clippy::too_many_arguments)]
pub fn copy_rect_2x_to<T: Copy>(
    dst: &mut [T],
    dst_w: usize,
    dst_h: usize,
    dst_x: usize,
    dst_y: usize,
    src: &[T],
    src_w: usize,
    src_x0: usize,
    src_y0: usize,
    src_x1: usize,
    src_y1: usize,
) {
    if src_x1 <= src_x0 || src_y1 <= src_y0 || dst_x >= dst_w || dst_y >= dst_h {
        return;
    }

    for sy in src_y0..src_y1 {
        let py0 = dst_y + (sy - src_y0) * 2;
        if py0 >= dst_h {
            break;
        }
        let src_row = &src[sy * src_w + src_x0..sy * src_w + src_x1];
        let copy_w = (src_row.len() * 2).min(dst_w.saturating_sub(dst_x));
        if copy_w == 0 {
            continue;
        }
        copy_2x_row(
            &mut dst[py0 * dst_w + dst_x..py0 * dst_w + dst_x + copy_w],
            src_row,
        );
        if py0 + 1 < dst_h {
            copy_2x_row(
                &mut dst[(py0 + 1) * dst_w + dst_x..(py0 + 1) * dst_w + dst_x + copy_w],
                src_row,
            );
        }
    }
}

/// Copy a `u32` source rectangle into a destination buffer at 2x scale.
///
/// This is the hot framebuffer path. Keep stores as 32-bit words: the MiSTer
/// framebuffer mapping is write-combined, and wider `u64` stores benchmarked
/// slower on the Cortex-A9.
#[allow(clippy::too_many_arguments)]
pub fn copy_rect_2x_u32_to(
    dst: &mut [u32],
    dst_w: usize,
    dst_h: usize,
    dst_x: usize,
    dst_y: usize,
    src: &[u32],
    src_w: usize,
    src_x0: usize,
    src_y0: usize,
    src_x1: usize,
    src_y1: usize,
) {
    if src_x1 <= src_x0 || src_y1 <= src_y0 || dst_x >= dst_w || dst_y >= dst_h {
        return;
    }

    for sy in src_y0..src_y1 {
        let py0 = dst_y + (sy - src_y0) * 2;
        if py0 >= dst_h {
            break;
        }
        let src_row = &src[sy * src_w + src_x0..sy * src_w + src_x1];
        let copy_w = (src_row.len() * 2).min(dst_w.saturating_sub(dst_x));
        if copy_w == 0 {
            continue;
        }
        copy_2x_u32_row(
            &mut dst[py0 * dst_w + dst_x..py0 * dst_w + dst_x + copy_w],
            src_row,
        );
        if py0 + 1 < dst_h {
            copy_2x_u32_row(
                &mut dst[(py0 + 1) * dst_w + dst_x..(py0 + 1) * dst_w + dst_x + copy_w],
                src_row,
            );
        }
    }
}

fn copy_2x_row<T: Copy>(dst: &mut [T], src: &[T]) {
    for (sx, &color) in src.iter().enumerate() {
        let dx = sx * 2;
        if dx >= dst.len() {
            break;
        }
        dst[dx] = color;
        if dx + 1 < dst.len() {
            dst[dx + 1] = color;
        }
    }
}

fn copy_2x_u32_row(dst: &mut [u32], src: &[u32]) {
    let dst_len = dst.len();
    let src_len = src.len();
    let packed_len = (dst.len() / 2).min(src.len());
    let mut i = 0;
    while i + 4 <= packed_len {
        let c0 = src[i];
        let c1 = src[i + 1];
        let c2 = src[i + 2];
        let c3 = src[i + 3];
        let d = i * 2;
        dst[d] = c0;
        dst[d + 1] = c0;
        dst[d + 2] = c1;
        dst[d + 3] = c1;
        dst[d + 4] = c2;
        dst[d + 5] = c2;
        dst[d + 6] = c3;
        dst[d + 7] = c3;
        i += 4;
    }
    while i < packed_len {
        let color = src[i];
        let d = i * 2;
        dst[d] = color;
        dst[d + 1] = color;
        i += 1;
    }
    if !dst_len.is_multiple_of(2) && packed_len < src_len {
        dst[packed_len * 2] = src[packed_len];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src() -> Vec<u8> {
        vec![
            1, 2, 3, 4, //
            5, 6, 7, 8, //
            9, 10, 11, 12,
        ]
    }

    #[test]
    fn copies_scale_1_rect_at_origin() {
        let mut dst = vec![0; 12];
        copy_rect_scaled_to(&mut dst, 4, 3, 0, 0, 1, &src(), 4, 1, 1, 3, 3);
        assert_eq!(dst, vec![6, 7, 0, 0, 10, 11, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn copies_scale_1_rect_at_offset() {
        let mut dst = vec![0; 20];
        copy_rect_scaled_to(&mut dst, 5, 4, 2, 1, 1, &src(), 4, 0, 0, 2, 2);
        assert_eq!(
            dst,
            vec![
                0, 0, 0, 0, 0, //
                0, 0, 1, 2, 0, //
                0, 0, 5, 6, 0, //
                0, 0, 0, 0, 0,
            ]
        );
    }

    #[test]
    fn copies_scale_2_rect() {
        let mut dst = vec![0; 24];
        copy_rect_scaled_to(&mut dst, 6, 4, 1, 0, 2, &src(), 4, 1, 0, 3, 2);
        assert_eq!(
            dst,
            vec![
                0, 2, 2, 3, 3, 0, //
                0, 2, 2, 3, 3, 0, //
                0, 6, 6, 7, 7, 0, //
                0, 6, 6, 7, 7, 0,
            ]
        );
    }

    #[test]
    fn specialized_2x_matches_generic_2x() {
        let mut generic = vec![0; 24];
        let mut specialized = vec![0; 24];
        copy_rect_scaled_to(&mut generic, 6, 4, 1, 0, 2, &src(), 4, 1, 0, 3, 2);
        copy_rect_2x_to(&mut specialized, 6, 4, 1, 0, &src(), 4, 1, 0, 3, 2);
        assert_eq!(specialized, generic);
    }

    #[test]
    fn specialized_u32_2x_matches_generic_2x() {
        let src = src().into_iter().map(u32::from).collect::<Vec<_>>();
        let mut generic = vec![0; 24];
        let mut specialized = vec![0; 24];
        copy_rect_scaled_to(&mut generic, 6, 4, 1, 0, 2, &src, 4, 1, 0, 3, 2);
        copy_rect_2x_u32_to(&mut specialized, 6, 4, 1, 0, &src, 4, 1, 0, 3, 2);
        assert_eq!(specialized, generic);
    }

    #[test]
    fn specialized_u32_handles_odd_right_clip() {
        let src = vec![1u32, 2, 3];
        let mut dst = vec![0; 5];
        copy_rect_2x_u32_to(&mut dst, 5, 1, 0, 0, &src, 3, 0, 0, 3, 1);
        assert_eq!(dst, vec![1, 1, 2, 2, 3]);
    }

    #[test]
    fn specialized_2x_handles_odd_right_clip() {
        let src = vec![1u8, 2, 3];
        let mut dst = vec![0; 5];
        copy_rect_2x_to(&mut dst, 5, 1, 0, 0, &src, 3, 0, 0, 3, 1);
        assert_eq!(dst, vec![1, 1, 2, 2, 3]);
    }

    #[test]
    fn clips_right_and_bottom_edges() {
        let mut dst = vec![0; 12];
        copy_rect_scaled_to(&mut dst, 4, 3, 3, 2, 2, &src(), 4, 0, 0, 2, 2);
        assert_eq!(dst, vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn ignores_empty_rects_and_zero_scale() {
        let mut dst = vec![9; 4];
        copy_rect_scaled_to(&mut dst, 2, 2, 0, 0, 0, &src(), 4, 0, 0, 2, 2);
        copy_rect_scaled_to(&mut dst, 2, 2, 0, 0, 1, &src(), 4, 1, 1, 1, 2);
        copy_rect_2x_to(&mut dst, 2, 2, 0, 0, &src(), 4, 1, 1, 1, 2);
        assert_eq!(dst, vec![9; 4]);
    }
}
