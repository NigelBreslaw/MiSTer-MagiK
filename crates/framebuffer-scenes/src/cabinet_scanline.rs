// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded, separable cabinet and cog sampling. Mip and coordinate setup is per frame;
//! two horizontal rows per level are reused, preserving reference rounding.
use super::{CABINET_WIDTH, CabinetTexture};
use crate::Rgb565Pixel;
#[cfg(any(not(target_arch = "arm"), test))]
use crate::launcher_texture::{mix, over_dithered};
const N: usize = CABINET_WIDTH + 1;
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Column {
    index: i16,
    weight: u16,
}
#[derive(Clone)]
pub(super) struct Scanlines {
    columns: [[Column; N]; 2],
    interior: [(usize, usize); 2],
    rows: [[[u32; N]; 2]; 2],
    tags: [[i64; 2]; 2],
}
impl Scanlines {
    pub(super) fn new() -> Self {
        Self {
            columns: [[Column::default(); N]; 2],
            interior: [(0, 0); 2],
            rows: [[[0; N]; 2]; 2],
            tags: [[i64::MIN; 2]; 2],
        }
    }
}
// Exclude only pixels that this frame's opaque game overlay will replace.
// Keep each remaining region contiguous so horizontal row reuse is retained.
pub(super) fn render(
    texture: &CabinetTexture,
    out: &mut [Rgb565Pixel],
    bounds: (usize, usize, usize, usize),
    transform: (i64, i64, i64),
    covered: Option<(usize, usize, usize, usize)>,
    black: bool,
) {
    let (x0, x1, y0, y1) = bounds;
    if let Some((left, right, top, bottom)) = covered {
        let left = left.max(x0).min(x1);
        let right = right.min(x1).max(x0);
        let top = top.max(y0).min(y1);
        let bottom = bottom.min(y1).max(y0);
        if left < right && top < bottom {
            for bounds in [
                (x0, x1, y0, top),
                (x0, x1, bottom, y1),
                (x0, left, top, bottom),
                (right, x1, top, bottom),
            ] {
                render_region(
                    texture,
                    out,
                    bounds,
                    (transform.0, transform.1, transform.2, transform.2),
                    960,
                    black,
                    256,
                    |_| (bounds.0, bounds.1),
                );
            }
            return;
        }
    }
    render_region(
        texture,
        out,
        bounds,
        (transform.0, transform.1, transform.2, transform.2),
        960,
        black,
        256,
        |_| (bounds.0, bounds.1),
    );
}
#[allow(clippy::too_many_arguments)]
pub(super) fn render_region(
    texture: &CabinetTexture,
    out: &mut [Rgb565Pixel],
    bounds: (usize, usize, usize, usize),
    transform: (i64, i64, i64, i64),
    width: usize,
    black: bool,
    alpha: u32,
    clip: impl Fn(usize) -> (usize, usize),
) {
    let (x0, x1, y0, y1) = bounds;
    let (cx, cy, inverse_x, inverse_y) = transform;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let n = x1 - x0;
    assert!(n <= N && x1 <= width && y1 * width <= out.len() && alpha <= 256);
    // At integer 1:1 geometry the reference bilinear weights and mip blend
    // are zero. Quantise the original row directly, preserving alpha and phase.
    if inverse_x == 65536 && inverse_y == 65536 && (cx | cy) & 65535 == 0 {
        let source = &texture.levels[0];
        let sx = x0 as i64 - (cx >> 16);
        let sy = y0 as i64 - (cy >> 16);
        if sx >= 0
            && sy >= 0
            && sx as usize + n <= source.width
            && sy as usize + y1 - y0 <= source.height
        {
            for y in y0..y1 {
                let start = (sy as usize + y - y0) * source.width + sx as usize;
                let (left, right) = clip(y);
                let left = left.max(x0).min(x1);
                let right = right.min(x1).max(left);
                let row = &source.pixels[start + left - x0..start + right - x0];
                composite(
                    &mut out[y * width + left..y * width + right],
                    [row; 4],
                    [0; 2],
                    0,
                    left,
                    y,
                    black,
                    alpha,
                );
            }
            return;
        }
    }
    let mut scratch = texture.scanlines.as_ref().unwrap().borrow_mut();
    scratch.tags = [[i64::MIN; 2]; 2];
    let footprint = inverse_x.max(inverse_y) as u32;
    let level = ((31 - footprint.max(65536).leading_zeros()).saturating_sub(16) as usize)
        .min(texture.levels.len() - 1);
    let lod = if level + 1 < texture.levels.len() {
        ((footprint >> level).saturating_sub(65536) >> 8).min(256)
    } else {
        0
    };
    let count = if lod == 0 { 1 } else { 2 };
    for l in 0..count {
        for i in 0..n {
            let sx = (((((x0 + i) as i64) << 16) + (1 << 15) - cx) * inverse_x) >> 16;
            let sx = (sx >> (level + l)) - (1 << 15);
            scratch.columns[l][i] = Column {
                index: i16::try_from(sx.div_euclid(65536)).expect("bounded cabinet coordinate"),
                weight: ((sx & 65535) >> 8) as u16,
            };
        }
        let columns = &scratch.columns[l][..n];
        let first = columns.iter().position(|c| c.index >= 0).unwrap_or(n);
        let end = first
            + columns[first..]
                .iter()
                .position(|c| c.index as usize + 1 >= texture.levels[level + l].width)
                .unwrap_or(n - first);
        scratch.interior[l] = (first, end);
    }
    for y in y0..y1 {
        let sy = ((((y as i64) << 16) + (1 << 15) - cy) * inverse_y) >> 16;
        let mut weights = [0; 2];
        let mut slots = [0; 2];
        for l in 0..count {
            let sy = (sy >> (level + l)) - (1 << 15);
            let iy = sy.div_euclid(65536);
            weights[l] = ((sy & 65535) >> 8) as u32;
            slots[l] = (iy & 1) as usize;
            let source = &texture.levels[level + l];
            for dy in 0..2 {
                let row = iy + dy;
                let slot = (row & 1) as usize;
                if scratch.tags[l][slot] == row {
                    continue;
                }
                let Scanlines {
                    columns,
                    interior,
                    rows,
                    tags,
                } = &mut *scratch;
                let dest = &mut rows[l][slot][..n];
                if row < 0 || row >= source.height as i64 {
                    dest.fill(0);
                } else {
                    horizontal(
                        &source.pixels
                            [row as usize * source.width..(row as usize + 1) * source.width],
                        &columns[l][..n],
                        dest,
                        interior[l],
                    );
                }
                tags[l][slot] = row;
            }
        }
        let (left, right) = clip(y);
        let left = left.max(x0).min(x1);
        let right = right.min(x1).max(left);
        let range = left - x0..right - x0;
        composite(
            &mut out[y * width + left..y * width + right],
            [
                &scratch.rows[0][slots[0]][range.clone()],
                &scratch.rows[0][slots[0] ^ 1][range.clone()],
                &scratch.rows[1][slots[1]][range.clone()],
                &scratch.rows[1][slots[1] ^ 1][range.clone()],
            ],
            weights,
            lod,
            left,
            y,
            black,
            alpha,
        );
    }
}
fn horizontal(source: &[u32], columns: &[Column], out: &mut [u32], interior: (usize, usize)) {
    assert!(interior.0 <= interior.1 && interior.1 <= out.len());
    assert_eq!(columns.len(), out.len());
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn magik_cabinet_horizontal(
                out: *mut u32,
                source: *const u32,
                width: usize,
                columns: *const Column,
                n: usize,
                first: usize,
                end: usize,
            );
        }
        // SAFETY: frame setup checks the monotonic mapping's interior against
        // the source width. The kernel checks border indices and handles tails.
        unsafe {
            magik_cabinet_horizontal(
                out.as_mut_ptr(),
                source.as_ptr(),
                source.len(),
                columns.as_ptr(),
                out.len(),
                interior.0,
                interior.1,
            );
        }
    }
    #[cfg(not(target_arch = "arm"))]
    for (dest, col) in out.iter_mut().zip(columns) {
        let at = |i: i16| {
            if i < 0 {
                0
            } else {
                source.get(i as usize).copied().unwrap_or(0)
            }
        };
        *dest = mix(at(col.index), at(col.index + 1), u32::from(col.weight));
    }
}
#[allow(clippy::too_many_arguments)]
fn composite(
    out: &mut [Rgb565Pixel],
    rows: [&[u32]; 4],
    weights: [u32; 2],
    lod: u32,
    x: usize,
    y: usize,
    black: bool,
    alpha: u32,
) {
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn magik_artwork_composite(
                out: *mut u16,
                a: *const u32,
                b: *const u32,
                c: *const u32,
                d: *const u32,
                n: usize,
                wy: u32,
                wy2: u32,
                lod: u32,
                x: usize,
                y: usize,
                black: u32,
                opacity: u32,
            );
        }
        // SAFETY: render supplies four rows of exactly out.len() elements;
        // output is RGB565, all weights are 0..=256, and the kernel handles tails.
        unsafe {
            magik_artwork_composite(
                out.as_mut_ptr().cast(),
                rows[0].as_ptr(),
                rows[1].as_ptr(),
                rows[2].as_ptr(),
                rows[3].as_ptr(),
                out.len(),
                weights[0],
                weights[1],
                lod,
                x,
                y,
                u32::from(black),
                alpha,
            );
        }
    }
    #[cfg(not(target_arch = "arm"))]
    let _ = black;
    #[cfg(not(target_arch = "arm"))]
    for (i, dest) in out.iter_mut().enumerate() {
        let a = mix(rows[0][i], rows[1][i], weights[0]);
        let p = if lod == 0 {
            a
        } else {
            mix(a, mix(rows[2][i], rows[3][i], weights[1]), lod)
        };
        *dest = over_dithered(
            if alpha == 256 { p } else { mix(0, p, alpha) },
            *dest,
            x + i,
            y,
        );
    }
}
pub(super) fn base(
    home: &[Rgb565Pixel],
    arcade: &[Rgb565Pixel],
    out: &mut [Rgb565Pixel],
    launcher_alpha: u32,
    chrome_alpha: u32,
    rows: (usize, usize),
) -> bool {
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn mister_magik_arcade_base(
                out: *mut u16,
                home: *const u16,
                arcade: *const u16,
                a: u16,
                b: u16,
                y0: usize,
                y1: usize,
            );
        }
        assert_eq!(home.len(), 960 * 540);
        assert_eq!(arcade.len(), home.len());
        assert_eq!(out.len(), home.len());
        // SAFETY: kernel dimensions are fixed at the validated 960x540 HDMI surface.
        unsafe {
            mister_magik_arcade_base(
                out.as_mut_ptr().cast(),
                home.as_ptr().cast(),
                arcade.as_ptr().cast(),
                ((launcher_alpha + 4) >> 3) as u16,
                ((chrome_alpha + 4) >> 3) as u16,
                rows.0,
                rows.1,
            );
        }
        true
    }
    #[cfg(not(target_arch = "arm"))]
    {
        let _ = (home, arcade, out, launcher_alpha, chrome_alpha, rows);
        false
    }
}
pub(super) fn over(out: &mut [Rgb565Pixel], source: &[Rgb565Pixel], alpha: u32) -> bool {
    assert_eq!(out.len(), source.len());
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn mister_magik_arcade_over(out: *mut u16, source: *const u16, n: usize, alpha: u16);
        }
        // SAFETY: equally sized, disjoint slices; kernel handles all lengths.
        unsafe {
            mister_magik_arcade_over(
                out.as_mut_ptr().cast(),
                source.as_ptr().cast(),
                out.len(),
                ((alpha + 4) >> 3) as u16,
            );
        }
        true
    }
    #[cfg(not(target_arch = "arm"))]
    {
        let _ = (out, source, alpha);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipped_anisotropic_rows_match_direct_bilinear_mip_reference() {
        let rgb: Vec<u8> = (0..412 * 374 * 3)
            .map(|i| (i * 73 + i / 39) as u8)
            .collect();
        let texture = CabinetTexture::from_rgb888_sized(412, 374, &rgb).unwrap();
        let width = 640;
        let height = 240;
        let bounds = (90, 450, 30, 220);
        for (inverse_x, inverse_y, alpha) in [
            (65536, 65536, 256),
            (95000, 160000, 256),
            (180000, 220000, 128),
        ] {
            let transform = (
                (100 << 16) + 12345,
                (40 << 16) + 45678,
                inverse_x,
                inverse_y,
            );
            let clip = |y: usize| {
                if y.is_multiple_of(13) {
                    (0, 0)
                } else {
                    (110 + y % 7, 400 - y % 11)
                }
            };
            let mut actual = vec![Rgb565Pixel(0x1234); width * height];
            let mut reference = actual.clone();
            render_region(
                &texture,
                &mut actual,
                bounds,
                transform,
                width,
                false,
                alpha,
                clip,
            );
            let footprint = inverse_x.max(inverse_y) as u32;
            let level = ((31 - footprint.max(65536).leading_zeros()).saturating_sub(16) as usize)
                .min(texture.levels.len() - 1);
            let lod = if level + 1 < texture.levels.len() {
                ((footprint >> level).saturating_sub(65536) >> 8).min(256)
            } else {
                0
            };
            // Direct four-texel sampling has no column tables or cached rows.
            for y in bounds.2..bounds.3 {
                let (left, right) = clip(y);
                for x in left.max(bounds.0)..right.min(bounds.1) {
                    let sample = |l: usize| {
                        let source = &texture.levels[l];
                        let u = (((((x as i64) << 16) + 32768 - transform.0) * inverse_x)
                            >> (16 + l))
                            - 32768;
                        let v = (((((y as i64) << 16) + 32768 - transform.1) * inverse_y)
                            >> (16 + l))
                            - 32768;
                        let ix = u.div_euclid(65536);
                        let iy = v.div_euclid(65536);
                        let at = |x: i64, y: i64| {
                            if x < 0
                                || y < 0
                                || x >= source.width as i64
                                || y >= source.height as i64
                            {
                                0
                            } else {
                                source.pixels[y as usize * source.width + x as usize]
                            }
                        };
                        mix(
                            mix(at(ix, iy), at(ix + 1, iy), ((u & 65535) >> 8) as u32),
                            mix(
                                at(ix, iy + 1),
                                at(ix + 1, iy + 1),
                                ((u & 65535) >> 8) as u32,
                            ),
                            ((v & 65535) >> 8) as u32,
                        )
                    };
                    let a = sample(level);
                    let p = if lod == 0 {
                        a
                    } else {
                        mix(a, sample(level + 1), lod)
                    };
                    let p = if alpha == 256 { p } else { mix(0, p, alpha) };
                    reference[y * width + x] = over_dithered(p, reference[y * width + x], x, y);
                }
            }
            assert_eq!(actual, reference, "transform={transform:?} alpha={alpha}");
        }
    }
}
