// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! System overview/list bands over a stationary device or CRT backdrop.
use crate::Rgb565Pixel;
use crate::card_page::{alpha_of, blend, ease_in_out, ease_out, window_q16};

pub fn duration_ms(crt: bool, to_list: bool) -> u32 {
    if crt {
        340
    } else if to_list {
        426
    } else {
        120 + (HDMI_HUB_BANDS.len() as u32 - 1) * 26 + 280
    }
}

// Solve x(u) for CSS cubic-bezier, then evaluate y(u). Bounded and allocation-free.
fn bezier(p: i64, x1: f64, y1: f64, x2: f64, y2: f64) -> i64 {
    let target = p as f64 / 65536.0;
    let curve = |u: f64, a: f64, b: f64| {
        3.0 * (1.0 - u) * (1.0 - u) * u * a + 3.0 * (1.0 - u) * u * u * b + u * u * u
    };
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..16 {
        let mid = (lo + hi) * 0.5;
        if curve(mid, x1, x2) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (curve((lo + hi) * 0.5, y1, y2) * 65536.0).round() as i64
}

/// Exclusive horizontal and vertical bounds: (left, right, top, bottom).
pub(crate) type Band = (usize, usize, usize, usize);
pub(crate) const HDMI_HUB_BANDS: [Band; 7] = [
    (26, 488, 104, 126),
    (26, 488, 126, 194),
    (26, 488, 194, 246),
    (26, 176, 302, 450),
    (176, 326, 302, 450),
    (326, 488, 302, 450),
    (26, 488, 462, 498),
];

pub(crate) fn crt_hub_bands(width: usize, height: usize) -> [Band; 4] {
    let narrow = width.min(height);
    let sy = if narrow >= 400 || (narrow <= 288 && height > width && height >= 640) {
        2
    } else {
        1
    };
    let header = (height * 5 / 100).max(6 * sy) + 18 * sy;
    let first_row = header + 26 * sy;
    std::array::from_fn(|i| {
        let (top, bottom) = if i == 0 {
            (header + 8, first_row)
        } else {
            (first_row + (i - 1) * 16 * sy, first_row + i * 16 * sy)
        };
        (38 * width / 640, 602 * width / 640, top, bottom)
    })
}

fn bands(crt: bool, hub: bool, width: usize, height: usize) -> ([Band; 7], usize) {
    let mut result = [(0, 0, 0, 0); 7];
    if crt {
        if hub {
            result[..4].copy_from_slice(&crt_hub_bands(width, height));
            return (result, 4);
        }
        let count = 7;
        for (i, b) in result.iter_mut().enumerate().take(count) {
            let (y0, y1) = (52 + i * 16, 68 + i * 16);
            *b = (
                38 * width / 640,
                602 * width / 640,
                y0 * height / 240,
                y1 * height / 240,
            );
        }
        (result, count)
    } else if hub {
        (HDMI_HUB_BANDS, 7)
    } else {
        result[0] = (26, 488, 48, 76);
        result[1] = (26, 488, 124, 498);
        (result, 2)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_into(
    width: usize,
    height: usize,
    source: &[Rgb565Pixel],
    destination: Option<&[Rgb565Pixel]>,
    backdrop: &[Rgb565Pixel],
    crt: bool,
    to_list: bool,
    t: u32,
    output: &mut [Rgb565Pixel],
) -> bool {
    let len = width.saturating_mul(height);
    if len == 0
        || source.len() != len
        || output.len() != len
        || destination.is_some_and(|d| d.len() != len)
    {
        return false;
    }
    if t == 0 {
        output.copy_from_slice(source);
        return true;
    }
    if t >= duration_ms(crt, to_list) {
        output.copy_from_slice(destination.unwrap_or(source));
        return true;
    }
    output.copy_from_slice(source);
    let (out_bands, out_count) = bands(crt, to_list, width, height);
    let (in_bands, in_count) = bands(crt, !to_list, width, height);
    // Clear only the panel's subjects; retain chrome and the device exactly.
    for &(left, right, top, bottom) in out_bands[..out_count]
        .iter()
        .chain(in_bands[..in_count].iter())
    {
        for y in top..bottom.min(height) {
            for x in left..right.min(width) {
                output[y * width + x] = if crt {
                    backdrop
                        .get(y * width + x)
                        .copied()
                        .unwrap_or(Rgb565Pixel(0))
                } else {
                    Rgb565Pixel(0)
                };
            }
        }
    }
    let dir = if to_list { 1 } else { -1 };
    let out_p = window_q16(t, 0, if crt { 150 } else { 180 });
    let out_p = if crt {
        ease_in_out(out_p)
    } else {
        bezier(out_p, 0.4, 0.0, 1.0, 1.0)
    };
    for b in &out_bands[..out_count] {
        blit(
            width,
            height,
            source,
            output,
            *b,
            (-(if crt { 24 } else { 40 }) * dir * out_p / 65536) as isize,
            256 - alpha_of(out_p),
            if crt { Some(backdrop) } else { None },
        );
    }
    if let Some(destination) = destination {
        for (i, b) in in_bands[..in_count].iter().enumerate() {
            let p = window_q16(
                t,
                if crt {
                    150 + i as u32 * 14
                } else {
                    120 + i as u32 * 26
                },
                if crt { 190 } else { 280 },
            );
            let p = if crt {
                ease_out(p)
            } else {
                bezier(p, 0.2, 0.8, 0.2, 1.0)
            };
            blit(
                width,
                height,
                destination,
                output,
                *b,
                ((if crt { 24 } else { 32 }) * dir * (65536 - p) / 65536) as isize,
                alpha_of(p),
                if crt { Some(backdrop) } else { None },
            );
        }
        if t >= if crt { 150 } else { 120 } {
            let top = if crt { height * 210 / 240 } else { 500 };
            output[top * width..].copy_from_slice(&destination[top * width..]);
        }
    }
    true
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn blit(
    width: usize,
    height: usize,
    source: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    (left, right, top, bottom): Band,
    dx: isize,
    alpha: u32,
    backdrop: Option<&[Rgb565Pixel]>,
) {
    if alpha == 0 {
        return;
    }
    for y in top..bottom.min(height) {
        for sx in left..right.min(width) {
            if backdrop.is_some_and(|bg| bg.get(y * width + sx) == Some(&source[y * width + sx])) {
                continue;
            }
            let x = sx as isize + dx;
            if x < left as isize || x >= right.min(width) as isize {
                continue;
            }
            let dst = y * width + x as usize;
            output[dst] = Rgb565Pixel(blend(output[dst].0, source[y * width + sx].0, alpha));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incoming_list_uses_list_bands_instead_of_tile_crops() {
        for (w, h, crt, t) in [(960, 540, false, 230), (640, 240, true, 330)] {
            let source = vec![Rgb565Pixel(0); w * h];
            let dest = vec![Rgb565Pixel(0xffff); w * h];
            let backdrop = vec![Rgb565Pixel(0); w * h];
            let mut out = source.clone();
            assert!(render_into(
                w,
                h,
                &source,
                Some(&dest),
                &backdrop,
                crt,
                true,
                t,
                &mut out
            ));
            if crt {
                assert_ne!(out[140 * w + 100].0, 0);
            } else {
                assert_ne!(out[250 * w + 100].0, 0);
                assert_eq!(out[250 * w + 100], out[420 * w + 100]);
                assert_eq!(out[76 * w + 100], source[76 * w + 100]);
            }
        }
    }

    #[test]
    fn panel_preserves_device_and_exact_endpoints() {
        for (w, h, crt) in [(960, 540, false), (640, 240, true)] {
            for to_list in [false, true] {
                let source = vec![Rgb565Pixel(0x1234); w * h];
                let dest = vec![Rgb565Pixel(0x5678); w * h];
                let backdrop = vec![Rgb565Pixel(0x0102); w * h];
                let mut out = vec![Rgb565Pixel(0); w * h];
                for t in [0, 119, 120, 149, 150, 220, duration_ms(crt, to_list)] {
                    assert!(render_into(
                        w,
                        h,
                        &source,
                        Some(&dest),
                        &backdrop,
                        crt,
                        to_list,
                        t,
                        &mut out
                    ));
                    if t == 0 {
                        assert!(out == source);
                    } else if t == duration_ms(crt, to_list) {
                        assert!(out == dest);
                    } else if !crt {
                        assert_eq!(out[200 * w + 600], source[200 * w + 600]);
                    }
                    assert_eq!(
                        out[20 * w + 40],
                        if t == duration_ms(crt, to_list) {
                            dest[20 * w + 40]
                        } else {
                            source[20 * w + 40]
                        }
                    );
                }
            }
        }
    }
    #[test]
    fn css_curve_endpoints_and_direction() {
        for curve in [(0.4, 0.0, 1.0, 1.0), (0.2, 0.8, 0.2, 1.0)] {
            assert!(bezier(0, curve.0, curve.1, curve.2, curve.3) <= 2);
            assert!(bezier(65536, curve.0, curve.1, curve.2, curve.3) >= 65534);
        }
    }
}
