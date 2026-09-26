// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Home -> Settings card zoom for the 960x540 landscape card launcher.
//!
//! The selected Settings card's outline zooms past the screen edges while the
//! full Settings cog, rendered from the same Blender camera at twice the card
//! framing, grows from its card-sized crop to its 1:1 resting position. The
//! Settings rows are then copied from Slint's own destination raster and slid
//! in one band at a time at whole-pixel offsets, so every glyph is Slint's.
//!
//! The renderer is a pure function of time: `t = 0` is exactly the launcher
//! frame and `t = SETTINGS_COG_DURATION_MS` is exactly the Settings frame.
//! Reverse playback evaluates the same timeline backwards.

use crate::Rgb565Pixel;

pub const SETTINGS_COG_WIDTH: usize = 960;
pub const SETTINGS_COG_HEIGHT: usize = 540;
pub const SETTINGS_COG_DURATION_MS: u32 = 1_000;

/// The backdrop asset: 412x374 RGB565, see apps/mister/assets/ui/settings.
pub const COG_ASSET_WIDTH: usize = 412;
pub const COG_ASSET_HEIGHT: usize = 374;

// Selected (centre) card of the landscape launcher: slot centre 610, half
// width 90, vertical centre 284 (crates/framebuffer-scenes/src/launcher.rs).
const CARD_X: i32 = 520;
const CARD_Y: i32 = 158;
const CARD_W: i32 = 180;
const CARD_H: i32 = 252;
const CARD_CX: i32 = CARD_X + CARD_W / 2;
const CARD_CY: i32 = CARD_Y + CARD_H / 2;
const CARD_RADIUS: i32 = 8;

// The 816x1142 backdrop render frames the card's 408-pixel-wide view in its
// centre (origin 204, 285.5); the packed asset is cropped at (312, 327).
const RENDER_CARD_X_Q1: i32 = 408; // 204 in half pixels
const RENDER_CARD_Y_Q1: i32 = 571; // 285.5 in half pixels
const RENDER_CARD_W: i32 = 408;
const ASSET_CROP_X: i32 = 312;
const ASSET_CROP_Y: i32 = 327;

// Resting cog position on the Settings screen (views/hdmi/settings.slint).
const COG_REST_X: i32 = -18;
const COG_REST_Y: i32 = 97;

// Content lies between the header rule (76) and the footer rule (500); the
// chrome above and on the rules is identical on both screens.
const CONTENT_TOP: usize = 77;
const CONTENT_BOTTOM: usize = 500;
const FOOTER_TOP: usize = 501;

const ZOOM_MAX_Q16: i64 = 8 << 16;
const OUTLINE: u16 = rgb565(156, 134, 231);

// Settings list bands in navigation order (headings interleaved), copied
// from the destination raster: (top, bottom) rows over x 400..934.
const LIST_LEFT: usize = 400;
const LIST_RIGHT: usize = 934;
const LIST_BANDS: [(usize, usize); 9] = [
    (104, 124),
    (124, 160),
    (160, 196),
    (196, 232),
    (232, 268),
    (286, 306),
    (306, 342),
    (342, 378),
    (378, 414),
];
const BAND_START_MS: u32 = 560;
const BAND_STAGGER_MS: u32 = 30;
const BAND_DURATION_MS: u32 = 200;
const BAND_TRAVEL: i32 = 40;

const fn rgb565(r: u16, g: u16, b: u16) -> u16 {
    ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
}

/// Normalised progress of a window starting at `at` lasting `duration`, Q16.
fn window_q16(t: u32, at: u32, duration: u32) -> i64 {
    if t <= at {
        0
    } else if t >= at + duration {
        1 << 16
    } else {
        (i64::from(t - at) << 16) / i64::from(duration)
    }
}

/// Cubic ease-in-out, Q16 -> Q16.
fn ease_in_out(p: i64) -> i64 {
    if p < 1 << 15 {
        4 * p * p / (1 << 16) * p / (1 << 16)
    } else {
        let q = (2 << 16) - 2 * p; // (-2p + 2) in Q16
        (1 << 16) - q * q / (1 << 16) * q / (1 << 16) / 2
    }
}

/// Quartic ease-out, Q16 -> Q16.
fn ease_out(p: i64) -> i64 {
    let q = (1 << 16) - p;
    let q2 = (q * q) >> 16;
    (1 << 16) - ((q2 * q2) >> 16)
}

/// 8^p for p in Q16, returned Q16. Piecewise exponential is exact at the ends.
fn zoom_q16(p: i64) -> i64 {
    // 8^p = 2^(3p): integer part by shift, fraction by a cubic fit of 2^f.
    let e = 3 * p; // Q16
    let whole = (e >> 16) as u32;
    let f = e & 0xffff;
    // 2^f ~= 1 + f*(0.6951 + f*(0.2262 + f*0.0787)), max error < 0.0002.
    let poly = (((((5158 * f) >> 16) + 14824) * f) >> 16) + 45553;
    let frac = (1 << 16) + ((poly * f) >> 16);
    (frac << whole).min(ZOOM_MAX_Q16)
}

#[inline]
fn unpack(p: u16) -> (u32, u32, u32) {
    (
        u32::from(p >> 11),
        u32::from((p >> 5) & 0x3f),
        u32::from(p & 0x1f),
    )
}

#[inline]
fn pack(r: u32, g: u32, b: u32) -> u16 {
    ((r.min(31) << 11) | (g.min(63) << 5) | b.min(31)) as u16
}

/// `over` on top of `under` with alpha in 0..=256.
#[inline]
fn blend(under: u16, over: u16, alpha: u32) -> u16 {
    if alpha >= 256 {
        return over;
    }
    if alpha == 0 {
        return under;
    }
    let (ur, ug, ub) = unpack(under);
    let (or, og, ob) = unpack(over);
    let inv = 256 - alpha;
    pack(
        (or * alpha + ur * inv) >> 8,
        (og * alpha + ug * inv) >> 8,
        (ob * alpha + ub * inv) >> 8,
    )
}

fn alpha_of(q16: i64) -> u32 {
    ((q16.clamp(0, 1 << 16) * 256 + (1 << 15)) >> 16) as u32
}

/// Horizontal span of a rounded rectangle on row `y` (all Q16 except y).
/// Returns the covered [x0, x1) in whole pixels, clipped to the frame.
fn rounded_span(y: i32, x: i64, top: i64, w: i64, h: i64, radius: i64) -> Option<(usize, usize)> {
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
        let chord = ((r2 - d2).max(0) as f64).sqrt() as i64; // Q12
        radius - (chord << 4)
    } else {
        0
    };
    let x0 = ((x + inset + (1 << 15)) >> 16).clamp(0, SETTINGS_COG_WIDTH as i64) as usize;
    let x1 = ((x + w - inset + (1 << 15)) >> 16).clamp(0, SETTINGS_COG_WIDTH as i64) as usize;
    (x0 < x1).then_some((x0, x1))
}

/// Bilinear RGB565 sample of the cog asset at (u, v) in Q16 asset pixels.
#[inline]
fn sample_cog(cog: &[Rgb565Pixel], u: i64, v: i64) -> u16 {
    let (x, y) = ((u >> 16) as i32, (v >> 16) as i32);
    let (fx, fy) = (((u >> 8) & 0xff) as u32, ((v >> 8) & 0xff) as u32);
    let at = |x: i32, y: i32| -> (u32, u32, u32) {
        if x < 0 || y < 0 || x >= COG_ASSET_WIDTH as i32 || y >= COG_ASSET_HEIGHT as i32 {
            (0, 0, 0)
        } else {
            unpack(cog[y as usize * COG_ASSET_WIDTH + x as usize].0)
        }
    };
    let (a, b, c, d) = (at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1));
    let lerp = |p: u32, q: u32, f: u32| p * (256 - f) + q * f;
    let row0 = (lerp(a.0, b.0, fx), lerp(a.1, b.1, fx), lerp(a.2, b.2, fx));
    let row1 = (lerp(c.0, d.0, fx), lerp(c.1, d.1, fx), lerp(c.2, d.2, fx));
    pack(
        (row0.0 * (256 - fy) + row1.0 * fy + 32768) >> 16,
        (row0.1 * (256 - fy) + row1.1 * fy + 32768) >> 16,
        (row0.2 * (256 - fy) + row1.2 * fy + 32768) >> 16,
    )
}

/// Render the Home -> Settings timeline at `t_ms` into `output`.
///
/// `launcher` and `settings` are the two settled 960x540 frames; `cog` is the
/// 412x374 backdrop asset that `settings` shows 1:1 at (-18, 97).
/// Returns `false` without drawing if any buffer has the wrong size.
#[must_use]
pub fn render_settings_cog_transition_into(
    launcher: &[Rgb565Pixel],
    settings: &[Rgb565Pixel],
    cog: &[Rgb565Pixel],
    t_ms: u32,
    output: &mut [Rgb565Pixel],
) -> bool {
    let frame_len = SETTINGS_COG_WIDTH * SETTINGS_COG_HEIGHT;
    if launcher.len() != frame_len
        || settings.len() != frame_len
        || output.len() != frame_len
        || cog.len() != COG_ASSET_WIDTH * COG_ASSET_HEIGHT
    {
        return false;
    }
    let t = t_ms.min(SETTINGS_COG_DURATION_MS);
    if t == 0 {
        output.copy_from_slice(launcher);
        return true;
    }
    if t == SETTINGS_COG_DURATION_MS {
        output.copy_from_slice(settings);
        return true;
    }
    const W: usize = SETTINGS_COG_WIDTH;

    // Timeline (ms): the outline zooms 0-760, the cog travels 80-840, the
    // card face fades 60-260, and list bands slide in from 560 with a 30 ms
    // stagger. The launcher remains still behind the expanding opaque card;
    // the card itself occludes the carousel instead of forcing a full-screen
    // fade every frame.
    let zoom_p = ease_in_out(window_q16(t, 0, 760));
    let cog_p = ease_in_out(window_q16(t, 80, 760));
    let z = zoom_q16(zoom_p);
    let face_alpha = 256 - alpha_of(window_q16(t, 60, 200));
    // The outline fades as it leaves the screen: 1 - 1.25 p^2.
    let outline_p = window_q16(t, 0, 760);
    let outline_alpha = alpha_of((1 << 16) - ((outline_p * outline_p) >> 16) * 5 / 4);

    // Window (the zoomed card) in Q16 screen pixels.
    let win_w = i64::from(CARD_W) * z;
    let win_h = i64::from(CARD_H) * z;
    let win_x = (i64::from(CARD_CX) << 16) - win_w / 2;
    let win_y = (i64::from(CARD_CY) << 16) - win_h / 2;
    let win_r = i64::from(CARD_RADIUS) * z;
    let stroke = ((3.0 * ((z as f64) / 65536.0).sqrt()) * 65536.0) as i64;

    // Cog: screen = origin + scale * asset, interpolated from the card crop.
    let c0 = (i64::from(CARD_W) << 16) / i64::from(RENDER_CARD_W); // Q16
    let start_x =
        (i64::from(CARD_X) << 16) + (i64::from(ASSET_CROP_X * 2 - RENDER_CARD_X_Q1) * c0) / 2;
    let start_y =
        (i64::from(CARD_Y) << 16) + (i64::from(ASSET_CROP_Y * 2 - RENDER_CARD_Y_Q1) * c0) / 2;
    let lerp = |a: i64, b: i64| a + (((b - a) * cog_p) >> 16);
    let cog_x = lerp(start_x, i64::from(COG_REST_X) << 16);
    let cog_y = lerp(start_y, i64::from(COG_REST_Y) << 16);
    let cog_s = lerp(c0, 1 << 16);
    let cog_at_rest = cog_p >= 1 << 16;
    let inv_s = (1i64 << 32) / cog_s.max(1); // Q16 reciprocal
    let inv_z = (1i64 << 32) / z.max(1); // Q16 reciprocal
    let cog_x0 = (cog_x >> 16).max(0) as usize;
    let cog_x1 = (((cog_x + COG_ASSET_WIDTH as i64 * cog_s) >> 16) + 1).clamp(0, W as i64) as usize;
    let cog_y0 = (cog_y >> 16).max(0) as usize;
    let cog_y1 = (((cog_y + COG_ASSET_HEIGHT as i64 * cog_s) >> 16) + 1).max(0) as usize;

    // Begin with the still launcher. Header and rule pixels come from the
    // destination because they are identical in production; keeping this
    // explicit also preserves the pure renderer's endpoint contract.
    output.copy_from_slice(launcher);
    output[..CONTENT_TOP * W].copy_from_slice(&settings[..CONTENT_TOP * W]);
    output[CONTENT_BOTTOM * W..FOOTER_TOP * W]
        .copy_from_slice(&settings[CONTENT_BOTTOM * W..FOOTER_TOP * W]);
    if t >= 860 {
        output[FOOTER_TOP * W..].copy_from_slice(&settings[FOOTER_TOP * W..]);
    }

    for y in CONTENT_TOP..CONTENT_BOTTOM {
        let row = y * W;
        let out = &mut output[row..row + W];
        let span = rounded_span(y as i32, win_x, win_y, win_w, win_h, win_r);
        let (in0, in1) = span.unwrap_or((W, W));

        // Inside the window: black, then the cog, then the fading card face.
        if span.is_some() {
            out[in0..in1].fill(Rgb565Pixel(0));
        }
        if span.is_some() && y >= cog_y0 && y < cog_y1 {
            let (x0, x1) = (cog_x0.max(in0), cog_x1.min(in1));
            if cog_at_rest {
                let v = y as i32 - COG_REST_Y;
                if (0..COG_ASSET_HEIGHT as i32).contains(&v) {
                    let cog_row =
                        &cog[v as usize * COG_ASSET_WIDTH..(v as usize + 1) * COG_ASSET_WIDTH];
                    for (x, pixel) in out.iter_mut().enumerate().take(x1).skip(x0) {
                        let u = x as i32 - COG_REST_X;
                        if (0..COG_ASSET_WIDTH as i32).contains(&u) {
                            *pixel = cog_row[u as usize];
                        }
                    }
                }
            } else {
                let v = (((((y as i64) << 16) + (1 << 15) - cog_y) * inv_s) >> 16) - (1 << 15);
                let mut u = (((((x0 as i64) << 16) + (1 << 15) - cog_x) * inv_s) >> 16) - (1 << 15);
                for pixel in out.iter_mut().take(x1).skip(x0) {
                    *pixel = Rgb565Pixel(sample_cog(cog, u, v));
                    u += inv_s;
                }
            }
        }
        if span.is_some() && face_alpha > 0 {
            // The launcher's own card pixels, scaled with the window.
            let sy = CARD_CY as i64
                + ((((((y as i64) << 16) + (1 << 15)) - (i64::from(CARD_CY) << 16)) * inv_z) >> 32);
            if (CARD_Y as i64..(CARD_Y + CARD_H) as i64).contains(&sy) {
                let face_row = sy as usize * W;
                let mut sx_q16 = (i64::from(CARD_CX) << 16)
                    + (((((in0 as i64) << 16) + (1 << 15) - (i64::from(CARD_CX) << 16)) * inv_z)
                        >> 16);
                for pixel in out.iter_mut().take(in1).skip(in0) {
                    let sx = sx_q16 >> 16;
                    if (CARD_X as i64..(CARD_X + CARD_W) as i64).contains(&sx) {
                        *pixel = Rgb565Pixel(blend(
                            pixel.0,
                            launcher[face_row + sx as usize].0,
                            face_alpha,
                        ));
                    }
                    sx_q16 += inv_z;
                }
            }
        }

        // Outline: the stroked expansion minus the window (whole row at caps).
        if outline_alpha > 0
            && let Some((o0, o1)) = rounded_span(
                y as i32,
                win_x - stroke,
                win_y - stroke,
                win_w + 2 * stroke,
                win_h + 2 * stroke,
                win_r + stroke,
            )
        {
            let (i0, i1) = span.map_or((o1, o1), |(i0, i1)| (i0.clamp(o0, o1), i1.clamp(o0, o1)));
            for x in (o0..i0).chain(i1..o1) {
                out[x] = Rgb565Pixel(blend(out[x].0, OUTLINE, outline_alpha));
            }
        }
    }

    // Settings list bands: Slint's pixels, whole-pixel slide, alpha fade.
    for (index, &(top, bottom)) in LIST_BANDS.iter().enumerate() {
        let at = BAND_START_MS + index as u32 * BAND_STAGGER_MS;
        let k = ease_out(window_q16(t, at, BAND_DURATION_MS));
        let alpha = alpha_of(k);
        if alpha == 0 {
            continue;
        }
        let offset = ((i64::from(BAND_TRAVEL) * ((1 << 16) - k) + (1 << 15)) >> 16) as usize;
        for y in top..bottom {
            let row = y * W;
            if alpha >= 256 {
                let len = (LIST_RIGHT - LIST_LEFT).min(W.saturating_sub(LIST_LEFT + offset));
                if len > 0 {
                    output[row + LIST_LEFT + offset..row + LIST_LEFT + offset + len]
                        .copy_from_slice(&settings[row + LIST_LEFT..row + LIST_LEFT + len]);
                }
                continue;
            }
            for x in LIST_LEFT..LIST_RIGHT {
                let destination = x + offset;
                if destination >= W {
                    break;
                }
                output[row + destination] = Rgb565Pixel(blend(
                    output[row + destination].0,
                    settings[row + x].0,
                    alpha,
                ));
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(value: u16) -> Vec<Rgb565Pixel> {
        vec![Rgb565Pixel(value); SETTINGS_COG_WIDTH * SETTINGS_COG_HEIGHT]
    }

    fn patterned_cog() -> Vec<Rgb565Pixel> {
        (0..COG_ASSET_WIDTH * COG_ASSET_HEIGHT)
            .map(|i| Rgb565Pixel(((i * 2654435761) >> 7) as u16 | 0x0821))
            .collect()
    }

    #[test]
    fn endpoints_are_exactly_the_two_screens() {
        let (launcher, settings, cog) = (frame(0x1234), frame(0x4321), patterned_cog());
        let mut output = frame(0);
        assert!(render_settings_cog_transition_into(
            &launcher,
            &settings,
            &cog,
            0,
            &mut output
        ));
        assert_eq!(output, launcher);
        assert!(render_settings_cog_transition_into(
            &launcher,
            &settings,
            &cog,
            SETTINGS_COG_DURATION_MS,
            &mut output,
        ));
        assert_eq!(output, settings);
    }

    #[test]
    fn cog_lands_on_the_settings_pixels_before_the_end() {
        // A Settings frame that is black except the resting cog.
        let mut settings = frame(0);
        let cog = patterned_cog();
        for v in 0..COG_ASSET_HEIGHT {
            for u in 0..COG_ASSET_WIDTH {
                let (x, y) = (u as i32 + COG_REST_X, v as i32 + COG_REST_Y);
                if (0..SETTINGS_COG_WIDTH as i32).contains(&x)
                    && (CONTENT_TOP as i32..CONTENT_BOTTOM as i32).contains(&y)
                {
                    settings[y as usize * SETTINGS_COG_WIDTH + x as usize] =
                        cog[v * COG_ASSET_WIDTH + u];
                }
            }
        }
        let launcher = frame(0x7bef);
        let mut output = frame(0);
        // After the cog settles (840 ms) and the launcher has faded, the
        // content matches the destination everywhere outside the list.
        assert!(render_settings_cog_transition_into(
            &launcher,
            &settings,
            &cog,
            900,
            &mut output
        ));
        for y in CONTENT_TOP..CONTENT_BOTTOM {
            for x in 0..LIST_LEFT {
                let index = y * SETTINGS_COG_WIDTH + x;
                assert_eq!(output[index], settings[index], "pixel ({x}, {y})");
            }
        }
    }

    #[test]
    fn launcher_stays_unchanged_outside_the_expanding_card() {
        let launcher = frame(0x7bef);
        let settings = frame(0);
        let cog = vec![Rgb565Pixel(0); COG_ASSET_WIDTH * COG_ASSET_HEIGHT];
        let mut output = frame(0);

        assert!(render_settings_cog_transition_into(
            &launcher,
            &settings,
            &cog,
            240,
            &mut output,
        ));
        assert_eq!(output[200 * SETTINGS_COG_WIDTH + 100], Rgb565Pixel(0x7bef));
        assert_eq!(output[520 * SETTINGS_COG_WIDTH + 100], Rgb565Pixel(0x7bef));
    }

    #[test]
    fn zoom_curve_is_monotonic_and_exact_at_the_ends() {
        assert_eq!(zoom_q16(0), 1 << 16);
        assert_eq!(zoom_q16(1 << 16), ZOOM_MAX_Q16);
        let mut previous = 0;
        for step in 0..=256 {
            let value = zoom_q16(step << 8);
            assert!(value >= previous);
            previous = value;
        }
    }

    #[test]
    fn list_bands_slide_in_whole_pixels() {
        let launcher = frame(0);
        let mut settings = frame(0);
        // Mark one pixel at the left edge of the first row band.
        settings[130 * SETTINGS_COG_WIDTH + LIST_LEFT] = Rgb565Pixel(0xffff);
        let cog = vec![Rgb565Pixel(0); COG_ASSET_WIDTH * COG_ASSET_HEIGHT];
        let mut output = frame(0);
        assert!(render_settings_cog_transition_into(
            &launcher,
            &settings,
            &cog,
            700,
            &mut output
        ));
        let row = &output[130 * SETTINGS_COG_WIDTH..131 * SETTINGS_COG_WIDTH];
        let lit: Vec<_> = (0..SETTINGS_COG_WIDTH).filter(|&x| row[x].0 != 0).collect();
        assert_eq!(lit.len(), 1, "one whole-pixel copy, no smear: {lit:?}");
        assert!(lit[0] > LIST_LEFT && lit[0] <= LIST_LEFT + BAND_TRAVEL as usize);
    }

    #[test]
    fn final_list_band_is_settled_before_the_exact_endpoint() {
        let launcher = frame(0);
        let mut settings = frame(0);
        settings[390 * SETTINGS_COG_WIDTH + LIST_LEFT] = Rgb565Pixel(0xffff);
        let cog = vec![Rgb565Pixel(0); COG_ASSET_WIDTH * COG_ASSET_HEIGHT];
        let mut output = frame(0);

        assert!(render_settings_cog_transition_into(
            &launcher,
            &settings,
            &cog,
            SETTINGS_COG_DURATION_MS - 1,
            &mut output,
        ));
        assert_eq!(output, settings);
    }
}
