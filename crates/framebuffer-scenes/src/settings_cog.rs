// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Home -> Settings card zoom for the HDMI and native CRT card launchers.
//!
//! The selected Settings card's outline zooms past the screen edges while the
//! full Settings cog, rendered from the same Blender camera at twice the card
//! framing, grows from its card-sized crop to its 1:1 resting position. The
//! Settings rows are then copied from Slint's own destination raster and slid
//! in one band at a time at whole-pixel offsets, so every glyph is Slint's.
//!
//! The renderer is a pure function of time: `t = 0` is exactly the launcher
//! frame and `t = SETTINGS_COG_DURATION_MS` is exactly the Settings frame.
//! Reverse playback evaluates the same timeline backwards. RGB888 source colour
//! and coverage survive filtered scaling until final destination-space dithering.

use crate::Rgb565Pixel;
#[cfg(test)]
use crate::card_page::lerp_rgb565;
use crate::card_page::{
    alpha_of, blend, blend_row, ease_in_out, ease_out, rounded_span, window_q16,
};

pub const SETTINGS_COG_WIDTH: usize = 960;
pub const SETTINGS_COG_HEIGHT: usize = 540;
pub const SETTINGS_COG_DURATION_MS: u32 = 1_000;

/// The backdrop source: 412x374 RGB888, see apps/mister/assets/ui/settings.
pub const COG_ASSET_WIDTH: usize = 412;
pub const COG_ASSET_HEIGHT: usize = 374;

// Selected (centre) card of the landscape launcher: slot centre 610, half
// width 90, vertical centre 284 (crates/framebuffer-scenes/src/launcher.rs).
const CARD_X: i32 = 520;
const CARD_Y: i32 = 158;
const CARD_W: i32 = 180;
const CARD_H: i32 = 252;
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

/// Shared CRT Settings-family geometry in logical framebuffer pixels.
///
/// The card zoom, segmented page transition and host-side viewport assets use
/// this one calculation. `safe_x`/`safe_y` preserve the text overscan inset;
/// callers rendering the full physical raster pass zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CrtSettingsGeometry {
    width: usize,
    height: usize,
    sx: usize,
    sy: usize,
    margin_x: usize,
    margin_y: usize,
    header_bottom: usize,
    footer_rule: usize,
}

impl CrtSettingsGeometry {
    #[must_use]
    pub fn for_dimensions(width: usize, height: usize) -> Option<Self> {
        Self::for_viewport(width, height, 0, 0)
    }

    #[must_use]
    pub fn for_viewport(width: usize, height: usize, safe_x: usize, safe_y: usize) -> Option<Self> {
        if width.max(height) != 640 || !matches!(width.min(height), 240 | 288 | 480 | 512 | 576) {
            return None;
        }
        let narrow = width.min(height);
        let (sx, sy) = if narrow <= 288 {
            if width > height { (2, 1) } else { (1, 2) }
        } else {
            (2, 2)
        };
        let margin_x = (width * 6 / 100).max(8 * sx).max(safe_x);
        let margin_y = (height * 5 / 100).max(6 * sy).max(safe_y);
        let header_bottom = margin_y + 18 * sy;
        let footer_rule = height.saturating_sub(margin_y + 20 * sy);
        Some(Self {
            width,
            height,
            sx,
            sy,
            margin_x,
            margin_y,
            header_bottom,
            footer_rule,
        })
    }

    pub const fn width(self) -> usize {
        self.width
    }

    pub const fn height(self) -> usize {
        self.height
    }

    pub const fn scale_x(self) -> usize {
        self.sx
    }

    pub const fn scale_y(self) -> usize {
        self.sy
    }

    pub const fn margin_x(self) -> usize {
        self.margin_x
    }

    pub const fn margin_y(self) -> usize {
        self.margin_y
    }

    pub const fn header_bottom(self) -> usize {
        self.header_bottom
    }

    pub const fn footer_rule(self) -> usize {
        self.footer_rule
    }

    pub const fn list_top(self) -> usize {
        self.header_bottom + 26 * self.sy
    }

    pub const fn row_height(self) -> usize {
        16 * self.sy
    }

    pub const fn group_gap(self) -> usize {
        6 * self.sy
    }

    pub const fn axis_scale(self, horizontal: bool) -> usize {
        if horizontal { self.sx } else { self.sy }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SettingsCogLayout {
    width: usize,
    height: usize,
    card_x: i32,
    card_y: i32,
    card_w: i32,
    card_h: i32,
    card_radius: i32,
    cog_rest_x: i32,
    cog_rest_y: i32,
    cog_rest_w: i32,
    cog_rest_h: i32,
    cog_rest_alpha: u32,
    content_top: usize,
    content_bottom: usize,
    footer_top: usize,
    list_left: usize,
    list_right: usize,
    list_bands: [(usize, usize); 9],
    list_band_count: usize,
    band_travel: i32,
    zoom_max_q16: i64,
}

impl SettingsCogLayout {
    const fn hdmi() -> Self {
        Self {
            width: SETTINGS_COG_WIDTH,
            height: SETTINGS_COG_HEIGHT,
            card_x: CARD_X,
            card_y: CARD_Y,
            card_w: CARD_W,
            card_h: CARD_H,
            card_radius: CARD_RADIUS,
            cog_rest_x: COG_REST_X,
            cog_rest_y: COG_REST_Y,
            cog_rest_w: COG_ASSET_WIDTH as i32,
            cog_rest_h: COG_ASSET_HEIGHT as i32,
            cog_rest_alpha: 128,
            content_top: CONTENT_TOP,
            content_bottom: CONTENT_BOTTOM,
            footer_top: FOOTER_TOP,
            list_left: LIST_LEFT,
            list_right: LIST_RIGHT,
            list_bands: LIST_BANDS,
            list_band_count: LIST_BANDS.len(),
            band_travel: BAND_TRAVEL,
            zoom_max_q16: ZOOM_MAX_Q16,
        }
    }

    fn for_dimensions(width: usize, height: usize) -> Option<Self> {
        if (width, height) == (SETTINGS_COG_WIDTH, SETTINGS_COG_HEIGHT) {
            return Some(Self::hdmi());
        }
        let geometry = CrtSettingsGeometry::for_dimensions(width, height)?;
        let sx = geometry.scale_x();
        let sy = geometry.scale_y();
        let (aspect_y, aspect_x) = match (width, height) {
            (640, 288) => (3, 5),
            (288, 640) => (5, 3),
            _ => (sy, sx),
        };
        let margin_x = geometry.margin_x();
        let margin_y = geometry.margin_y();
        let top = margin_y + 36 * sy;
        let bottom = height.saturating_sub(margin_y + 34 * sy);
        let available_h = bottom.saturating_sub(top);
        let available_w = width.saturating_sub(2 * margin_x);
        let card_w =
            ((available_w * 34 / 100).min(available_h * 5 * aspect_x / (9 * aspect_y)) / 2 * 2)
                .max(72 * sx);
        let card_h = (card_w * 7 * aspect_y / (5 * aspect_x) / 2 * 2).max(2);
        let centre_y = top + available_h.saturating_sub(card_h + card_h / 5) / 2 + card_h / 2;
        let card_x = width.saturating_sub(card_w) / 2;
        let card_y = centre_y.saturating_sub(card_h / 2);
        let header_bottom = geometry.header_bottom();
        let footer_rule = geometry.footer_rule();
        let list_top = geometry.list_top();
        let row_h = geometry.row_height();
        let gap = geometry.group_gap();
        let starts = [
            list_top,
            list_top + row_h,
            list_top + 2 * row_h,
            list_top + 3 * row_h + gap,
            list_top + 4 * row_h + gap,
            list_top + 5 * row_h + 2 * gap,
            list_top + 6 * row_h + 2 * gap,
            list_top + 7 * row_h + 2 * gap,
        ];
        let mut bands = [(0, 0); 9];
        let mut index = 0;
        while index < starts.len() {
            let end = if index + 1 < starts.len() {
                starts[index + 1]
            } else {
                starts[index] + row_h
            };
            bands[index] = (starts[index], end.min(footer_rule));
            index += 1;
        }
        let cog_rest_w = 150 * sx;
        let cog_rest_h = 150 * sy;
        Some(Self {
            width,
            height,
            card_x: card_x as i32,
            card_y: card_y as i32,
            card_w: card_w as i32,
            card_h: card_h as i32,
            card_radius: 2 * sx.min(sy) as i32,
            cog_rest_x: (width - cog_rest_w + 6 * sx) as i32,
            cog_rest_y: (header_bottom + 20 * sy) as i32,
            cog_rest_w: cog_rest_w as i32,
            cog_rest_h: cog_rest_h as i32,
            cog_rest_alpha: 77,
            content_top: header_bottom + 1,
            content_bottom: footer_rule,
            footer_top: footer_rule + 1,
            list_left: margin_x,
            list_right: width - margin_x,
            list_bands: bands,
            list_band_count: starts.len(),
            band_travel: (12 * sx) as i32,
            zoom_max_q16: 7 << 16,
        })
    }
}

#[must_use]
pub fn supports_dimensions(width: usize, height: usize) -> bool {
    SettingsCogLayout::for_dimensions(width, height).is_some()
}

const fn rgb565(r: u16, g: u16, b: u16) -> u16 {
    ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
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

fn zoom_q16_to(p: i64, maximum: i64) -> i64 {
    if maximum == ZOOM_MAX_Q16 {
        return zoom_q16(p);
    }
    let progress = p.clamp(0, 1 << 16) as f64 / 65536.0;
    ((maximum as f64 / 65536.0).powf(progress) * 65536.0).round() as i64
}

/// Immutable colour/mip data; the renderer owns its sampling scratch.
#[derive(Clone, Debug)]
pub struct CogArtwork(crate::arcade_card::CabinetArtwork);
impl CogArtwork {
    pub fn from_rgb888(rgb: &[u8]) -> Result<Self, String> {
        Ok(CogTexture::from_rgb888(rgb)?.artwork())
    }
}
#[derive(Clone, Debug)]
pub struct CogTexture(crate::arcade_card::CabinetTexture);
impl CogTexture {
    pub fn from_rgb888(rgb: &[u8]) -> Result<Self, String> {
        Ok(Self(crate::arcade_card::CabinetTexture::from_rgb888_sized(
            COG_ASSET_WIDTH,
            COG_ASSET_HEIGHT,
            rgb,
        )?))
    }
    pub fn from_artwork(artwork: &CogArtwork) -> Self {
        Self(crate::arcade_card::CabinetTexture::from_artwork(&artwork.0))
    }
    pub fn storage_bytes(&self) -> usize {
        self.0.storage_bytes()
    }
    pub fn prepare_destination(&self, destination: &mut [Rgb565Pixel]) -> bool {
        if destination.len() != SETTINGS_COG_WIDTH * SETTINGS_COG_HEIGHT {
            return false;
        }
        let pixels = self.destination_pixels();
        for y in 0..COG_ASSET_HEIGHT {
            for x in 0..COG_ASSET_WIDTH {
                let dx = COG_REST_X + x as i32;
                let dy = COG_REST_Y + y as i32;
                if dx >= 0
                    && dx < SETTINGS_COG_WIDTH as i32
                    && dy >= CONTENT_TOP as i32
                    && dy < CONTENT_BOTTOM as i32
                {
                    destination[dy as usize * SETTINGS_COG_WIDTH + dx as usize] =
                        pixels[y * COG_ASSET_WIDTH + x];
                }
            }
        }
        true
    }
    pub fn artwork(&self) -> CogArtwork {
        CogArtwork(self.0.artwork())
    }
    pub fn destination_pixels(&self) -> Vec<Rgb565Pixel> {
        self.0.pixels_at(COG_REST_X, COG_REST_Y, 128)
    }
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
    cog: &CogTexture,
    t_ms: u32,
    output: &mut [Rgb565Pixel],
) -> bool {
    render_settings_cog_transition_for_dimensions_into(
        SETTINGS_COG_WIDTH,
        SETTINGS_COG_HEIGHT,
        launcher,
        settings,
        cog,
        t_ms,
        output,
    )
}

/// Render the same card-to-Settings timeline for a supported physical raster.
///
/// CRT portrait modes are already rotated into physical scanout space here,
/// so their responsive card and page geometry is derived directly from the
/// buffer dimensions rather than rotating pixels in the hot path.
#[must_use]
pub fn render_settings_cog_transition_for_dimensions_into(
    width: usize,
    height: usize,
    launcher: &[Rgb565Pixel],
    settings: &[Rgb565Pixel],
    cog: &CogTexture,
    t_ms: u32,
    output: &mut [Rgb565Pixel],
) -> bool {
    let Some(layout) = SettingsCogLayout::for_dimensions(width, height) else {
        return false;
    };
    let frame_len = width.saturating_mul(height);
    if launcher.len() != frame_len || settings.len() != frame_len || output.len() != frame_len {
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
    let w = layout.width;

    // Timeline (ms): the outline zooms 0-760, the cog travels 80-840, the
    // card face fades 60-260, and list bands slide in from 560 with a 30 ms
    // stagger. The launcher remains still behind the expanding opaque card;
    // the card itself occludes the carousel instead of forcing a full-screen
    // fade every frame.
    let zoom_p = ease_in_out(window_q16(t, 0, 760));
    let cog_p = ease_in_out(window_q16(t, 80, 760));
    let z = zoom_q16_to(zoom_p, layout.zoom_max_q16);
    let face_alpha = 256 - alpha_of(window_q16(t, 60, 200));
    // The outline fades as it leaves the screen: 1 - 1.25 p^2.
    let outline_p = window_q16(t, 0, 760);
    let outline_alpha = alpha_of((1 << 16) - ((outline_p * outline_p) >> 16) * 5 / 4);

    // Window (the zoomed card) in Q16 screen pixels.
    let card_cx = layout.card_x + layout.card_w / 2;
    let card_cy = layout.card_y + layout.card_h / 2;
    let win_w = i64::from(layout.card_w) * z;
    let win_h = i64::from(layout.card_h) * z;
    let win_x = (i64::from(card_cx) << 16) - win_w / 2;
    let win_y = (i64::from(card_cy) << 16) - win_h / 2;
    let win_r = i64::from(layout.card_radius) * z;
    let stroke = ((3.0 * ((z as f64) / 65536.0).sqrt()) * 65536.0) as i64;

    // Cog: screen = origin + scale * asset, interpolated from the card crop.
    let c0_x = (i64::from(layout.card_w) << 16) / i64::from(RENDER_CARD_W);
    let c0_y = (i64::from(layout.card_h) << 16) / i64::from(RENDER_CARD_Y_Q1);
    let start_x = (i64::from(layout.card_x) << 16)
        + (i64::from(ASSET_CROP_X * 2 - RENDER_CARD_X_Q1) * c0_x) / 2;
    let start_y = (i64::from(layout.card_y) << 16)
        + (i64::from(ASSET_CROP_Y * 2 - RENDER_CARD_Y_Q1) * c0_y) / 2;
    let lerp = |a: i64, b: i64| a + (((b - a) * cog_p) >> 16);
    let cog_x = lerp(start_x, i64::from(layout.cog_rest_x) << 16);
    let cog_y = lerp(start_y, i64::from(layout.cog_rest_y) << 16);
    let cog_sx = lerp(
        c0_x,
        (i64::from(layout.cog_rest_w) << 16) / COG_ASSET_WIDTH as i64,
    );
    let cog_sy = lerp(
        c0_y,
        (i64::from(layout.cog_rest_h) << 16) / COG_ASSET_HEIGHT as i64,
    );
    let cog_alpha = (256 << 16)
        + ((((i64::from(layout.cog_rest_alpha) - 256) << 16) * window_q16(t, 380, 320)) >> 16);
    let cog_alpha = cog_alpha >> 16;
    let inv_sx = (1i64 << 32) / cog_sx.max(1);
    let inv_sy = (1i64 << 32) / cog_sy.max(1);
    // Register the fading native card with the cog camera, rather than the
    // independently expanding outline. Otherwise reverse playback shows two
    // displaced cog silhouettes during the card/artwork crossfade.
    let face_x = cog_x - (i64::from(ASSET_CROP_X * 2 - RENDER_CARD_X_Q1) * cog_sx) / 2;
    let face_y = cog_y - (i64::from(ASSET_CROP_Y * 2 - RENDER_CARD_Y_Q1) * cog_sy) / 2;
    let face_inverse_x = (c0_x << 16) / cog_sx.max(1);
    let face_inverse_y = (c0_y << 16) / cog_sy.max(1);
    let cog_x0 = (cog_x >> 16).max(0) as usize;
    let cog_x1 =
        (((cog_x + COG_ASSET_WIDTH as i64 * cog_sx) >> 16) + 1).clamp(0, w as i64) as usize;
    let cog_y0 = (cog_y >> 16).max(0) as usize;
    let cog_y1 = (((cog_y + COG_ASSET_HEIGHT as i64 * cog_sy) >> 16) + 1)
        .clamp(0, layout.height as i64) as usize;
    // Copy only the Home pixels that remain visible, instead of copying and
    // immediately clearing the entire covered window. Keep each clip span once.
    output[..layout.content_top * w].copy_from_slice(&settings[..layout.content_top * w]);
    output[layout.content_bottom * w..layout.footer_top * w]
        .copy_from_slice(&settings[layout.content_bottom * w..layout.footer_top * w]);
    let footer = if t >= 860 { settings } else { launcher };
    output[layout.footer_top * w..].copy_from_slice(&footer[layout.footer_top * w..]);
    let mut spans = [None; 640];
    for (y, span) in spans
        .iter_mut()
        .enumerate()
        .take(layout.content_bottom)
        .skip(layout.content_top)
    {
        *span = rounded_span(y as i32, win_x, win_y, win_w, win_h, win_r, w);
        let row = y * w;
        if let Some((left, right)) = *span {
            output[row..row + left].copy_from_slice(&launcher[row..row + left]);
            output[row + left..row + right].fill(Rgb565Pixel(0));
            output[row + right..row + w].copy_from_slice(&launcher[row + right..row + w]);
        } else {
            output[row..row + w].copy_from_slice(&launcher[row..row + w]);
        }
    }
    cog.0.render_clipped(
        w,
        output,
        (cog_x0, cog_x1, cog_y0, cog_y1),
        (cog_x, cog_y, inv_sx, inv_sy),
        cog_alpha as u32,
        |y| spans[y].unwrap_or((w, w)),
    );
    for (y, &span) in spans
        .iter()
        .enumerate()
        .take(layout.content_bottom)
        .skip(layout.content_top)
    {
        let row = y * w;
        let out = &mut output[row..row + w];
        let (in0, in1) = span.unwrap_or((w, w));

        if span.is_some() && face_alpha > 0 {
            // Native label/frame pixels share the cog's camera transform.
            let sy = i64::from(layout.card_y)
                + ((((((y as i64) << 16) + (1 << 15) - face_y) * face_inverse_y) >> 16) >> 16);
            if (layout.card_y as i64..(layout.card_y + layout.card_h) as i64).contains(&sy) {
                let face_row = sy as usize * w;
                let mut sx_q16 = (i64::from(layout.card_x) << 16)
                    + (((((in0 as i64) << 16) + (1 << 15) - face_x) * face_inverse_x) >> 16);
                for pixel in out.iter_mut().take(in1).skip(in0) {
                    let sx = sx_q16 >> 16;
                    if (layout.card_x as i64..(layout.card_x + layout.card_w) as i64).contains(&sx)
                    {
                        *pixel = Rgb565Pixel(blend(
                            pixel.0,
                            launcher[face_row + sx as usize].0,
                            face_alpha,
                        ));
                    }
                    sx_q16 += face_inverse_x;
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
                w,
            )
        {
            let (i0, i1) = span.map_or((o1, o1), |(i0, i1)| (i0.clamp(o0, o1), i1.clamp(o0, o1)));
            for x in (o0..i0).chain(i1..o1) {
                out[x] = Rgb565Pixel(blend(out[x].0, OUTLINE, outline_alpha));
            }
        }
    }

    // Settings list bands: Slint's pixels, whole-pixel slide, alpha fade.
    for (index, &(top, bottom)) in layout.list_bands[..layout.list_band_count]
        .iter()
        .enumerate()
    {
        let at = BAND_START_MS + index as u32 * BAND_STAGGER_MS;
        let k = ease_out(window_q16(t, at, BAND_DURATION_MS));
        let alpha = alpha_of(k);
        if alpha == 0 {
            continue;
        }
        let offset = ((i64::from(layout.band_travel) * ((1 << 16) - k) + (1 << 15)) >> 16) as usize;
        let len =
            (layout.list_right - layout.list_left).min(w.saturating_sub(layout.list_left + offset));
        for y in top..bottom {
            let row = y * w;
            blend_row(
                &mut output[row + layout.list_left + offset..row + layout.list_left + offset + len],
                &settings[row + layout.list_left..row + layout.list_left + len],
                alpha,
            );
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

    fn patterned_cog() -> CogTexture {
        CogTexture::from_rgb888(
            &(0..COG_ASSET_WIDTH * COG_ASSET_HEIGHT * 3)
                .map(|i| ((i * 2654435761) >> 7) as u8)
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    #[test]
    fn packed_rgb565_interpolation_preserves_endpoints_and_lanes() {
        assert_eq!(lerp_rgb565(0x1234, 0xabcd, 0), 0x1234);
        assert_eq!(lerp_rgb565(0x1234, 0xabcd, 32), 0xabcd);
        assert_eq!(lerp_rgb565(0x0000, 0xffff, 16), 0x7bef);
    }

    #[test]
    fn native_card_and_filtered_cog_remain_one_registered_shape_during_handoff() {
        let mut rgb = vec![0; COG_ASSET_WIDTH * COG_ASSET_HEIGHT * 3];
        for y in 187..199 {
            for x in 199..211 {
                rgb[(y * COG_ASSET_WIDTH + x) * 3..(y * COG_ASSET_WIDTH + x) * 3 + 3].fill(255);
            }
        }
        let texture = CogTexture::from_rgb888(&rgb).unwrap();
        let mut launcher = frame(0);
        // The same camera landmark on the native 180x252 card.
        for y in 259..264 {
            for x in 656..661 {
                launcher[y * SETTINGS_COG_WIDTH + x] = Rgb565Pixel(0xffff);
            }
        }
        let destination = frame(0);
        let mut pixels = frame(0);
        for t in [120, 180, 239] {
            assert!(render_settings_cog_transition_into(
                &launcher,
                &destination,
                &texture,
                t,
                &mut pixels
            ));
            let mut xs = Vec::new();
            for x in 600..700 {
                if (230..280).any(|y| {
                    let p = pixels[y * SETTINGS_COG_WIDTH + x].0;
                    let (r, g, b) = (
                        u32::from(p >> 11),
                        u32::from((p >> 5) & 63),
                        u32::from(p & 31),
                    );
                    g > 0 && g * 31 * 10 >= r * 63 * 9 && g * 31 * 10 >= b * 63 * 9
                }) {
                    xs.push(x);
                }
            }
            assert!(!xs.is_empty());
            assert!(
                xs.windows(2).all(|p| p[1] == p[0] + 1),
                "displaced camera landmarks at {t}: {xs:?}"
            );
        }
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
        let resting = cog.destination_pixels();
        for v in 0..COG_ASSET_HEIGHT {
            for u in 0..COG_ASSET_WIDTH {
                let (x, y) = (u as i32 + COG_REST_X, v as i32 + COG_REST_Y);
                if (0..SETTINGS_COG_WIDTH as i32).contains(&x)
                    && (CONTENT_TOP as i32..CONTENT_BOTTOM as i32).contains(&y)
                {
                    settings[y as usize * SETTINGS_COG_WIDTH + x as usize] =
                        resting[v * COG_ASSET_WIDTH + u];
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
        let cog =
            CogTexture::from_rgb888(&vec![0; COG_ASSET_WIDTH * COG_ASSET_HEIGHT * 3]).unwrap();
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
        let cog =
            CogTexture::from_rgb888(&vec![0; COG_ASSET_WIDTH * COG_ASSET_HEIGHT * 3]).unwrap();
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
        let cog =
            CogTexture::from_rgb888(&vec![0; COG_ASSET_WIDTH * COG_ASSET_HEIGHT * 3]).unwrap();
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

    #[test]
    fn native_crt_rasters_are_supported_in_both_orientations() {
        for (width, height) in [
            (640, 240),
            (240, 640),
            (640, 288),
            (288, 640),
            (640, 480),
            (480, 640),
            (640, 576),
            (576, 640),
        ] {
            assert!(supports_dimensions(width, height), "{width}x{height}");
        }
        assert!(!supports_dimensions(800, 600));
    }

    #[test]
    fn crt_layout_uses_the_responsive_launcher_card_geometry() {
        let layout = SettingsCogLayout::for_dimensions(640, 240).expect("native CRT layout");
        assert_eq!(
            (layout.card_x, layout.card_y, layout.card_w, layout.card_h),
            (239, 54, 162, 112)
        );
        assert_eq!((layout.content_top, layout.content_bottom), (31, 208));
    }

    #[test]
    fn crt_endpoints_are_exact_and_midpoint_is_rendered() {
        for (width, height) in [(640, 240), (240, 640)] {
            let launcher = vec![Rgb565Pixel(0x1234); width * height];
            let settings = vec![Rgb565Pixel(0x4321); width * height];
            let cog = patterned_cog();
            let mut output = vec![Rgb565Pixel(0); width * height];
            assert!(render_settings_cog_transition_for_dimensions_into(
                width,
                height,
                &launcher,
                &settings,
                &cog,
                0,
                &mut output,
            ));
            assert_eq!(output, launcher);
            assert!(render_settings_cog_transition_for_dimensions_into(
                width,
                height,
                &launcher,
                &settings,
                &cog,
                SETTINGS_COG_DURATION_MS / 2,
                &mut output,
            ));
            assert_ne!(output, launcher);
            assert_ne!(output, settings);
            assert!(render_settings_cog_transition_for_dimensions_into(
                width,
                height,
                &launcher,
                &settings,
                &cog,
                SETTINGS_COG_DURATION_MS,
                &mut output,
            ));
            assert_eq!(output, settings);
        }
    }
}
