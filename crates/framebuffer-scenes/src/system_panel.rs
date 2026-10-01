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
        582
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

#[derive(Clone, Copy)]
struct Band {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
}
fn bands(crt: bool, hub: bool, width: usize, height: usize) -> ([Band; 7], usize) {
    let empty = Band {
        x0: 0,
        x1: 0,
        y0: 0,
        y1: 0,
    };
    let mut result = [empty; 7];
    if crt {
        let count = if hub { 4 } else { 7 };
        for (i, b) in result.iter_mut().enumerate().take(count) {
            let (y0, y1) = if hub {
                if i == 0 {
                    (35, 52)
                } else {
                    (52 + (i - 1) * 16, 68 + (i - 1) * 16)
                }
            } else {
                (52 + i * 16, 68 + i * 16)
            };
            *b = Band {
                x0: 38 * width / 640,
                x1: 602 * width / 640,
                y0: y0 * height / 240,
                y1: y1 * height / 240,
            };
        }
        (result, count)
    } else if hub {
        for (b, (x0, x1, y0, y1)) in result.iter_mut().zip([
            (26, 488, 104, 126),
            (26, 488, 126, 194),
            (26, 488, 194, 246),
            (26, 176, 302, 450),
            (176, 326, 302, 450),
            (326, 488, 302, 450),
            (26, 488, 462, 498),
        ]) {
            *b = Band { x0, x1, y0, y1 };
        }
        (result, 7)
    } else {
        result[0] = Band {
            x0: 26,
            x1: 488,
            y0: 48,
            y1: 76,
        };
        result[1] = Band {
            x0: 26,
            x1: 488,
            y0: 124,
            y1: 498,
        };
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
    for band in out_bands[..out_count]
        .iter()
        .chain(in_bands[..in_count].iter())
    {
        for y in band.y0..band.y1.min(height) {
            for x in band.x0..band.x1.min(width) {
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
            -(if crt { 24 } else { 40 }) * dir * out_p / 65536,
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
                (if crt { 24 } else { 32 }) * dir * (65536 - p) / 65536,
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
fn blit(
    width: usize,
    height: usize,
    source: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    b: Band,
    dx: i64,
    alpha: u32,
    backdrop: Option<&[Rgb565Pixel]>,
) {
    if alpha == 0 {
        return;
    }
    for y in b.y0..b.y1.min(height) {
        for sx in b.x0..b.x1.min(width) {
            if backdrop.is_some_and(|bg| bg.get(y * width + sx) == Some(&source[y * width + sx])) {
                continue;
            }
            let x = sx as i64 + dx;
            if x < b.x0 as i64 || x >= b.x1.min(width) as i64 {
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
