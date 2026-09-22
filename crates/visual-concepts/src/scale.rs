// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::Pixel;

pub fn expand(low: &[Pixel], lw: usize, lh: usize, p: &mut [Pixel], w: usize, h: usize) {
    let sx = w / lw;
    let sy = h / lh;
    if sx * lw == w && sy * lh == h {
        // Exact integer enlargement avoids two software divisions per texel on
        // Cortex-A9. Odd reduced geometries retain the general fitting path.
        for source_y in 0..lh {
            let y0 = source_y * sy;
            let source = &low[source_y * lw..(source_y + 1) * lw];
            let row = &mut p[y0 * w..(y0 + 1) * w];
            // Constant span lengths let ARM emit stores instead of calling a
            // variable-length fill for every working pixel.
            match sx {
                2 => expand_row::<2>(source, row),
                4 => expand_row::<4>(source, row),
                8 => expand_row::<8>(source, row),
                _ => {
                    for (span, &pixel) in row.chunks_exact_mut(sx).zip(source) {
                        span.fill(pixel);
                    }
                }
            }
            for y in y0 + 1..y0 + sy {
                p.copy_within(y0 * w..(y0 + 1) * w, y * w);
            }
        }
        return;
    }
    for sy in 0..lh {
        let y0 = sy * h / lh;
        let y1 = (sy + 1) * h / lh;
        let row = &mut p[y0 * w..(y0 + 1) * w];
        for sx in 0..lw {
            row[sx * w / lw..(sx + 1) * w / lw].fill(low[sy * lw + sx]);
        }
        for y in y0 + 1..y1 {
            p.copy_within(y0 * w..(y0 + 1) * w, y * w);
        }
    }
}
fn expand_row<const SCALE: usize>(source: &[Pixel], row: &mut [Pixel]) {
    for (span, &pixel) in row.as_chunks_mut::<SCALE>().0.iter_mut().zip(source) {
        span.copy_from_slice(&[pixel; SCALE]);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integer_enlargement_preserves_all_four_source_corners() {
        let mut p = vec![Pixel(99); 4 * 6];
        expand(
            &[Pixel(1), Pixel(2), Pixel(3), Pixel(4)],
            2,
            2,
            &mut p,
            4,
            6,
        );
        assert_eq!(
            p,
            vec![
                Pixel(1),
                Pixel(1),
                Pixel(2),
                Pixel(2),
                Pixel(1),
                Pixel(1),
                Pixel(2),
                Pixel(2),
                Pixel(1),
                Pixel(1),
                Pixel(2),
                Pixel(2),
                Pixel(3),
                Pixel(3),
                Pixel(4),
                Pixel(4),
                Pixel(3),
                Pixel(3),
                Pixel(4),
                Pixel(4),
                Pixel(3),
                Pixel(3),
                Pixel(4),
                Pixel(4)
            ]
        );
    }
    #[test]
    fn scale_covers_uneven_rows() {
        let mut p = vec![Pixel(99); 7 * 5];
        expand(
            &[Pixel(1), Pixel(2), Pixel(3), Pixel(4)],
            2,
            2,
            &mut p,
            7,
            5,
        );
        assert!(!p.contains(&Pixel(99)));
        assert_eq!(p[34], Pixel(4));
    }
}
