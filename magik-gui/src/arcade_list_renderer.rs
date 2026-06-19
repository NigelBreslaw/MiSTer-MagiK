use std::collections::HashMap;

use crate::arcade_catalog::{ArcadeGameEntry, ARCADE_ROW_HEIGHT};
use crate::bitmap_text::{ConsoleFont, TextGradient};
use crate::fb::{pixel_to_rgb565, Display, Pixel};
use crate::ui_display::UiDisplay;
use crate::ui_runner::ui_frame_target::{DirtyRect, UiFrameTarget};
use slint::platform::software_renderer::Rgb565Pixel;

pub(crate) const ARCADE_LIST_X: usize = 8;
pub(crate) const ARCADE_LIST_Y: usize = 56;
pub(crate) const ARCADE_LIST_W: usize = 464;
pub(crate) const ARCADE_LIST_H: usize = 384;
pub(crate) const ARCADE_LIST_FONT_PX: f32 = 16.0;
pub(crate) const ARCADE_LIST_META_FONT_PX: f32 = 8.0;
pub(crate) const ARCADE_LIST_FADE_H: usize = 48;
pub(crate) const ARCADE_LIST_FADE_MAX_ALPHA: u32 = 256;
pub(crate) const ARCADE_LIST_FADE_COLOR: Pixel = Pixel(0x001a1424);
pub(crate) const ARCADE_LIST_FADE_COLOR_565: Rgb565Pixel = rgb565_from_rgb888(0x1a, 0x14, 0x24);
pub(crate) const ARCADE_TITLE_GRADIENT: TextGradient =
    TextGradient::new(Pixel(0x00fff6ff), Pixel(0x00dbd1e6), Pixel(0x00938a9b));
pub(crate) const ARCADE_ROW_CACHE_MAX: usize = 128;
const ARCADE_ROW_CACHE_PRUNE_TO: usize = 96;

pub(crate) struct ArcadeListRenderer {
    title_font: ConsoleFont,
    meta_font: ConsoleFont,
    row_cache: HashMap<usize, CachedArcadeRow>,
    surface: Vec<Rgb565Pixel>,
    band_scratch: Vec<Pixel>,
    selection_horizontal: Vec<Rgb565Pixel>,
    selection_vertical: Vec<Rgb565Pixel>,
    row_cache_epoch: u64,
    surface_y: usize,
    last_draw: Option<ArcadeListDrawKey>,
}

pub(crate) struct CachedArcadeRow {
    pub(crate) title: String,
    pub(crate) pixels: Vec<Rgb565Pixel>,
    pub(crate) last_used: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ArcadeListDrawKey {
    len: usize,
    visual_px: i32,
    anchor_hash: u64,
    visible_hash: u64,
}

pub(crate) enum ArcadeListUpdate {
    Full(DirtyRect),
    /// The cached RAM list surface was advanced by scrolling and patching only
    /// the newly exposed content band. This is not a framebuffer dirty rect:
    /// presenting a scroll by reading from live `/dev/fb0` was measured slower
    /// on MiSTer's write-combined framebuffer than rewriting the list overlay.
    Scroll {
        delta_y: isize,
    },
}

impl ArcadeListRenderer {
    pub(crate) fn new() -> Self {
        Self {
            title_font: ConsoleFont::new(ARCADE_LIST_FONT_PX),
            meta_font: ConsoleFont::new(ARCADE_LIST_META_FONT_PX),
            row_cache: HashMap::new(),
            surface: vec![ARCADE_LIST_FADE_COLOR_565; ARCADE_LIST_W * ARCADE_LIST_H],
            band_scratch: Vec::new(),
            selection_horizontal: Vec::new(),
            selection_vertical: Vec::new(),
            row_cache_epoch: 0,
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
        let previous = self.last_draw;
        let key = ArcadeListDrawKey {
            len: games.len(),
            visual_px,
            anchor_hash: arcade_anchor_hash(games.get(anchor)),
            visible_hash: arcade_visible_window_hash(games, visual_index),
        };
        if !force && self.last_draw.as_ref() == Some(&key) {
            return None;
        }
        if force && self.last_draw.as_ref() == Some(&key) {
            return Some(ArcadeListUpdate::Full(Self::dirty_rect()));
        }
        let content_delta = previous
            .as_ref()
            .map(|previous| previous.visual_px - visual_px)
            .unwrap_or(0);
        let same_len = previous
            .as_ref()
            .is_some_and(|previous| previous.len == key.len);
        let visible_content_changed_at_same_position = previous.as_ref().is_some_and(|previous| {
            previous.len == key.len
                && previous.visual_px == key.visual_px
                && previous.visible_hash != key.visible_hash
        });
        let can_reuse_scrolled_surface = same_len && !visible_content_changed_at_same_position;
        self.last_draw = Some(key);
        if previous.is_none() || !can_reuse_scrolled_surface || games.is_empty() {
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
        if !can_reuse_scrolled_surface {
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
        if games.is_empty() {
            let mut band = std::mem::take(&mut self.band_scratch);
            band.resize(ARCADE_LIST_W * band_h, ARCADE_LIST_FADE_COLOR);
            band.fill(ARCADE_LIST_FADE_COLOR);
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
        self.fill_surface_band(band_y, band_h, ARCADE_LIST_FADE_COLOR_565);
        let row_h = ARCADE_ROW_HEIGHT as isize;
        let local_anchor_y = Self::selection_y() as isize;
        let Some((first, end)) = arcade_visible_window_range(games.len(), visual_index) else {
            return;
        };
        for idx in first..=end {
            let y =
                local_anchor_y + ((idx as f32 - visual_index) * ARCADE_ROW_HEIGHT as f32) as isize;
            let clip_y0 = y.max(band_y as isize);
            let clip_y1 = (y + row_h).min((band_y + band_h) as isize);
            if clip_y1 <= clip_y0 {
                continue;
            }
            self.blit_cached_row_to_surface(band_h, band_y, &games[idx].title, idx, y);
        }
    }

    fn blit_cached_row_to_surface(
        &mut self,
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
            if self.row_cache.len() >= ARCADE_ROW_CACHE_MAX {
                prune_arcade_row_cache(&mut self.row_cache);
            }
            let row = self.render_row(title, idx);
            let last_used = self.next_row_cache_epoch();
            self.row_cache.insert(
                idx,
                CachedArcadeRow {
                    title: title.to_string(),
                    pixels: row,
                    last_used,
                },
            );
        } else {
            let last_used = self.next_row_cache_epoch();
            if let Some(cached) = self.row_cache.get_mut(&idx) {
                cached.last_used = last_used;
            }
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
            let viewport_y = band_y + dst_y + row_y;
            let dst_y = (self.surface_y + viewport_y) % ARCADE_LIST_H;
            let dst = dst_y * ARCADE_LIST_W;
            self.surface[dst..dst + ARCADE_LIST_W].copy_from_slice(&row[src..src + ARCADE_LIST_W]);
        }
    }

    fn fill_surface_band(&mut self, band_y: usize, band_h: usize, color: Rgb565Pixel) {
        for row in 0..band_h {
            let dst_y = (self.surface_y + band_y + row) % ARCADE_LIST_H;
            let dst = dst_y * ARCADE_LIST_W;
            self.surface[dst..dst + ARCADE_LIST_W].fill(color);
        }
    }

    fn next_row_cache_epoch(&mut self) -> u64 {
        self.row_cache_epoch = self.row_cache_epoch.wrapping_add(1);
        self.row_cache_epoch
    }

    fn copy_band_to_surface(&mut self, band: &[Pixel], band_y: usize, band_h: usize) {
        for row in 0..band_h {
            let src = row * ARCADE_LIST_W;
            let dst_y = (self.surface_y + band_y + row) % ARCADE_LIST_H;
            let dst = dst_y * ARCADE_LIST_W;
            copy_pixel_to_rgb565_row(
                &band[src..src + ARCADE_LIST_W],
                &mut self.surface[dst..dst + ARCADE_LIST_W],
            );
        }
    }

    pub(crate) fn copy_layer_to_target(
        &mut self,
        target: &mut UiFrameTarget,
        disp: &mut Display,
        ui: &UiDisplay,
    ) {
        self.copy_viewport_band_to_target(target, disp, ui, 0, ARCADE_LIST_H);
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
            target.copy_rect_from_565(
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

    fn copy_selection_frame_to_target(
        &mut self,
        target: &mut UiFrameTarget,
        disp: &mut Display,
        ui: &UiDisplay,
    ) {
        let rect = Self::selection_rect();
        let color = rgb565_from_rgb888(0x06, 0xd6, 0xa0);
        let thickness = 3usize;
        let h = rect.y1.saturating_sub(rect.y0).min(ARCADE_LIST_H);
        self.selection_horizontal
            .resize(ARCADE_LIST_W * thickness, color);
        self.selection_horizontal.fill(color);
        target.copy_rect_from_565(
            disp,
            ui,
            rect.x0,
            rect.y0,
            ARCADE_LIST_W,
            thickness,
            &self.selection_horizontal,
        );
        target.copy_rect_from_565(
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
        target.copy_rect_from_565(
            disp,
            ui,
            rect.x0,
            rect.y0,
            thickness,
            h,
            &self.selection_vertical,
        );
        target.copy_rect_from_565(
            disp,
            ui,
            rect.x1.saturating_sub(thickness),
            rect.y0,
            thickness,
            h,
            &self.selection_vertical,
        );
    }

    fn render_row(&mut self, title: &str, idx: usize) -> Vec<Rgb565Pixel> {
        let mut row = vec![Pixel(0); ARCADE_LIST_W * ARCADE_ROW_HEIGHT as usize];
        draw_arcade_row_background(&mut row, idx);
        let title = clipped_title(title, 30);
        self.title_font.draw_text_clipped_gradient(
            &mut row,
            ARCADE_LIST_W,
            ARCADE_LIST_W,
            0,
            ARCADE_ROW_HEIGHT as usize,
            12,
            30,
            &title,
            ARCADE_TITLE_GRADIENT,
        );
        row.into_iter().map(pixel_to_rgb565).collect()
    }
}

pub(crate) fn prune_arcade_row_cache(row_cache: &mut HashMap<usize, CachedArcadeRow>) {
    if row_cache.len() < ARCADE_ROW_CACHE_MAX {
        return;
    }
    let keep = ARCADE_ROW_CACHE_PRUNE_TO.min(row_cache.len());
    let mut last_used = row_cache
        .values()
        .map(|row| row.last_used)
        .collect::<Vec<_>>();
    let cutoff_index = last_used.len().saturating_sub(keep);
    let (_, cutoff, _) = last_used.select_nth_unstable(cutoff_index);
    let cutoff = *cutoff;
    row_cache.retain(|_, row| row.last_used >= cutoff);
}

const ARCADE_LIST_HASH_OFFSET: u64 = 0xcbf29ce484222325;
const ARCADE_LIST_HASH_PRIME: u64 = 0x100000001b3;

fn arcade_anchor_hash(game: Option<&ArcadeGameEntry>) -> u64 {
    let mut hash = ARCADE_LIST_HASH_OFFSET;
    if let Some(game) = game {
        arcade_hash_game(&mut hash, game);
    }
    hash
}

fn arcade_visible_window_hash(games: &[ArcadeGameEntry], visual_index: f32) -> u64 {
    let mut hash = ARCADE_LIST_HASH_OFFSET;
    let Some((first, end)) = arcade_visible_window_range(games.len(), visual_index) else {
        return hash;
    };
    arcade_hash_usize(&mut hash, first);
    arcade_hash_usize(&mut hash, end);
    for idx in first..=end {
        arcade_hash_usize(&mut hash, idx);
        arcade_hash_game(&mut hash, &games[idx]);
    }
    hash
}

fn arcade_visible_window_range(len: usize, visual_index: f32) -> Option<(usize, usize)> {
    if len == 0 {
        return None;
    }
    let first = ((visual_index.floor() as isize) - 7).max(0) as usize;
    let last = ((visual_index.ceil() as isize) + 8).max(0) as usize;
    Some((first.min(len - 1), last.min(len - 1)))
}

fn arcade_hash_game(hash: &mut u64, game: &ArcadeGameEntry) {
    arcade_hash_bytes(hash, game.system_id.as_bytes());
    arcade_hash_bytes(hash, game.mra_path.as_bytes());
    arcade_hash_bytes(hash, game.image_path.as_bytes());
    arcade_hash_bytes(hash, game.title.as_bytes());
}

fn arcade_hash_usize(hash: &mut u64, value: usize) {
    arcade_hash_bytes(hash, &(value as u64).to_le_bytes());
}

fn arcade_hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= *byte as u64;
        *hash = hash.wrapping_mul(ARCADE_LIST_HASH_PRIME);
    }
    *hash ^= 0xff;
    *hash = hash.wrapping_mul(ARCADE_LIST_HASH_PRIME);
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

const fn rgb565_from_rgb888(r: u8, g: u8, b: u8) -> Rgb565Pixel {
    Rgb565Pixel((((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)) as u16)
}

fn copy_pixel_to_rgb565_row(src: &[Pixel], dst: &mut [Rgb565Pixel]) {
    for (src, dst) in src.iter().zip(dst.iter_mut()) {
        *dst = pixel_to_rgb565(*src);
    }
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
    fn new(alpha: u32, color: Rgb565Pixel) -> Self {
        let color = color.0 as u32;
        let cr = (color >> 11) & 0x1f;
        let cg = (color >> 5) & 0x3f;
        let cb = color & 0x1f;
        Self {
            inv: 256 - alpha,
            cr_alpha: cr * alpha,
            cg_alpha: cg * alpha,
            cb_alpha: cb * alpha,
        }
    }
}

pub(crate) fn fade_blend_constants(fade_h: usize, color: Rgb565Pixel) -> Vec<FadeBlendConstants> {
    (0..fade_h)
        .map(|row| FadeBlendConstants::new(fade_alpha(row, fade_h), color))
        .collect()
}

pub(crate) fn blend_row_towards(
    src: &[Rgb565Pixel],
    dst: &mut [Rgb565Pixel],
    constants: FadeBlendConstants,
) {
    for (src, dst) in src.iter().zip(dst.iter_mut()) {
        let src = src.0 as u32;
        let sr = (src >> 11) & 0x1f;
        let sg = (src >> 5) & 0x3f;
        let sb = src & 0x1f;
        let r = (sr * constants.inv + constants.cr_alpha) >> 8;
        let g = (sg * constants.inv + constants.cg_alpha) >> 8;
        let b = (sb * constants.inv + constants.cb_alpha) >> 8;
        *dst = Rgb565Pixel(((r << 11) | (g << 5) | b) as u16);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arcade_anchor_hash_changes_for_anchor_identity_fields() {
        let base = game("arcade", "/media/fat/_Arcade/a.mra", "Alpha");

        assert_ne!(
            arcade_anchor_hash(Some(&base)),
            arcade_anchor_hash(Some(&game("console", "/media/fat/_Arcade/a.mra", "Alpha")))
        );
        assert_ne!(
            arcade_anchor_hash(Some(&base)),
            arcade_anchor_hash(Some(&game("arcade", "/media/fat/_Arcade/b.mra", "Alpha")))
        );
        assert_ne!(
            arcade_anchor_hash(Some(&base)),
            arcade_anchor_hash(Some(&game("arcade", "/media/fat/_Arcade/a.mra", "Beta")))
        );
    }

    #[test]
    fn arcade_anchor_hash_is_stable_for_same_anchor() {
        let base = game("arcade", "/media/fat/_Arcade/a.mra", "Alpha");

        assert_eq!(
            arcade_anchor_hash(Some(&base)),
            arcade_anchor_hash(Some(&base))
        );
    }

    #[test]
    fn redraws_when_visible_non_anchor_row_changes() {
        let mut renderer = ArcadeListRenderer::new();
        let mut games = (0..20)
            .map(|idx| {
                game(
                    "arcade",
                    &format!("/media/fat/_Arcade/{idx}.mra"),
                    &format!("Game {idx}"),
                )
            })
            .collect::<Vec<_>>();

        assert!(matches!(
            renderer.draw(&games, 7.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
        assert!(renderer.draw(&games, 7.0, false).is_none());

        games[3].title = "Changed visible row".into();

        assert!(matches!(
            renderer.draw(&games, 7.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
    }

    #[test]
    fn forced_present_reuses_surface_when_draw_key_is_unchanged() {
        let mut renderer = ArcadeListRenderer::new();
        let games = (0..20)
            .map(|idx| {
                game(
                    "arcade",
                    &format!("/media/fat/_Arcade/{idx}.mra"),
                    &format!("Game {idx}"),
                )
            })
            .collect::<Vec<_>>();

        assert!(matches!(
            renderer.draw(&games, 7.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
        let before = renderer.surface.clone();

        assert!(matches!(
            renderer.draw(&games, 7.0, true),
            Some(ArcadeListUpdate::Full(_))
        ));

        assert_eq!(renderer.surface, before);
    }

    #[test]
    fn arcade_row_title_uses_gradient_pixels() {
        let mut renderer = ArcadeListRenderer::new();
        let row = renderer.render_row("MAGIK", 0);
        let bg = pixel_to_rgb565(Pixel(0x001a1424));
        let border = pixel_to_rgb565(Pixel(0x00251c34));
        let title_pixels = row
            .iter()
            .copied()
            .filter(|px| *px != bg && *px != border)
            .collect::<Vec<_>>();

        assert!(!title_pixels.is_empty());
        let min_luma = title_pixels.iter().copied().map(rgb565_luma).min().unwrap();
        let max_luma = title_pixels.iter().copied().map(rgb565_luma).max().unwrap();

        assert!(max_luma > min_luma);
    }

    fn game(system_id: &str, mra_path: &str, title: &str) -> ArcadeGameEntry {
        ArcadeGameEntry {
            title: title.into(),
            mra_path: mra_path.into(),
            image_path: "".into(),
            has_image: false,
            system_id: system_id.into(),
        }
    }

    fn rgb565_luma(pixel: Rgb565Pixel) -> u32 {
        let value = pixel.0 as u32;
        let r = ((value >> 11) & 0x1f) << 3;
        let g = ((value >> 5) & 0x3f) << 2;
        let b = (value & 0x1f) << 3;
        r * 30 + g * 59 + b * 11
    }
}
