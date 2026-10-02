// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Allocation-free RGB565 primitives shared by launcher-card page reveals.

/// Normalised progress of a window starting at `at` lasting `duration`, Q16.
pub(crate) fn window_q16(t: u32, at: u32, duration: u32) -> i64 {
    if t <= at {
        0
    } else if t >= at + duration {
        1 << 16
    } else {
        (i64::from(t - at) << 16) / i64::from(duration)
    }
}

pub(crate) fn ease_in_out(p: i64) -> i64 {
    if p < 1 << 15 {
        4 * p * p / (1 << 16) * p / (1 << 16)
    } else {
        let q = (2 << 16) - 2 * p;
        (1 << 16) - q * q / (1 << 16) * q / (1 << 16) / 2
    }
}

pub(crate) fn ease_out(p: i64) -> i64 {
    let q = (1 << 16) - p;
    let q2 = (q * q) >> 16;
    (1 << 16) - ((q2 * q2) >> 16)
}

#[inline]
pub(crate) fn lerp_rgb565(a: u16, b: u16, fraction: u32) -> u16 {
    let fraction = fraction.min(32);
    let inverse = 32 - fraction;
    let a = u32::from(a);
    let b = u32::from(b);
    let red_blue = ((((a & 0xf81f) * inverse) + ((b & 0xf81f) * fraction)) >> 5) & 0xf81f;
    let green = ((((a & 0x07e0) * inverse) + ((b & 0x07e0) * fraction)) >> 5) & 0x07e0;
    (red_blue | green) as u16
}

#[inline]
pub(crate) fn blend(under: u16, over: u16, alpha: u32) -> u16 {
    if alpha >= 256 {
        return over;
    }
    if alpha == 0 {
        return under;
    }
    lerp_rgb565(under, over, (alpha + 4) >> 3)
}

/// Fade a complete source from black using the same five-bit channel weights
/// as `blend`; endpoint frames need only a fill or a copy.
pub(crate) fn fade_from_black(
    output: &mut [crate::Rgb565Pixel],
    source: &[crate::Rgb565Pixel],
    alpha: u32,
) {
    assert_eq!(output.len(), source.len());
    assert!(alpha <= 256);
    if alpha == 0 {
        output.fill(crate::Rgb565Pixel(0));
    } else if alpha == 256 {
        output.copy_from_slice(source);
    } else if !crate::blend_rgb565_black_neon_if_available(
        output,
        source,
        0,
        source.len(),
        ((alpha + 4) >> 3) as u16,
        true,
    ) {
        for (out, src) in output.iter_mut().zip(source) {
            out.0 = blend(0, src.0, alpha);
        }
    }
}

/// Blend already rendered UI pixels without changing their native grid.
pub(crate) fn blend_row(
    output: &mut [crate::Rgb565Pixel],
    source: &[crate::Rgb565Pixel],
    alpha: u32,
) {
    assert_eq!(output.len(), source.len());
    assert!(alpha <= 256);
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn mister_magik_arcade_over(out: *mut u16, source: *const u16, n: usize, alpha: u16);
        }
        // SAFETY: equally sized disjoint RGB565 slices; kernel handles tails.
        unsafe {
            mister_magik_arcade_over(
                output.as_mut_ptr().cast(),
                source.as_ptr().cast(),
                output.len(),
                ((alpha + 4) >> 3) as u16,
            );
        }
    }
    #[cfg(not(target_arch = "arm"))]
    for (out, src) in output.iter_mut().zip(source) {
        out.0 = blend(out.0, src.0, alpha);
    }
}

pub(crate) fn alpha_of(q16: i64) -> u32 {
    ((q16.clamp(0, 1 << 16) * 256 + (1 << 15)) >> 16) as u32
}

/// Horizontal span of a rounded rectangle on row `y` (all Q16 except y).
pub(crate) fn rounded_span(
    y: i32,
    x: i64,
    top: i64,
    w: i64,
    h: i64,
    radius: i64,
    frame_width: usize,
) -> Option<(usize, usize)> {
    let yc = (i64::from(y) << 16) + (1 << 15);
    if yc < top || yc >= top + h {
        return None;
    }
    let radius = radius.min(w / 2).min(h / 2).max(0);
    let dy = if yc < top + radius {
        top + radius - yc
    } else if yc > top + h - radius {
        yc - (top + h - radius)
    } else {
        0
    };
    let inset = if dy > 0 {
        let r2 = (radius >> 4) * (radius >> 4);
        let d2 = (dy >> 4) * (dy >> 4);
        let chord = ((r2 - d2).max(0) as f64).sqrt() as i64;
        radius - (chord << 4)
    } else {
        0
    };
    let x0 = ((x + inset + (1 << 15)) >> 16).clamp(0, frame_width as i64) as usize;
    let x1 = ((x + w - inset + (1 << 15)) >> 16).clamp(0, frame_width as i64) as usize;
    (x0 < x1).then_some((x0, x1))
}

#[cfg(test)]
mod fade_tests {
    use super::*;
    use crate::Rgb565Pixel;

    #[test]
    fn black_fade_matches_scalar_for_every_colour_and_alpha() {
        let source = (0..=u16::MAX).map(Rgb565Pixel).collect::<Vec<_>>();
        let mut output = vec![Rgb565Pixel(0xbeef); source.len() + 2];
        for alpha in 0..=256 {
            fade_from_black(&mut output[1..source.len() + 1], &source, alpha);
            assert_eq!(output[0], Rgb565Pixel(0xbeef));
            assert_eq!(output[source.len() + 1], Rgb565Pixel(0xbeef));
            for (actual, source) in output[1..source.len() + 1].iter().zip(&source) {
                assert_eq!(actual.0, blend(0, source.0, alpha), "alpha={alpha}");
            }
        }
    }
}
