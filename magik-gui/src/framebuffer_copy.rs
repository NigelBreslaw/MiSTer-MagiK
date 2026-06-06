/// Copy a source rectangle into a destination buffer, nearest-neighbor scaled.
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

fn copy_2x_row<T: Copy>(dst: &mut [T], src: &[T]) {
    for (sx, &color) in src.iter().enumerate() {
        let dx = sx * 2;
        if dx + 1 >= dst.len() {
            break;
        }
        dst[dx] = color;
        dst[dx + 1] = color;
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
