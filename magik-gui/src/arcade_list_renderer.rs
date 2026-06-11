use std::collections::HashMap;

use crate::arcade_catalog::{ArcadeGameEntry, ARCADE_ROW_HEIGHT};
use crate::bitmap_text::ConsoleFont;
use crate::fb::{Display, Pixel};
use crate::ui_display::UiDisplay;
use crate::ui_runner::{DirtyRect, UiFrameTarget};

pub(crate) const ARCADE_LIST_X: usize = 8;
pub(crate) const ARCADE_LIST_Y: usize = 56;
pub(crate) const ARCADE_LIST_W: usize = 464;
pub(crate) const ARCADE_LIST_H: usize = 384;
pub(crate) const ARCADE_LIST_FONT_PX: f32 = 16.0;
pub(crate) const ARCADE_LIST_META_FONT_PX: f32 = 8.0;
pub(crate) const ARCADE_LIST_FADE_H: usize = 48;
pub(crate) const ARCADE_LIST_FADE_MAX_ALPHA: u32 = 256;
pub(crate) const ARCADE_LIST_FADE_COLOR: Pixel = Pixel(0x001a1424);

pub(crate) struct ArcadeListRenderer {
    title_font: ConsoleFont,
    meta_font: ConsoleFont,
    row_cache: HashMap<usize, CachedArcadeRow>,
    surface: Vec<Pixel>,
    band_scratch: Vec<Pixel>,
    fade_scratch: Vec<Pixel>,
    fade_constants: Vec<FadeBlendConstants>,
    selection_horizontal: Vec<Pixel>,
    selection_vertical: Vec<Pixel>,
    surface_y: usize,
    last_draw: Option<ArcadeListDrawKey>,
}

pub(crate) struct CachedArcadeRow {
    pub(crate) title: String,
    pub(crate) pixels: Vec<Pixel>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArcadeListDrawKey {
    len: usize,
    visual_px: i32,
    anchor_system_id: String,
    anchor_mra_path: String,
    anchor_title: String,
}

pub(crate) enum ArcadeListUpdate {
    Full(DirtyRect),
    Scroll { delta_y: isize },
}

impl ArcadeListRenderer {
    pub(crate) fn new() -> Self {
        Self {
            title_font: ConsoleFont::new(ARCADE_LIST_FONT_PX),
            meta_font: ConsoleFont::new(ARCADE_LIST_META_FONT_PX),
            row_cache: HashMap::new(),
            surface: vec![Pixel(0); ARCADE_LIST_W * ARCADE_LIST_H],
            band_scratch: Vec::new(),
            fade_scratch: Vec::new(),
            fade_constants: fade_blend_constants(ARCADE_LIST_FADE_H, ARCADE_LIST_FADE_COLOR),
            selection_horizontal: Vec::new(),
            selection_vertical: Vec::new(),
            surface_y: 0,
            last_draw: None,
        }
    }

    pub(crate) fn dirty_rect() -> DirtyRect {
        DirtyRect {
            x0: ARCADE_LIST_X,
            y0: ARCADE_LIST_Y,
            x1: ARCADE_LIST_X + ARCADE_LIST_W,
            y1: ARCADE_LIST_Y + ARCADE_LIST_H,
        }
    }

    pub(crate) fn draw(
        &mut self,
        games: &[ArcadeGameEntry],
        visual_index: f32,
        force: bool,
    ) -> Option<ArcadeListUpdate> {
        let visual_px = (visual_index * ARCADE_ROW_HEIGHT as f32).round() as i32;
        let anchor = visual_index
            .round()
            .clamp(0.0, games.len().saturating_sub(1) as f32) as usize;
        let previous = self.last_draw.clone();
        let key = ArcadeListDrawKey {
            len: games.len(),
            visual_px,
            anchor_system_id: games
                .get(anchor)
                .map(|game| game.system_id.clone())
                .unwrap_or_default(),
            anchor_mra_path: games
                .get(anchor)
                .map(|game| game.mra_path.clone())
                .unwrap_or_default(),
            anchor_title: games
                .get(anchor)
                .map(|game| game.title.clone())
                .unwrap_or_default(),
        };
        if !force && self.last_draw.as_ref() == Some(&key) {
            return None;
        }
        let same_game_set = previous
            .as_ref()
            .is_some_and(|previous| previous.len == key.len);
        self.last_draw = Some(key);
        let content_delta = previous
            .as_ref()
            .map(|previous| previous.visual_px - visual_px)
            .unwrap_or(0);
        if force || previous.is_none() || !same_game_set || games.is_empty() {
            self.surface_y = 0;
            self.draw_content_band(games, visual_index, 0, ARCADE_LIST_H);
        } else if content_delta == 0 {
        } else if content_delta.unsigned_abs() as usize >= ARCADE_LIST_H {
            self.surface_y = 0;
            self.draw_content_band(games, visual_index, 0, ARCADE_LIST_H);
        } else if content_delta < 0 {
            let d = content_delta.unsigned_abs() as usize;
            self.surface_y = (self.surface_y + d) % ARCADE_LIST_H;
            self.draw_content_band(games, visual_index, ARCADE_LIST_H - d, d);
        } else {
            let d = content_delta as usize;
            self.surface_y = (self.surface_y + ARCADE_LIST_H - d) % ARCADE_LIST_H;
            self.draw_content_band(games, visual_index, 0, d);
        }
        if force {
            return Some(ArcadeListUpdate::Full(Self::dirty_rect()));
        }
        if previous.is_none() {
            return Some(ArcadeListUpdate::Full(Self::dirty_rect()));
        }
        if !same_game_set {
            return Some(ArcadeListUpdate::Full(Self::dirty_rect()));
        }
        if content_delta == 0 || content_delta.unsigned_abs() as usize >= ARCADE_LIST_H {
            return Some(ArcadeListUpdate::Full(Self::dirty_rect()));
        }
        Some(ArcadeListUpdate::Scroll {
            delta_y: content_delta as isize,
        })
    }

    pub(crate) fn selection_rect() -> DirtyRect {
        let y = Self::selection_y();
        DirtyRect {
            x0: ARCADE_LIST_X,
            y0: ARCADE_LIST_Y + y,
            x1: ARCADE_LIST_X + ARCADE_LIST_W,
            y1: ARCADE_LIST_Y + y + ARCADE_ROW_HEIGHT as usize,
        }
    }

    fn selection_y() -> usize {
        let row_h = ARCADE_ROW_HEIGHT as usize;
        let visible_rows = (ARCADE_LIST_H / row_h).max(1);
        (visible_rows / 2) * row_h
    }

    fn draw_content_band(
        &mut self,
        games: &[ArcadeGameEntry],
        visual_index: f32,
        band_y: usize,
        band_h: usize,
    ) {
        if band_h == 0 || band_y >= ARCADE_LIST_H {
            return;
        }
        let band_h = band_h.min(ARCADE_LIST_H - band_y);
        let mut band = std::mem::take(&mut self.band_scratch);
        band.resize(ARCADE_LIST_W * band_h, ARCADE_LIST_FADE_COLOR);
        band.fill(ARCADE_LIST_FADE_COLOR);
        if games.is_empty() {
            self.meta_font.draw_text_clipped(
                &mut band,
                ARCADE_LIST_W,
                ARCADE_LIST_W,
                0,
                band_h,
                96,
                (ARCADE_LIST_H / 2).saturating_sub(band_y) as isize,
                "NO GAMES",
                Pixel(0x00706080),
            );
            self.copy_band_to_surface(&band, band_y, band_h);
            self.band_scratch = band;
            return;
        }
        let row_h = ARCADE_ROW_HEIGHT as isize;
        let local_anchor_y = Self::selection_y() as isize;
        let first = ((visual_index.floor() as isize) - 7).max(0) as usize;
        let last = ((visual_index.ceil() as isize) + 8).max(0) as usize;
        let end = last.min(games.len().saturating_sub(1));
        for idx in first..=end {
            let y =
                local_anchor_y + ((idx as f32 - visual_index) * ARCADE_ROW_HEIGHT as f32) as isize;
            let clip_y0 = y.max(band_y as isize);
            let clip_y1 = (y + row_h).min((band_y + band_h) as isize);
            if clip_y1 <= clip_y0 {
                continue;
            }
            self.blit_cached_row_to_band(&mut band, band_h, band_y, &games[idx].title, idx, y);
        }
        self.copy_band_to_surface(&band, band_y, band_h);
        self.band_scratch = band;
    }

    fn blit_cached_row_to_band(
        &mut self,
        band: &mut [Pixel],
        band_h: usize,
        band_y: usize,
        title: &str,
        idx: usize,
        y: isize,
    ) {
        let needs_render = self
            .row_cache
            .get(&idx)
            .is_none_or(|cached| cached.title != title);
        if needs_render {
            if self.row_cache.len() > 128 {
                self.row_cache.clear();
            }
            let row = self.render_row(title, idx);
            self.row_cache.insert(
                idx,
                CachedArcadeRow {
                    title: title.to_string(),
                    pixels: row,
                },
            );
        }
        let row = &self.row_cache.get(&idx).expect("row cache insert").pixels;
        let row_h = ARCADE_ROW_HEIGHT as isize;
        let clip_y0 = y.max(band_y as isize);
        let clip_y1 = (y + row_h).min((band_y + band_h) as isize);
        if clip_y1 <= clip_y0 {
            return;
        }
        let copy_h = (clip_y1 - clip_y0) as usize;
        let src_y = (clip_y0 - y) as usize;
        let dst_y = (clip_y0 as usize).saturating_sub(band_y);
        for row_y in 0..copy_h {
            let src = (src_y + row_y) * ARCADE_LIST_W;
            let dst = (dst_y + row_y) * ARCADE_LIST_W;
            band[dst..dst + ARCADE_LIST_W].copy_from_slice(&row[src..src + ARCADE_LIST_W]);
        }
    }

    fn copy_band_to_surface(&mut self, band: &[Pixel], band_y: usize, band_h: usize) {
        for row in 0..band_h {
            let src = row * ARCADE_LIST_W;
            let dst_y = (self.surface_y + band_y + row) % ARCADE_LIST_H;
            let dst = dst_y * ARCADE_LIST_W;
            self.surface[dst..dst + ARCADE_LIST_W].copy_from_slice(&band[src..src + ARCADE_LIST_W]);
        }
    }

    pub(crate) fn copy_layer_to_target(
        &mut self,
        target: &mut UiFrameTarget,
        disp: &mut Display,
        ui: &UiDisplay,
    ) {
        let fade_h = ARCADE_LIST_FADE_H.min(ARCADE_LIST_H / 2);
        self.copy_fade_to_target(target, disp, ui);
        self.copy_viewport_band_to_target(target, disp, ui, fade_h, ARCADE_LIST_H - fade_h * 2);
        self.copy_selection_frame_to_target(target, disp, ui);
    }

    fn copy_viewport_band_to_target(
        &self,
        target: &mut UiFrameTarget,
        disp: &mut Display,
        ui: &UiDisplay,
        viewport_y: usize,
        h: usize,
    ) {
        if h == 0 || viewport_y >= ARCADE_LIST_H {
            return;
        }
        let h = h.min(ARCADE_LIST_H - viewport_y);
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % ARCADE_LIST_H;
            let copy_h = (h - copied).min(ARCADE_LIST_H - src_y);
            let src = src_y * ARCADE_LIST_W;
            target.copy_rect_from(
                disp,
                ui,
                ARCADE_LIST_X,
                ARCADE_LIST_Y + viewport_y + copied,
                ARCADE_LIST_W,
                copy_h,
                &self.surface[src..src + copy_h * ARCADE_LIST_W],
            );
            copied += copy_h;
        }
    }

    fn surface_row(&self, viewport_y: usize) -> &[Pixel] {
        let src_y = (self.surface_y + viewport_y) % ARCADE_LIST_H;
        let src = src_y * ARCADE_LIST_W;
        &self.surface[src..src + ARCADE_LIST_W]
    }

    fn copy_fade_to_target(
        &mut self,
        target: &mut UiFrameTarget,
        disp: &mut Display,
        ui: &UiDisplay,
    ) {
        let fade_h = ARCADE_LIST_FADE_H.min(ARCADE_LIST_H / 2);
        let mut band = std::mem::take(&mut self.fade_scratch);
        band.resize(ARCADE_LIST_W * fade_h, Pixel(0));
        for row in 0..fade_h {
            blend_row_towards(
                self.surface_row(row),
                &mut band[row * ARCADE_LIST_W..(row + 1) * ARCADE_LIST_W],
                self.fade_constants[row],
            );
        }
        target.copy_rect_from(
            disp,
            ui,
            ARCADE_LIST_X,
            ARCADE_LIST_Y,
            ARCADE_LIST_W,
            fade_h,
            &band,
        );

        for row in 0..fade_h {
            let viewport_y = ARCADE_LIST_H - fade_h + row;
            blend_row_towards(
                self.surface_row(viewport_y),
                &mut band[row * ARCADE_LIST_W..(row + 1) * ARCADE_LIST_W],
                self.fade_constants[fade_h - 1 - row],
            );
        }
        target.copy_rect_from(
            disp,
            ui,
            ARCADE_LIST_X,
            ARCADE_LIST_Y + ARCADE_LIST_H - fade_h,
            ARCADE_LIST_W,
            fade_h,
            &band,
        );
        self.fade_scratch = band;
    }

    fn copy_selection_frame_to_target(
        &mut self,
        target: &mut UiFrameTarget,
        disp: &mut Display,
        ui: &UiDisplay,
    ) {
        let rect = Self::selection_rect();
        let color = Pixel(0x0006d6a0);
        let thickness = 3usize;
        let h = rect.y1.saturating_sub(rect.y0).min(ARCADE_LIST_H);
        self.selection_horizontal
            .resize(ARCADE_LIST_W * thickness, color);
        self.selection_horizontal.fill(color);
        target.copy_rect_from(
            disp,
            ui,
            rect.x0,
            rect.y0,
            ARCADE_LIST_W,
            thickness,
            &self.selection_horizontal,
        );
        target.copy_rect_from(
            disp,
            ui,
            rect.x0,
            rect.y1.saturating_sub(thickness),
            ARCADE_LIST_W,
            thickness,
            &self.selection_horizontal,
        );
        self.selection_vertical.resize(thickness * h, color);
        self.selection_vertical.fill(color);
        target.copy_rect_from(
            disp,
            ui,
            rect.x0,
            rect.y0,
            thickness,
            h,
            &self.selection_vertical,
        );
        target.copy_rect_from(
            disp,
            ui,
            rect.x1.saturating_sub(thickness),
            rect.y0,
            thickness,
            h,
            &self.selection_vertical,
        );
    }

    fn render_row(&mut self, title: &str, idx: usize) -> Vec<Pixel> {
        let mut row = vec![Pixel(0); ARCADE_LIST_W * ARCADE_ROW_HEIGHT as usize];
        draw_arcade_row_background(&mut row, idx);
        let title = clipped_title(title, 30);
        self.title_font.draw_text_clipped(
            &mut row,
            ARCADE_LIST_W,
            ARCADE_LIST_W,
            0,
            ARCADE_ROW_HEIGHT as usize,
            12,
            30,
            &title,
            Pixel(0x00e8e0f0),
        );
        row
    }
}

pub(crate) fn draw_arcade_row_background(row: &mut [Pixel], idx: usize) {
    let bg = if idx % 2 == 0 {
        Pixel(0x001a1424)
    } else {
        Pixel(0x00150f20)
    };
    let border = Pixel(0x00251c34);
    for row_y in 0..ARCADE_ROW_HEIGHT as isize {
        let dy = row_y as usize;
        let line = &mut row[dy * ARCADE_LIST_W..(dy + 1) * ARCADE_LIST_W];
        for px in line.iter_mut() {
            *px = bg;
        }
        if row_y == 0 || row_y == ARCADE_ROW_HEIGHT as isize - 1 {
            for px in line.iter_mut() {
                *px = border;
            }
        }
    }
}

fn fade_alpha(row_from_edge: usize, fade_h: usize) -> u32 {
    if fade_h <= 1 {
        return ARCADE_LIST_FADE_MAX_ALPHA;
    }
    let inv = (fade_h - 1 - row_from_edge) as u32;
    (ARCADE_LIST_FADE_MAX_ALPHA * inv) / (fade_h - 1) as u32
}

pub(crate) fn blend_velocity_fade_h_from_env() -> usize {
    std::env::var("MISTER_BLEND_BENCH_FADE_H")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|h| *h > 0)
        .unwrap_or(ARCADE_LIST_FADE_H)
        .min(ARCADE_LIST_H / 2)
}

#[derive(Clone, Copy)]
pub(crate) struct FadeBlendConstants {
    inv: u32,
    cr_alpha: u32,
    cg_alpha: u32,
    cb_alpha: u32,
}

impl FadeBlendConstants {
    fn new(alpha: u32, color: Pixel) -> Self {
        let cr = (color.0 >> 16) & 0xff;
        let cg = (color.0 >> 8) & 0xff;
        let cb = color.0 & 0xff;
        Self {
            inv: 256 - alpha,
            cr_alpha: cr * alpha,
            cg_alpha: cg * alpha,
            cb_alpha: cb * alpha,
        }
    }
}

pub(crate) fn fade_blend_constants(fade_h: usize, color: Pixel) -> Vec<FadeBlendConstants> {
    (0..fade_h)
        .map(|row| FadeBlendConstants::new(fade_alpha(row, fade_h), color))
        .collect()
}

pub(crate) fn blend_row_towards(src: &[Pixel], dst: &mut [Pixel], constants: FadeBlendConstants) {
    for (src, dst) in src.iter().zip(dst.iter_mut()) {
        let sr = (src.0 >> 16) & 0xff;
        let sg = (src.0 >> 8) & 0xff;
        let sb = src.0 & 0xff;
        let r = (sr * constants.inv + constants.cr_alpha) >> 8;
        let g = (sg * constants.inv + constants.cg_alpha) >> 8;
        let b = (sb * constants.inv + constants.cb_alpha) >> 8;
        *dst = Pixel((r << 16) | (g << 8) | b);
    }
}

fn clipped_title(title: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in title.chars().take(max_chars) {
        out.push(ch);
    }
    if title.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}
