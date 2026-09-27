// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Home <-> Arcade launcher-card reveal for HDMI and CRT rasters.

use crate::Rgb565Pixel;
use crate::card_page::{alpha_of, blend, ease_in_out, ease_out, rounded_span, window_q16};
use crate::navigation::NavigationTransitionRect;

pub const ARCADE_CARD_DURATION_MS: u32 = 1_000;
pub const CABINET_WIDTH: usize = 483;
pub const CABINET_HEIGHT: usize = 519;

const HDMI_CARD: NavigationTransitionRect = NavigationTransitionRect {
    x: 520,
    y: 158,
    width: 180,
    height: 252,
};
const HDMI_CABINET_X: i32 = 490;
const HDMI_CABINET_Y: i32 = 35;
const HDMI_SCREEN: NavigationTransitionRect = NavigationTransitionRect {
    x: 572,
    y: 96,
    width: 320,
    height: 320,
};
const LIST_LEFT: usize = 26;
const LIST_RIGHT: usize = 488;
const LIST_BANDS: [(usize, usize); 11] = [
    (101, 124),
    (124, 160),
    (160, 196),
    (196, 232),
    (232, 268),
    (268, 304),
    (304, 340),
    (340, 376),
    (376, 412),
    (412, 448),
    (448, 494),
];
const RED: u16 = rgb565(231, 105, 90);

const fn rgb565(r: u16, g: u16, b: u16) -> u16 {
    ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
}

#[must_use]
pub fn supports_dimensions(width: usize, height: usize) -> bool {
    width > 0 && height > 0 && width.saturating_mul(height) <= 960 * 576
}

#[allow(clippy::too_many_arguments)]
pub fn render_arcade_card_transition_into(
    width: usize,
    height: usize,
    launcher: &[Rgb565Pixel],
    arcade: &[Rgb565Pixel],
    cabinet: &[Rgb565Pixel],
    source_card: NavigationTransitionRect,
    t_ms: u32,
    output: &mut [Rgb565Pixel],
) -> bool {
    let len = width.saturating_mul(height);
    if !supports_dimensions(width, height)
        || launcher.len() != len
        || arcade.len() != len
        || output.len() != len
        || cabinet.len() != CABINET_WIDTH * CABINET_HEIGHT
    {
        return false;
    }
    let t = t_ms.min(ARCADE_CARD_DURATION_MS);
    if t == 0 {
        output.copy_from_slice(launcher);
        return true;
    }
    if t == ARCADE_CARD_DURATION_MS {
        output.copy_from_slice(arcade);
        return true;
    }
    if (width, height) == (960, 540) {
        render_hdmi(launcher, arcade, cabinet, t, output);
    } else {
        render_crt(width, height, launcher, arcade, source_card, t, output);
    }
    true
}

fn render_hdmi(
    launcher: &[Rgb565Pixel],
    arcade: &[Rgb565Pixel],
    cabinet: &[Rgb565Pixel],
    t: u32,
    output: &mut [Rgb565Pixel],
) {
    const W: usize = 960;
    const H: usize = 540;
    let launcher_alpha = 256_u32.saturating_sub(alpha_of(ease_out(window_q16(t, 120, 360))));
    let chrome_alpha = alpha_of(ease_out(window_q16(t, 220, 300)));
    for (index, pixel) in output.iter_mut().enumerate() {
        let x = index % W;
        let y = index / W;
        let destination_is_subject = (490..960).contains(&x) && (35..540).contains(&y)
            || (LIST_LEFT..LIST_RIGHT).contains(&x) && (96..500).contains(&y);
        let chrome = if destination_is_subject {
            0
        } else {
            arcade[index].0
        };
        let base = blend(0, launcher[index].0, launcher_alpha);
        *pixel = Rgb565Pixel(blend(base, chrome, chrome_alpha));
    }

    // The cabinet camera shares the card framing. Interpolate from that crop
    // to the exact 1:1 resting asset without introducing an intermediate PNG.
    let p = ease_in_out(window_q16(t, 80, 760));
    let start_scale = (i64::from(HDMI_CARD.width) << 16) / 423;
    let start_x = (i64::from(HDMI_CARD.x) << 16) - 30 * start_scale;
    let start_y = (i64::from(HDMI_CARD.y) << 16) - 28 * start_scale;
    let lerp = |a: i64, b: i64| a + (((b - a) * p) >> 16);
    let cabinet_x = lerp(start_x, i64::from(HDMI_CABINET_X) << 16);
    let cabinet_y = lerp(start_y, i64::from(HDMI_CABINET_Y) << 16);
    let scale = lerp(start_scale, 1 << 16).max(1);
    let inverse = (1_i64 << 32) / scale;
    let x0 = (cabinet_x >> 16).max(0) as usize;
    let x1 = (((cabinet_x + CABINET_WIDTH as i64 * scale) >> 16) + 1).clamp(0, W as i64) as usize;
    let y0 = (cabinet_y >> 16).max(0) as usize;
    let y1 = (((cabinet_y + CABINET_HEIGHT as i64 * scale) >> 16) + 1).clamp(0, H as i64) as usize;
    for y in y0..y1 {
        let source_y = (((((y as i64) << 16) + (1 << 15) - cabinet_y) * inverse) >> 16) >> 16;
        if !(0..CABINET_HEIGHT as i64).contains(&source_y) {
            continue;
        }
        for x in x0..x1 {
            let source_x = (((((x as i64) << 16) + (1 << 15) - cabinet_x) * inverse) >> 16) >> 16;
            if !(0..CABINET_WIDTH as i64).contains(&source_x) {
                continue;
            }
            let sampled = cabinet[source_y as usize * CABINET_WIDTH + source_x as usize].0;
            if sampled != 0 {
                output[y * W + x] = Rgb565Pixel(sampled);
            }
        }
    }

    draw_outline(W, H, HDMI_CARD, t, output);

    let screen_alpha = alpha_of(ease_out(window_q16(t, 760, 160)));
    copy_rect_alpha(W, arcade, output, HDMI_SCREEN, screen_alpha, 0);
    for (index, &(top, bottom)) in LIST_BANDS.iter().enumerate() {
        let p = ease_out(window_q16(t, 500 + index as u32 * 22, 280));
        let offset = ((32 * ((1 << 16) - p) + (1 << 15)) >> 16) as usize;
        copy_band_alpha(
            W,
            arcade,
            output,
            LIST_LEFT,
            LIST_RIGHT,
            top,
            bottom,
            alpha_of(p),
            offset,
        );
    }
    let footer_alpha = alpha_of(ease_out(window_q16(t, 640, 220)));
    copy_band_alpha(W, arcade, output, 0, W, 500, H, footer_alpha, 0);
}

fn render_crt(
    width: usize,
    height: usize,
    launcher: &[Rgb565Pixel],
    arcade: &[Rgb565Pixel],
    source_card: NavigationTransitionRect,
    t: u32,
    output: &mut [Rgb565Pixel],
) {
    let p = ease_in_out(window_q16(t, 0, 760));
    let fade = alpha_of(ease_out(window_q16(t, 120, 360)));
    for ((out, from), to) in output.iter_mut().zip(launcher).zip(arcade) {
        *out = Rgb565Pixel(blend(from.0, 0, fade));
        let late = alpha_of(ease_out(window_q16(t, 560, 360)));
        *out = Rgb565Pixel(blend(out.0, to.0, late));
    }

    let card = if source_card.width == 0 || source_card.height == 0 {
        NavigationTransitionRect {
            x: (width / 2).saturating_sub(45) as u16,
            y: (height / 2).saturating_sub(63) as u16,
            width: 90,
            height: 126,
        }
    } else {
        source_card
    };
    let x = ((i64::from(card.x) * ((1 << 16) - p)) >> 16) as usize;
    let y = ((i64::from(card.y) * ((1 << 16) - p)) >> 16) as usize;
    let w = (i64::from(card.width) + (((width as i64 - i64::from(card.width)) * p) >> 16)) as usize;
    let h =
        (i64::from(card.height) + (((height as i64 - i64::from(card.height)) * p) >> 16)) as usize;
    for destination_y in y..(y + h).min(height) {
        let source_y = (destination_y - y).saturating_mul(height) / h.max(1);
        for destination_x in x..(x + w).min(width) {
            let source_x = (destination_x - x).saturating_mul(width) / w.max(1);
            output[destination_y * width + destination_x] = arcade[source_y * width + source_x];
        }
    }
    draw_outline(width, height, card, t, output);
}

fn draw_outline(
    width: usize,
    height: usize,
    card: NavigationTransitionRect,
    t: u32,
    output: &mut [Rgb565Pixel],
) {
    let p = ease_in_out(window_q16(t, 0, 760));
    let z = (1_i64 << 16) + (((7_i64 << 16) * p) >> 16);
    let cx = i64::from(card.x) + i64::from(card.width) / 2;
    let cy = i64::from(card.y) + i64::from(card.height) / 2;
    let w = i64::from(card.width) * z;
    let h = i64::from(card.height) * z;
    let x = (cx << 16) - w / 2;
    let y = (cy << 16) - h / 2;
    let radius = 8 * z;
    let stroke = 2_i64 << 16;
    let alpha = 256_u32.saturating_sub(alpha_of((p * p) >> 16).saturating_mul(5) / 4);
    if alpha == 0 {
        return;
    }
    for row in 0..height {
        if let Some((outer0, outer1)) = rounded_span(
            row as i32,
            x - stroke,
            y - stroke,
            w + 2 * stroke,
            h + 2 * stroke,
            radius + stroke,
            width,
        ) {
            let inner = rounded_span(row as i32, x, y, w, h, radius, width);
            let (inner0, inner1) = inner.unwrap_or((outer1, outer1));
            for column in (outer0..inner0).chain(inner1..outer1) {
                let index = row * width + column;
                output[index] = Rgb565Pixel(blend(output[index].0, RED, alpha));
            }
        }
    }
}

fn copy_rect_alpha(
    width: usize,
    source: &[Rgb565Pixel],
    destination: &mut [Rgb565Pixel],
    rect: NavigationTransitionRect,
    alpha: u32,
    offset: usize,
) {
    copy_band_alpha(
        width,
        source,
        destination,
        rect.x as usize,
        rect.right() as usize,
        rect.y as usize,
        rect.bottom() as usize,
        alpha,
        offset,
    );
}

#[allow(clippy::too_many_arguments)]
fn copy_band_alpha(
    width: usize,
    source: &[Rgb565Pixel],
    destination: &mut [Rgb565Pixel],
    left: usize,
    right: usize,
    top: usize,
    bottom: usize,
    alpha: u32,
    offset: usize,
) {
    if alpha == 0 {
        return;
    }
    for y in top..bottom {
        for x in left..right {
            let target_x = x + offset;
            if target_x >= width {
                break;
            }
            let source_index = y * width + x;
            let target_index = y * width + target_x;
            destination[target_index] = Rgb565Pixel(blend(
                destination[target_index].0,
                source[source_index].0,
                alpha,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_exact() {
        let launcher = vec![Rgb565Pixel(0x1234); 960 * 540];
        let arcade = vec![Rgb565Pixel(0x4321); 960 * 540];
        let cabinet = vec![Rgb565Pixel(0); CABINET_WIDTH * CABINET_HEIGHT];
        let mut output = vec![Rgb565Pixel(0); 960 * 540];
        assert!(render_arcade_card_transition_into(
            960,
            540,
            &launcher,
            &arcade,
            &cabinet,
            HDMI_CARD,
            0,
            &mut output
        ));
        assert_eq!(output, launcher);
        assert!(render_arcade_card_transition_into(
            960,
            540,
            &launcher,
            &arcade,
            &cabinet,
            HDMI_CARD,
            ARCADE_CARD_DURATION_MS,
            &mut output
        ));
        assert_eq!(output, arcade);
    }
}
