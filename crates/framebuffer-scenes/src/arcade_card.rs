// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Home <-> Arcade launcher-card reveal for HDMI and CRT rasters.

use crate::Rgb565Pixel;

#[path = "cabinet_scanline.rs"]
mod scanline;
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
const HDMI_CONTENT_TOP: usize = 77;
const HDMI_CONTENT_BOTTOM: usize = 500;
const HDMI_SCREEN: NavigationTransitionRect = NavigationTransitionRect {
    x: 572,
    y: 96,
    width: 320,
    height: 320,
};
const LIST_LEFT: usize = 26;
const LIST_RIGHT: usize = 488;
const LIST_BANDS: [(usize, usize); 11] = [
    (88, 124),
    (124, 160),
    (160, 196),
    (196, 232),
    (232, 268),
    (268, 304),
    (304, 340),
    (340, 376),
    (376, 412),
    (412, 448),
    (448, 484),
];
const RED: u16 = rgb565(231, 105, 90);

const fn rgb565(r: u16, g: u16, b: u16) -> u16 {
    ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
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
    render_with_texture(
        width,
        height,
        launcher,
        arcade,
        cabinet,
        source_card,
        t_ms,
        output,
        None,
        (0, height),
    )
}

/// Filtered asset experiment sharing the production timeline and composition.
#[allow(clippy::too_many_arguments)]
pub fn render_arcade_card_filtered_into(
    width: usize,
    height: usize,
    launcher: &[Rgb565Pixel],
    arcade: &[Rgb565Pixel],
    texture: &CabinetTexture,
    source_card: NavigationTransitionRect,
    t_ms: u32,
    output: &mut [Rgb565Pixel],
) -> bool {
    render_with_texture(
        width,
        height,
        launcher,
        arcade,
        &texture.reference,
        source_card,
        t_ms,
        output,
        Some(texture),
        (0, height),
    )
}
/// Render disjoint HDMI row bands for the live two-worker experiment. Sources
/// remain immutable; each worker owns its complete output and row scratch.
#[allow(clippy::too_many_arguments)]
pub fn render_arcade_card_filtered_band_into(
    launcher: &[Rgb565Pixel],
    arcade: &[Rgb565Pixel],
    texture: &CabinetTexture,
    t_ms: u32,
    output: &mut [Rgb565Pixel],
    rows: (usize, usize),
) -> bool {
    if rows.0 > rows.1 || rows.1 > 540 {
        return false;
    }
    render_with_texture(
        960,
        540,
        launcher,
        arcade,
        &texture.reference,
        HDMI_CARD,
        t_ms,
        output,
        Some(texture),
        rows,
    )
}
#[allow(clippy::too_many_arguments)]
fn render_with_texture(
    width: usize,
    height: usize,
    launcher: &[Rgb565Pixel],
    arcade: &[Rgb565Pixel],
    cabinet: &[Rgb565Pixel],
    source_card: NavigationTransitionRect,
    t_ms: u32,
    output: &mut [Rgb565Pixel],
    filtered: Option<&CabinetTexture>,
    rows: (usize, usize),
) -> bool {
    let len = width.saturating_mul(height);
    if width == 0
        || height == 0
        || len > 960 * 576
        || launcher.len() != len
        || arcade.len() != len
        || output.len() != len
        || cabinet.len() != CABINET_WIDTH * CABINET_HEIGHT
    {
        return false;
    }
    let t = t_ms.min(ARCADE_CARD_DURATION_MS);
    if t == 0 {
        output[rows.0 * width..rows.1 * width]
            .copy_from_slice(&launcher[rows.0 * width..rows.1 * width]);
        return true;
    }
    if t == ARCADE_CARD_DURATION_MS {
        output[rows.0 * width..rows.1 * width]
            .copy_from_slice(&arcade[rows.0 * width..rows.1 * width]);
        return true;
    }
    if (width, height) == (960, 540) {
        render_hdmi(launcher, arcade, cabinet, t, output, filtered, rows);
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
    filtered: Option<&CabinetTexture>,
    rows: (usize, usize),
) {
    const W: usize = 960;
    const H: usize = 540;
    let fast = filtered.is_some_and(|texture| texture.scanlines.is_some());
    let launcher_alpha = 256_u32.saturating_sub(alpha_of(ease_out(window_q16(t, 120, 360))));
    let chrome_alpha = alpha_of(ease_out(window_q16(t, 220, 300)));
    if !fast || !scanline::base(launcher, arcade, output, launcher_alpha, chrome_alpha, rows) {
        for (index, pixel) in output
            .iter_mut()
            .enumerate()
            .take(rows.1 * W)
            .skip(rows.0 * W)
        {
            let x = index % W;
            let y = index / W;
            if y < HDMI_CONTENT_TOP {
                *pixel = arcade[index];
                continue;
            }
            let destination_is_subject = (HDMI_CABINET_X as usize..W).contains(&x)
                && (HDMI_CONTENT_TOP..HDMI_CONTENT_BOTTOM).contains(&y)
                || (LIST_LEFT..LIST_RIGHT).contains(&x) && (88..484).contains(&y);
            let chrome = if destination_is_subject {
                0
            } else {
                arcade[index].0
            };
            let base = blend(0, launcher[index].0, launcher_alpha);
            *pixel = Rgb565Pixel(blend(base, chrome, chrome_alpha));
        }
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
    let y0 = ((cabinet_y >> 16).clamp(HDMI_CONTENT_TOP as i64, HDMI_CONTENT_BOTTOM as i64)
        as usize)
        .max(rows.0);
    let y1 = (((cabinet_y + CABINET_HEIGHT as i64 * scale) >> 16) + 1)
        .clamp(HDMI_CONTENT_TOP as i64, HDMI_CONTENT_BOTTOM as i64) as usize;
    let y1 = y1.min(rows.1);
    let screen_alpha = alpha_of(ease_out(window_q16(t, 760, 160)));
    if fast {
        scanline::render(
            filtered.unwrap(),
            output,
            (x0, x1, y0, y1),
            (cabinet_x, cabinet_y, inverse),
            (((screen_alpha + 4) >> 3) == 32).then_some((
                HDMI_SCREEN.x as usize,
                HDMI_SCREEN.right() as usize,
                HDMI_SCREEN.y as usize,
                HDMI_SCREEN.bottom() as usize,
            )),
            ((launcher_alpha + 4) >> 3) == 0
                && x0 >= HDMI_CABINET_X as usize
                && y0 >= HDMI_CONTENT_TOP
                && y1 <= HDMI_CONTENT_BOTTOM,
        );
    } else {
        for y in y0..y1 {
            let source_y = (((((y as i64) << 16) + (1 << 15) - cabinet_y) * inverse) >> 16) >> 16;
            if filtered.is_none() && !(0..CABINET_HEIGHT as i64).contains(&source_y) {
                continue;
            }
            for x in x0..x1 {
                let source_x =
                    (((((x as i64) << 16) + (1 << 15) - cabinet_x) * inverse) >> 16) >> 16;
                if filtered.is_none() && !(0..CABINET_WIDTH as i64).contains(&source_x) {
                    continue;
                }
                if let Some(texture) = filtered {
                    let sx = ((((x as i64) << 16) + (1 << 15) - cabinet_x) * inverse) >> 16;
                    let sy = ((((y as i64) << 16) + (1 << 15) - cabinet_y) * inverse) >> 16;
                    let sample = texture.sample(sx - (1 << 15), sy - (1 << 15), inverse as u32);
                    output[y * W + x] =
                        crate::launcher_texture::over_dithered(sample, output[y * W + x], x, y);
                    continue;
                }
                let sampled = cabinet[source_y as usize * CABINET_WIDTH + source_x as usize].0;
                if sampled != 0 {
                    output[y * W + x] = Rgb565Pixel(sampled);
                }
            }
        }
    }

    draw_outline(
        W,
        H,
        HDMI_CARD,
        t,
        HDMI_CONTENT_TOP.max(rows.0),
        HDMI_CONTENT_BOTTOM.min(rows.1),
        output,
    );

    copy_rect_alpha(W, arcade, output, HDMI_SCREEN, screen_alpha, 0, fast, rows);
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
            fast,
            rows,
        );
    }
    let footer_alpha = alpha_of(ease_out(window_q16(t, 640, 220)));
    copy_band_alpha(W, arcade, output, 0, W, 500, H, footer_alpha, 0, fast, rows);
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
    draw_outline(width, height, card, t, 0, height, output);
}

fn draw_outline(
    width: usize,
    height: usize,
    card: NavigationTransitionRect,
    t: u32,
    clip_top: usize,
    clip_bottom: usize,
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
    for row in clip_top.min(height)..clip_bottom.min(height) {
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

#[allow(clippy::too_many_arguments)]
fn copy_rect_alpha(
    width: usize,
    source: &[Rgb565Pixel],
    destination: &mut [Rgb565Pixel],
    rect: NavigationTransitionRect,
    alpha: u32,
    offset: usize,
    fast: bool,
    rows: (usize, usize),
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
        fast,
        rows,
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
    fast: bool,
    rows: (usize, usize),
) {
    if alpha == 0 {
        return;
    }
    for y in top.max(rows.0)..bottom.min(rows.1) {
        let n = right.min(width.saturating_sub(offset)).saturating_sub(left);
        if fast
            && scanline::over(
                &mut destination[y * width + left + offset..y * width + left + offset + n],
                &source[y * width + left..y * width + left + n],
                alpha,
            )
        {
            continue;
        }
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
    fn filtered_bands_preserve_other_rows_and_reject_invalid_ranges() {
        let texture = CabinetTexture::from_rgb888(&vec![64; CABINET_WIDTH * CABINET_HEIGHT * 3])
            .unwrap()
            .with_scanlines();
        let home = vec![Rgb565Pixel(0x1234); 960 * 540];
        let arcade = vec![Rgb565Pixel(0xabcd); 960 * 540];
        let sentinel = Rgb565Pixel(0xbeef);
        let mut expected = vec![sentinel; 960 * 540];
        let mut tile = expected.clone();
        for t in [0, 200, 500, 800, 1000] {
            assert!(render_arcade_card_filtered_into(
                960,
                540,
                &home,
                &arcade,
                &texture,
                HDMI_CARD,
                t,
                &mut expected
            ));
            tile.fill(sentinel);
            assert!(render_arcade_card_filtered_band_into(
                &home,
                &arcade,
                &texture,
                t,
                &mut tile,
                (289, 540)
            ));
            assert!(tile[..289 * 960].iter().all(|&p| p == sentinel));
            assert_eq!(tile[289 * 960..], expected[289 * 960..]);
        }
        let before = tile.clone();
        assert!(!render_arcade_card_filtered_band_into(
            &home,
            &arcade,
            &texture,
            500,
            &mut tile,
            (400, 399)
        ));
        assert!(!render_arcade_card_filtered_band_into(
            &home,
            &arcade,
            &texture,
            500,
            &mut tile,
            (540, 541)
        ));
        assert_eq!(tile, before);
    }

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

    #[test]
    fn hdmi_cabinet_animation_never_crosses_page_chrome() {
        const W: usize = 960;
        const H: usize = 540;
        let launcher = vec![Rgb565Pixel(0); W * H];
        let arcade = vec![Rgb565Pixel(0); W * H];
        let cabinet = vec![Rgb565Pixel(0xffff); CABINET_WIDTH * CABINET_HEIGHT];
        let mut output = vec![Rgb565Pixel(0); W * H];

        for t_ms in [1, 80, 200, 400, 600, 839, 900, 999] {
            assert!(render_arcade_card_transition_into(
                W,
                H,
                &launcher,
                &arcade,
                &cabinet,
                HDMI_CARD,
                t_ms,
                &mut output,
            ));
            assert!(
                output[..HDMI_CONTENT_TOP * W]
                    .iter()
                    .all(|pixel| pixel.0 == 0)
            );
            assert!(
                output[HDMI_CONTENT_BOTTOM * W..]
                    .iter()
                    .all(|pixel| pixel.0 == 0)
            );
        }
    }

    #[test]
    fn hdmi_chrome_remains_visible_while_cabinet_finishes() {
        const W: usize = 960;
        const H: usize = 540;
        const CHROME: Rgb565Pixel = Rgb565Pixel(0x1234);
        let launcher = vec![Rgb565Pixel(0); W * H];
        let arcade = vec![CHROME; W * H];
        let cabinet = vec![Rgb565Pixel(0xffff); CABINET_WIDTH * CABINET_HEIGHT];
        let mut output = vec![Rgb565Pixel(0); W * H];

        assert!(render_arcade_card_transition_into(
            W,
            H,
            &launcher,
            &arcade,
            &cabinet,
            HDMI_CARD,
            900,
            &mut output,
        ));
        assert_eq!(output[50 * W + 600], CHROME);
        assert_eq!(output[520 * W + 600], CHROME);
    }

    #[test]
    fn hdmi_header_is_pixel_stable_for_every_transition_phase() {
        const W: usize = 960;
        const H: usize = 540;
        const HEADER: Rgb565Pixel = Rgb565Pixel(0x1234);
        let launcher = vec![Rgb565Pixel(0xabcd); W * H];
        let arcade = vec![HEADER; W * H];
        let cabinet = vec![Rgb565Pixel(0xffff); CABINET_WIDTH * CABINET_HEIGHT];
        let mut output = vec![Rgb565Pixel(0); W * H];

        for t_ms in [1, 80, 200, 400, 600, 839, 900, 999] {
            assert!(render_arcade_card_transition_into(
                W,
                H,
                &launcher,
                &arcade,
                &cabinet,
                HDMI_CARD,
                t_ms,
                &mut output,
            ));
            assert_eq!(
                &output[..HDMI_CONTENT_TOP * W],
                &arcade[..HDMI_CONTENT_TOP * W]
            );
        }
    }

    #[test]
    fn hdmi_list_transition_reveals_all_eleven_contiguous_rows() {
        assert_eq!(LIST_BANDS.len(), 11);
        assert_eq!(LIST_BANDS.first(), Some(&(88, 124)));
        assert_eq!(LIST_BANDS.last(), Some(&(448, 484)));
        assert!(LIST_BANDS.windows(2).all(|bands| bands[0].1 == bands[1].0));
    }
}

/// Preparation-only 2D minification pyramid. RGB8 and coverage survive until
/// destination-space composition. Geometry remains the production 483x519.
#[derive(Clone)]
pub struct CabinetTexture {
    reference: Vec<Rgb565Pixel>,
    levels: Vec<CabinetLevel>,
    scanlines: Option<Box<std::cell::RefCell<scanline::Scanlines>>>,
}
#[derive(Clone)]
struct CabinetLevel {
    pixels: Vec<u32>,
    width: usize,
    height: usize,
}
impl CabinetTexture {
    pub fn from_rgb565(pixels: &[Rgb565Pixel]) -> Result<Self, String> {
        if pixels.len() != CABINET_WIDTH * CABINET_HEIGHT {
            return Err("invalid cabinet geometry".into());
        }
        let rgb: Vec<_> = pixels
            .iter()
            .flat_map(|p| {
                let r = p.0 >> 11;
                let g = (p.0 >> 5) & 63;
                let b = p.0 & 31;
                [
                    ((r << 3) | (r >> 2)) as u8,
                    ((g << 2) | (g >> 4)) as u8,
                    ((b << 3) | (b >> 2)) as u8,
                ]
            })
            .collect();
        Self::from_rgb888(&rgb)
    }
    pub fn from_rgb888(rgb: &[u8]) -> Result<Self, String> {
        if rgb.len() != CABINET_WIDTH * CABINET_HEIGHT * 3 {
            return Err("invalid RGB888 cabinet geometry".into());
        }
        let reference = rgb
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| Rgb565Pixel(rgb565(u16::from(p[0]), u16::from(p[1]), u16::from(p[2]))))
            .collect();
        let pixels = rgb
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| {
                if *p == [0, 0, 0] {
                    0
                } else {
                    u32::from_le_bytes([p[0], p[1], p[2], 255])
                }
            })
            .collect();
        let mut levels = vec![CabinetLevel {
            pixels,
            width: CABINET_WIDTH,
            height: CABINET_HEIGHT,
        }];
        while levels.last().unwrap().width > 1 || levels.last().unwrap().height > 1 {
            let old = levels.last().unwrap();
            let width = old.width.div_ceil(2);
            let height = old.height.div_ceil(2);
            let mut pixels = Vec::with_capacity(width * height);
            for y in 0..height {
                for x in 0..width {
                    let at = |dx: usize, dy: usize| {
                        old.pixels[(y * 2 + dy).min(old.height - 1) * old.width
                            + (x * 2 + dx).min(old.width - 1)]
                    };
                    pixels.push(crate::launcher_texture::mix(
                        crate::launcher_texture::mix(at(0, 0), at(1, 0), 128),
                        crate::launcher_texture::mix(at(0, 1), at(1, 1), 128),
                        128,
                    ));
                }
            }
            levels.push(CabinetLevel {
                pixels,
                width,
                height,
            });
        }
        Ok(Self {
            reference,
            levels,
            scanlines: None,
        })
    }
    /// Opt into bounded row scratch and the live scanline scaler experiment.
    pub fn with_scanlines(mut self) -> Self {
        self.scanlines = Some(Box::new(
            std::cell::RefCell::new(scanline::Scanlines::new()),
        ));
        self
    }
    /// Prepare the resting cabinet with the same final quantisation as the
    /// filtered reveal. Text, game pixels and chrome remain on their native grid.
    pub fn prepare_destination(&self, destination: &mut [Rgb565Pixel]) -> bool {
        if destination.len() != 960 * 540 {
            return false;
        }
        for y in HDMI_CONTENT_TOP..HDMI_CONTENT_BOTTOM {
            for x in HDMI_CABINET_X as usize..960 {
                if (HDMI_SCREEN.x as usize..(HDMI_SCREEN.x + HDMI_SCREEN.width) as usize)
                    .contains(&x)
                    && (HDMI_SCREEN.y as usize..(HDMI_SCREEN.y + HDMI_SCREEN.height) as usize)
                        .contains(&y)
                {
                    continue;
                }
                let sx = x - HDMI_CABINET_X as usize;
                let sy = y - HDMI_CABINET_Y as usize;
                let p = self.levels[0].pixels[sy * CABINET_WIDTH + sx];
                destination[y * 960 + x] =
                    crate::launcher_texture::over_dithered(p, Rgb565Pixel(0), x, y);
            }
        }
        true
    }
    pub fn storage_bytes(&self) -> usize {
        self.scanlines
            .as_ref()
            .map_or(0, |_| std::mem::size_of::<scanline::Scanlines>())
            + self.reference.capacity() * 2
            + self
                .levels
                .iter()
                .map(|l| l.pixels.capacity() * 4)
                .sum::<usize>()
    }
    fn sample(&self, x: i64, y: i64, footprint: u32) -> u32 {
        let level = ((31 - footprint.max(65536).leading_zeros()).saturating_sub(16) as usize)
            .min(self.levels.len() - 1);
        let sample = |index: usize| {
            let l = &self.levels[index];
            let sx = ((x + (1 << 15)) >> index) - (1 << 15);
            let sy = ((y + (1 << 15)) >> index) - (1 << 15);
            let ix = sx.div_euclid(65536);
            let iy = sy.div_euclid(65536);
            let at = |dx: i64, dy: i64| {
                if ix + dx < 0
                    || iy + dy < 0
                    || ix + dx >= l.width as i64
                    || iy + dy >= l.height as i64
                {
                    0
                } else {
                    l.pixels[(iy + dy) as usize * l.width + (ix + dx) as usize]
                }
            };
            crate::launcher_texture::mix(
                crate::launcher_texture::mix(at(0, 0), at(1, 0), ((sx & 65535) >> 8) as u32),
                crate::launcher_texture::mix(at(0, 1), at(1, 1), ((sx & 65535) >> 8) as u32),
                ((sy & 65535) >> 8) as u32,
            )
        };
        let a = sample(level);
        if level + 1 == self.levels.len() {
            a
        } else {
            let weight = ((footprint >> level).saturating_sub(65536) >> 8).min(256);
            if weight == 0 {
                a
            } else {
                crate::launcher_texture::mix(a, sample(level + 1), weight)
            }
        }
    }
}
