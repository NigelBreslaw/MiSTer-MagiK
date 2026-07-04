use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use crate::arcade_catalog::{
    ArcadeGameEntry, ArcadeGameView, ARCADE_LIST_VISIBLE_H, ARCADE_ROW_HEIGHT,
};
use crate::bitmap_text::{ConsoleFont, TextGradient};
use mister_magik_fb::framebuffer::mapped::{pixel_to_rgb565, MappedRgb565Framebuffer, Pixel};
use mister_magik_fb::framebuffer::target::{DirtyRect, UiFrameTarget};
use slint::platform::software_renderer::Rgb565Pixel;

pub(crate) const ARCADE_LIST_X: usize = 8;
pub(crate) const ARCADE_LIST_Y: usize = 56;
// Wider than the half-screen pane on purpose: the list can borrow boundary
// space without covering the centered preview cabinet.
pub(crate) const ARCADE_LIST_W: usize = 510;
pub(crate) const ARCADE_LIST_H: usize = ARCADE_LIST_VISIBLE_H as usize;
pub(crate) const ARCADE_SEARCH_LIST_Y: usize = 56;
pub(crate) const ARCADE_LIST_FONT_PX: f32 = 16.0;
pub(crate) const ARCADE_LIST_META_FONT_PX: f32 = 8.0;
pub(crate) const ARCADE_LIST_BG_COLOR: Pixel = Pixel(0x001a1424);
pub(crate) const ARCADE_LIST_BG_COLOR_565: Rgb565Pixel = rgb565_from_rgb888(0x1a, 0x14, 0x24);
const ARCADE_LIST_ALT_BG_COLOR_565: Rgb565Pixel = rgb565_from_rgb888(0x15, 0x0f, 0x20);
const ARCADE_LIST_ROW_BORDER_COLOR_565: Rgb565Pixel = rgb565_from_rgb888(0x25, 0x1c, 0x34);
const ARCADE_SELECTION_FILL_COLOR_565: Rgb565Pixel = rgb565_from_rgb888(0xe7, 0xe3, 0xec);
pub(crate) const ARCADE_TITLE_GRADIENT: TextGradient =
    TextGradient::new(Pixel(0x00fff6ff), Pixel(0x00dbd1e6), Pixel(0x00938a9b));
pub(crate) const ARCADE_FILTER_ACTIVE_GRADIENT: TextGradient =
    TextGradient::new(Pixel(0x0006d6a0), Pixel(0x0005b98a), Pixel(0x00047764));
pub(crate) const ARCADE_ROW_CACHE_MAX: usize = 128;
const ARCADE_ROW_CACHE_PRUNE_TO: usize = 96;
const ARCADE_ROW_FINGERPRINT_CACHE_MAX: usize = 512;
const ARCADE_ROW_FINGERPRINT_CACHE_PRUNE_TO: usize = 384;
const ARCADE_LIST_LAYER_COPY_BANDS: [(usize, usize); 1] = [(0, ARCADE_LIST_H)];
const ARCADE_SELECTION_FRAME_THICKNESS: usize = 3;
const ARCADE_SELECTION_FRAME_COLOR: Rgb565Pixel = rgb565_from_rgb888(0x06, 0xd6, 0xa0);
const ARCADE_NEW_BADGE_FILL: Pixel = Pixel(0x0006d6a0);
const ARCADE_NEW_BADGE_FILL_565: Rgb565Pixel = rgb565_from_rgb888(0x06, 0xd6, 0xa0);
const ARCADE_NEW_BADGE_TEXT: Pixel = Pixel(0x00120d1a);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ArcadeListGeometry {
    pub(crate) x: usize,
    pub(crate) y: usize,
}

impl ArcadeListGeometry {
    pub(crate) const NORMAL: Self = Self {
        x: ARCADE_LIST_X,
        y: ARCADE_LIST_Y,
    };

    pub(crate) fn search_for_render_w(render_w: usize) -> Self {
        Self {
            x: render_w.saturating_sub(ARCADE_LIST_X + ARCADE_LIST_W),
            y: ARCADE_SEARCH_LIST_Y,
        }
    }

    pub(crate) fn dirty_rect(self) -> DirtyRect {
        DirtyRect {
            x0: self.x,
            y0: self.y,
            x1: self.x + ARCADE_LIST_W,
            y1: self.y + ARCADE_LIST_H,
        }
    }
}

pub(crate) struct ArcadeListRenderer {
    title_font: ConsoleFont,
    meta_font: ConsoleFont,
    row_cache: HashMap<usize, CachedArcadeRow>,
    surface: Vec<Rgb565Pixel>,
    band_scratch: Vec<Pixel>,
    selection_invert_scratch: Vec<Rgb565Pixel>,
    selection_horizontal: Vec<Rgb565Pixel>,
    selection_vertical: Vec<Rgb565Pixel>,
    row_cache_epoch: u64,
    row_fingerprint_epoch: u64,
    row_fingerprint_cache: HashMap<usize, CachedArcadeRowFingerprint>,
    surface_y: usize,
    last_draw: Option<ArcadeListDrawKey>,
    last_filter_draw: Option<ArcadeFilterListDrawKey>,
    geometry: ArcadeListGeometry,
}

pub(crate) struct CachedArcadeRow {
    pub(crate) title: Arc<str>,
    pub(crate) is_new: bool,
    pub(crate) pixels: Vec<Rgb565Pixel>,
    pub(crate) last_used: u64,
}

struct CachedArcadeRowFingerprint {
    title: Arc<str>,
    is_new: bool,
    hash: u64,
    last_used: u64,
}

impl CachedArcadeRowFingerprint {
    fn matches(&self, game: &ArcadeGameEntry) -> bool {
        self.is_new == game.is_new && arc_str_eq(&self.title, &game.title)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ArcadeListDrawKey {
    len: usize,
    visual_px: i32,
    anchor_hash: u64,
    visible_hash: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArcadeListItem {
    pub(crate) title: String,
    pub(crate) count: Option<usize>,
    pub(crate) active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ArcadeFilterListDrawKey {
    len: usize,
    visual_px: i32,
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
        rect: DirtyRect,
    },
}

impl ArcadeListRenderer {
    pub(crate) fn new() -> Self {
        Self {
            title_font: ConsoleFont::new(ARCADE_LIST_FONT_PX),
            meta_font: ConsoleFont::new(ARCADE_LIST_META_FONT_PX),
            row_cache: HashMap::new(),
            surface: vec![ARCADE_LIST_BG_COLOR_565; ARCADE_LIST_W * ARCADE_LIST_H],
            band_scratch: Vec::new(),
            selection_invert_scratch: Vec::new(),
            selection_horizontal: Vec::new(),
            selection_vertical: Vec::new(),
            row_cache_epoch: 0,
            row_fingerprint_epoch: 0,
            row_fingerprint_cache: HashMap::new(),
            surface_y: 0,
            last_draw: None,
            last_filter_draw: None,
            geometry: ArcadeListGeometry::NORMAL,
        }
    }

    pub(crate) fn dirty_rect(&self) -> DirtyRect {
        self.geometry.dirty_rect()
    }

    pub(crate) fn set_geometry(&mut self, geometry: ArcadeListGeometry) {
        if self.geometry != geometry {
            self.geometry = geometry;
            self.last_draw = None;
            self.last_filter_draw = None;
            self.surface_y = 0;
        }
    }

    pub(crate) fn invalidate_presented_layer(&mut self) {
        self.last_draw = None;
        self.last_filter_draw = None;
        self.surface_y = 0;
    }

    pub(crate) fn draw(
        &mut self,
        games: ArcadeGameView<'_>,
        _selected: usize,
        visual_index: f32,
        force: bool,
    ) -> Option<ArcadeListUpdate> {
        self.last_filter_draw = None;
        let visual_px = arcade_visual_px(visual_index);
        let anchor = arcade_anchor_for_visual_px(games.len(), visual_px);
        let previous = self.last_draw;
        let anchor_hash = games
            .get(anchor)
            .map(|game| self.arcade_cached_game_hash(anchor, game))
            .unwrap_or(ARCADE_LIST_HASH_OFFSET);
        let same_position = previous.as_ref().is_some_and(|previous| {
            previous.len == games.len()
                && previous.visual_px == visual_px
                && previous.anchor_hash == anchor_hash
        });
        let visible_hash = if previous.is_none() || same_position {
            Some(self.arcade_visible_window_hash(games, visual_px))
        } else {
            None
        };
        let key = ArcadeListDrawKey {
            len: games.len(),
            visual_px,
            anchor_hash,
            visible_hash,
        };
        if !force && self.last_draw.as_ref() == Some(&key) {
            return None;
        }
        if force && self.last_draw.as_ref() == Some(&key) {
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
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
            self.draw_content_band(games, visual_px, 0, ARCADE_LIST_H);
        } else if content_delta == 0 {
        } else if content_delta.unsigned_abs() as usize >= ARCADE_LIST_H {
            self.surface_y = 0;
            self.draw_content_band(games, visual_px, 0, ARCADE_LIST_H);
        } else if content_delta < 0 {
            let d = content_delta.unsigned_abs() as usize;
            self.surface_y = (self.surface_y + d) % ARCADE_LIST_H;
            self.draw_content_band(games, visual_px, ARCADE_LIST_H - d, d);
        } else {
            let d = content_delta as usize;
            self.surface_y = (self.surface_y + ARCADE_LIST_H - d) % ARCADE_LIST_H;
            self.draw_content_band(games, visual_px, 0, d);
        }
        if force {
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        if previous.is_none() {
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        if !can_reuse_scrolled_surface {
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        if content_delta == 0 || content_delta.unsigned_abs() as usize >= ARCADE_LIST_H {
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        Some(ArcadeListUpdate::Scroll {
            delta_y: content_delta as isize,
            rect: self.dirty_rect(),
        })
    }

    pub(crate) fn draw_filter_items(
        &mut self,
        items: &[ArcadeListItem],
        _selected: usize,
        visual_index: f32,
        force: bool,
    ) -> Option<ArcadeListUpdate> {
        self.last_draw = None;
        let visual_px = arcade_visual_px(visual_index);
        let key = ArcadeFilterListDrawKey {
            len: items.len(),
            visual_px,
            visible_hash: arcade_filter_visible_window_hash(items, visual_px),
        };
        if !force && self.last_filter_draw.as_ref() == Some(&key) {
            return None;
        }
        let previous = self.last_filter_draw;
        self.last_filter_draw = Some(key);
        if force && previous.as_ref() == Some(&key) {
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        let content_delta = previous
            .as_ref()
            .map(|previous| previous.visual_px - key.visual_px)
            .unwrap_or(0);
        let can_reuse_scrolled_surface = previous
            .as_ref()
            .is_some_and(|previous| previous.len == key.len)
            && !previous.as_ref().is_some_and(|previous| {
                previous.len == key.len
                    && previous.visual_px == key.visual_px
                    && previous.visible_hash != key.visible_hash
            });
        if previous.is_none() || !can_reuse_scrolled_surface || items.is_empty() {
            self.surface_y = 0;
            self.draw_filter_content_band(items, visual_px, 0, ARCADE_LIST_H);
        } else if content_delta == 0 {
        } else if content_delta.unsigned_abs() as usize >= ARCADE_LIST_H {
            self.surface_y = 0;
            self.draw_filter_content_band(items, visual_px, 0, ARCADE_LIST_H);
        } else if content_delta < 0 {
            let d = content_delta.unsigned_abs() as usize;
            self.surface_y = (self.surface_y + d) % ARCADE_LIST_H;
            self.draw_filter_content_band(items, visual_px, ARCADE_LIST_H - d, d);
        } else {
            let d = content_delta as usize;
            self.surface_y = (self.surface_y + ARCADE_LIST_H - d) % ARCADE_LIST_H;
            self.draw_filter_content_band(items, visual_px, 0, d);
        }
        if force || previous.is_none() || !can_reuse_scrolled_surface {
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        if content_delta == 0 || content_delta.unsigned_abs() as usize >= ARCADE_LIST_H {
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        Some(ArcadeListUpdate::Scroll {
            delta_y: content_delta as isize,
            rect: self.dirty_rect(),
        })
    }

    pub(crate) fn selection_rect(&self) -> DirtyRect {
        let y = Self::centered_selection_y();
        DirtyRect {
            x0: self.geometry.x,
            y0: self.geometry.y + y,
            x1: self.geometry.x + ARCADE_LIST_W,
            y1: self.geometry.y + y + ARCADE_ROW_HEIGHT as usize,
        }
    }

    fn centered_selection_y() -> usize {
        let row_h = ARCADE_ROW_HEIGHT as usize;
        let visible_rows = (ARCADE_LIST_H / row_h).max(1);
        (visible_rows / 2) * row_h
    }

    fn draw_content_band(
        &mut self,
        games: ArcadeGameView<'_>,
        visual_px: i32,
        band_y: usize,
        band_h: usize,
    ) {
        if band_h == 0 || band_y >= ARCADE_LIST_H {
            return;
        }
        let band_h = band_h.min(ARCADE_LIST_H - band_y);
        if games.is_empty() {
            let mut band = std::mem::take(&mut self.band_scratch);
            band.resize(ARCADE_LIST_W * band_h, ARCADE_LIST_BG_COLOR);
            band.fill(ARCADE_LIST_BG_COLOR);
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
        self.fill_surface_band(band_y, band_h, ARCADE_LIST_BG_COLOR_565);
        let row_h = ARCADE_ROW_HEIGHT as isize;
        let Some((first, end)) = arcade_visible_window_range_px(games.len(), visual_px) else {
            return;
        };
        for idx in first..=end {
            let y = arcade_row_y(idx, visual_px);
            let clip_y0 = y.max(band_y as isize);
            let clip_y1 = (y + row_h).min((band_y + band_h) as isize);
            if clip_y1 <= clip_y0 {
                continue;
            }
            let Some(game) = games.get(idx) else {
                continue;
            };
            self.blit_cached_row_to_surface(band_h, band_y, game, idx, y);
        }
    }

    fn draw_filter_content_band(
        &mut self,
        items: &[ArcadeListItem],
        visual_px: i32,
        band_y: usize,
        band_h: usize,
    ) {
        if band_h == 0 || band_y >= ARCADE_LIST_H {
            return;
        }
        let band_h = band_h.min(ARCADE_LIST_H - band_y);
        self.fill_surface_band(band_y, band_h, ARCADE_LIST_BG_COLOR_565);
        if items.is_empty() {
            let mut band = std::mem::take(&mut self.band_scratch);
            band.resize(ARCADE_LIST_W * band_h, ARCADE_LIST_BG_COLOR);
            band.fill(ARCADE_LIST_BG_COLOR);
            self.meta_font.draw_text_clipped(
                &mut band,
                ARCADE_LIST_W,
                ARCADE_LIST_W,
                0,
                band_h,
                96,
                (ARCADE_LIST_H / 2).saturating_sub(band_y) as isize,
                "NO FILTERS",
                Pixel(0x00706080),
            );
            self.copy_band_to_surface(&band, band_y, band_h);
            self.band_scratch = band;
            return;
        }
        let row_h = ARCADE_ROW_HEIGHT as isize;
        let Some((first, end)) = arcade_visible_window_range_px(items.len(), visual_px) else {
            return;
        };
        for (idx, item) in items.iter().enumerate().take(end + 1).skip(first) {
            let y = arcade_row_y(idx, visual_px);
            let clip_y0 = y.max(band_y as isize);
            let clip_y1 = (y + row_h).min((band_y + band_h) as isize);
            if clip_y1 <= clip_y0 {
                continue;
            }
            let row = self.render_filter_row(item, idx);
            let copy_h = (clip_y1 - clip_y0) as usize;
            let src_y = (clip_y0 - y) as usize;
            for row_y in 0..copy_h {
                let src = (src_y + row_y) * ARCADE_LIST_W;
                let viewport_y = clip_y0 as usize + row_y;
                let dst_y = (self.surface_y + viewport_y) % ARCADE_LIST_H;
                let dst = dst_y * ARCADE_LIST_W;
                self.surface[dst..dst + ARCADE_LIST_W]
                    .copy_from_slice(&row[src..src + ARCADE_LIST_W]);
            }
        }
    }

    fn blit_cached_row_to_surface(
        &mut self,
        band_h: usize,
        band_y: usize,
        game: &ArcadeGameEntry,
        idx: usize,
        y: isize,
    ) {
        let needs_render = self.row_cache.get(&idx).is_none_or(|cached| {
            !arc_str_eq(&cached.title, &game.title) || cached.is_new != game.is_new
        });
        if needs_render {
            if self.row_cache.len() >= ARCADE_ROW_CACHE_MAX {
                prune_arcade_row_cache(&mut self.row_cache);
            }
            let row = self.render_row(game.title.as_ref(), game.is_new, idx);
            let last_used = self.next_row_cache_epoch();
            self.row_cache.insert(
                idx,
                CachedArcadeRow {
                    title: Arc::clone(&game.title),
                    is_new: game.is_new,
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

    fn next_row_fingerprint_epoch(&mut self) -> u64 {
        self.row_fingerprint_epoch = self.row_fingerprint_epoch.wrapping_add(1);
        self.row_fingerprint_epoch
    }

    fn arcade_visible_window_hash(&mut self, games: ArcadeGameView<'_>, visual_px: i32) -> u64 {
        let mut hash = ARCADE_LIST_HASH_OFFSET;
        let Some((first, end)) = arcade_visible_window_range_px(games.len(), visual_px) else {
            return hash;
        };
        arcade_hash_usize(&mut hash, first);
        arcade_hash_usize(&mut hash, end);
        for idx in first..=end {
            arcade_hash_usize(&mut hash, idx);
            let row_hash = games
                .get(idx)
                .map(|game| self.arcade_cached_game_hash(idx, game))
                .unwrap_or(ARCADE_LIST_HASH_OFFSET);
            arcade_hash_u64(&mut hash, row_hash);
        }
        hash
    }

    fn arcade_cached_game_hash(&mut self, idx: usize, game: &ArcadeGameEntry) -> u64 {
        let last_used = self.next_row_fingerprint_epoch();
        if let Some(cached) = self.row_fingerprint_cache.get_mut(&idx) {
            if cached.matches(game) {
                cached.last_used = last_used;
                return cached.hash;
            }
        }
        if self.row_fingerprint_cache.len() >= ARCADE_ROW_FINGERPRINT_CACHE_MAX {
            prune_arcade_row_fingerprint_cache(&mut self.row_fingerprint_cache);
        }
        let hash = arcade_game_hash(game);
        self.row_fingerprint_cache.insert(
            idx,
            CachedArcadeRowFingerprint {
                title: Arc::clone(&game.title),
                is_new: game.is_new,
                hash,
                last_used,
            },
        );
        hash
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
        disp: &mut MappedRgb565Framebuffer,
        redraw_selection_frame: bool,
    ) {
        for (viewport_y, h) in ARCADE_LIST_LAYER_COPY_BANDS {
            self.copy_viewport_band_to_target(target, disp, viewport_y, h);
        }
        if redraw_selection_frame {
            self.copy_selection_frame_to_target(target, disp);
        }
    }

    fn copy_viewport_band_to_target(
        &mut self,
        target: &mut UiFrameTarget,
        disp: &mut MappedRgb565Framebuffer,
        viewport_y: usize,
        h: usize,
    ) {
        if h == 0 || viewport_y >= ARCADE_LIST_H {
            return;
        }
        let h = h.min(ARCADE_LIST_H - viewport_y);
        for_each_arcade_list_present_segment(viewport_y, h, |kind, x, y, w, h| match kind {
            ArcadeListPresentKind::Normal => {
                self.copy_surface_rect_to_target(target, disp, x, y, w, h);
            }
            ArcadeListPresentKind::Inverted => {
                if arcade_selection_inversion_enabled() {
                    self.copy_inverted_surface_rect_to_target(target, disp, x, y, w, h);
                } else {
                    self.copy_surface_rect_to_target(target, disp, x, y, w, h);
                }
            }
        });
    }

    fn copy_surface_rect_to_target(
        &mut self,
        target: &mut UiFrameTarget,
        disp: &mut MappedRgb565Framebuffer,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % ARCADE_LIST_H;
            let copy_h = (h - copied).min(ARCADE_LIST_H - src_y);
            self.copy_surface_chunk_to_target(target, disp, x, viewport_y + copied, w, copy_h);
            copied += copy_h;
        }
    }

    fn copy_surface_chunk_to_target(
        &mut self,
        target: &mut UiFrameTarget,
        disp: &mut MappedRgb565Framebuffer,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        if w == 0 || h == 0 {
            return;
        }
        let src_y = (self.surface_y + viewport_y) % ARCADE_LIST_H;
        if x == 0 && w == ARCADE_LIST_W {
            let src = src_y * ARCADE_LIST_W;
            target.present_rect_565(
                disp,
                self.geometry.x,
                self.geometry.y + viewport_y,
                ARCADE_LIST_W,
                h,
                &self.surface[src..src + h * ARCADE_LIST_W],
            );
            return;
        }
        target.present_rect_565_strided(
            disp,
            self.geometry.x + x,
            self.geometry.y + viewport_y,
            w,
            h,
            &self.surface,
            ARCADE_LIST_W,
            x,
            src_y,
        );
    }

    fn copy_inverted_surface_rect_to_target(
        &mut self,
        target: &mut UiFrameTarget,
        disp: &mut MappedRgb565Framebuffer,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % ARCADE_LIST_H;
            let copy_h = (h - copied).min(ARCADE_LIST_H - src_y);
            let target_x = self.geometry.x + x;
            let target_y = self.geometry.y + viewport_y + copied;
            let inverted = self.prepare_inverted_surface_chunk(x, viewport_y + copied, w, copy_h);
            target.present_rect_565(disp, target_x, target_y, w, copy_h, inverted);
            copied += copy_h;
        }
    }

    fn prepare_inverted_surface_chunk(
        &mut self,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) -> &[Rgb565Pixel] {
        self.selection_invert_scratch
            .resize(w.saturating_mul(h), Rgb565Pixel(0));
        if w == 0 || h == 0 {
            return &self.selection_invert_scratch;
        }
        let src_y = (self.surface_y + viewport_y) % ARCADE_LIST_H;
        for row in 0..h {
            let src = (src_y + row) * ARCADE_LIST_W + x;
            let dst = row * w;
            for col in 0..w {
                self.selection_invert_scratch[dst + col] =
                    selected_aperture_pixel(self.surface[src + col]);
            }
        }
        &self.selection_invert_scratch
    }

    fn copy_selection_frame_to_target(
        &mut self,
        target: &mut UiFrameTarget,
        disp: &mut MappedRgb565Framebuffer,
    ) {
        let rect = self.selection_rect();
        let color = ARCADE_SELECTION_FRAME_COLOR;
        let thickness = ARCADE_SELECTION_FRAME_THICKNESS;
        let h = rect.y1.saturating_sub(rect.y0).min(ARCADE_LIST_H);
        self.selection_horizontal
            .resize(ARCADE_LIST_W * thickness, color);
        self.selection_horizontal.fill(color);
        target.present_rect_565(
            disp,
            rect.x0,
            rect.y0,
            ARCADE_LIST_W,
            thickness,
            &self.selection_horizontal,
        );
        target.present_rect_565(
            disp,
            rect.x0,
            rect.y1.saturating_sub(thickness),
            ARCADE_LIST_W,
            thickness,
            &self.selection_horizontal,
        );
        self.selection_vertical.resize(thickness * h, color);
        self.selection_vertical.fill(color);
        target.present_rect_565(
            disp,
            rect.x0,
            rect.y0,
            thickness,
            h,
            &self.selection_vertical,
        );
        target.present_rect_565(
            disp,
            rect.x1.saturating_sub(thickness),
            rect.y0,
            thickness,
            h,
            &self.selection_vertical,
        );
    }

    fn render_row(&mut self, title: &str, is_new: bool, idx: usize) -> Vec<Rgb565Pixel> {
        let mut row = vec![Pixel(0); ARCADE_LIST_W * ARCADE_ROW_HEIGHT as usize];
        draw_arcade_row_background(&mut row, idx);
        let title = clipped_title(title, if is_new { 26 } else { 33 });
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
        if is_new {
            draw_new_badge(&mut row, &mut self.meta_font);
        }
        row.into_iter().map(pixel_to_rgb565).collect()
    }

    fn render_filter_row(&mut self, item: &ArcadeListItem, idx: usize) -> Vec<Rgb565Pixel> {
        let mut row = vec![Pixel(0); ARCADE_LIST_W * ARCADE_ROW_HEIGHT as usize];
        draw_arcade_row_background(&mut row, idx);
        let title = clipped_title(&item.title, if item.count.is_some() { 29 } else { 33 });
        let gradient = if item.active {
            ARCADE_FILTER_ACTIVE_GRADIENT
        } else {
            ARCADE_TITLE_GRADIENT
        };
        self.title_font.draw_text_clipped_gradient(
            &mut row,
            ARCADE_LIST_W,
            ARCADE_LIST_W,
            0,
            ARCADE_ROW_HEIGHT as usize,
            12,
            30,
            &title,
            gradient,
        );
        if let Some(count) = item.count {
            let count = count.to_string();
            self.meta_font.draw_text_clipped(
                &mut row,
                ARCADE_LIST_W,
                ARCADE_LIST_W,
                0,
                ARCADE_ROW_HEIGHT as usize,
                (ARCADE_LIST_W - 60) as isize,
                29,
                &count,
                Pixel(0x00706080),
            );
        }
        row.into_iter().map(pixel_to_rgb565).collect()
    }
}

fn draw_new_badge(row: &mut [Pixel], font: &mut ConsoleFont) {
    let x = ARCADE_LIST_W.saturating_sub(58);
    let y = 14usize;
    let w = 42usize;
    let h = 18usize;
    for dy in 0..h {
        let row_y = y + dy;
        if row_y >= ARCADE_ROW_HEIGHT as usize {
            break;
        }
        let start = row_y * ARCADE_LIST_W + x;
        let end = (start + w).min((row_y + 1) * ARCADE_LIST_W);
        row[start..end].fill(ARCADE_NEW_BADGE_FILL);
    }
    font.draw_text_clipped(
        row,
        ARCADE_LIST_W,
        ARCADE_LIST_W,
        0,
        ARCADE_ROW_HEIGHT as usize,
        x as isize + 9,
        y as isize + 12,
        "NEW",
        ARCADE_NEW_BADGE_TEXT,
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArcadeListPresentKind {
    Normal,
    Inverted,
}

pub(crate) fn for_each_arcade_list_present_segment(
    viewport_y: usize,
    h: usize,
    emit: impl FnMut(ArcadeListPresentKind, usize, usize, usize, usize),
) {
    if h == 0 || viewport_y >= ARCADE_LIST_H {
        return;
    }
    let y0 = viewport_y;
    let y1 = (viewport_y + h).min(ARCADE_LIST_H);

    let selection_y = ArcadeListRenderer::centered_selection_y();
    let selection_bottom = selection_y + ARCADE_ROW_HEIGHT as usize;
    let inner_top = selection_y + ARCADE_SELECTION_FRAME_THICKNESS;
    let inner_bottom = selection_bottom.saturating_sub(ARCADE_SELECTION_FRAME_THICKNESS);
    let mut emit = emit;

    emit_row_overlap(
        y0..y1,
        0..selection_y,
        0,
        ARCADE_LIST_W,
        ArcadeListPresentKind::Normal,
        &mut emit,
    );
    emit_row_overlap(
        y0..y1,
        inner_top..inner_bottom,
        ARCADE_SELECTION_FRAME_THICKNESS,
        ARCADE_LIST_W - ARCADE_SELECTION_FRAME_THICKNESS * 2,
        ArcadeListPresentKind::Inverted,
        &mut emit,
    );
    emit_row_overlap(
        y0..y1,
        selection_bottom..ARCADE_LIST_H,
        0,
        ARCADE_LIST_W,
        ArcadeListPresentKind::Normal,
        &mut emit,
    );
}

fn emit_row_overlap(
    viewport: Range<usize>,
    band: Range<usize>,
    x: usize,
    w: usize,
    kind: ArcadeListPresentKind,
    emit: &mut impl FnMut(ArcadeListPresentKind, usize, usize, usize, usize),
) {
    let out_y0 = viewport.start.max(band.start);
    let out_y1 = viewport.end.min(band.end);
    if out_y1 > out_y0 && w > 0 {
        emit(kind, x, out_y0, w, out_y1 - out_y0);
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

fn prune_arcade_row_fingerprint_cache(row_cache: &mut HashMap<usize, CachedArcadeRowFingerprint>) {
    if row_cache.len() < ARCADE_ROW_FINGERPRINT_CACHE_MAX {
        return;
    }
    let keep = ARCADE_ROW_FINGERPRINT_CACHE_PRUNE_TO.min(row_cache.len());
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

#[cfg(test)]
fn arcade_anchor_hash(game: Option<&ArcadeGameEntry>) -> u64 {
    let mut hash = ARCADE_LIST_HASH_OFFSET;
    if let Some(game) = game {
        arcade_hash_game(&mut hash, game);
    }
    hash
}

#[cfg(test)]
fn arcade_visible_window_hash(games: ArcadeGameView<'_>, visual_index: f32) -> u64 {
    let mut hash = ARCADE_LIST_HASH_OFFSET;
    let Some((first, end)) =
        arcade_visible_window_range_px(games.len(), arcade_visual_px(visual_index))
    else {
        return hash;
    };
    arcade_hash_usize(&mut hash, first);
    arcade_hash_usize(&mut hash, end);
    for idx in first..=end {
        arcade_hash_usize(&mut hash, idx);
        if let Some(game) = games.get(idx) {
            arcade_hash_game(&mut hash, game);
        }
    }
    hash
}

fn arcade_filter_visible_window_hash(items: &[ArcadeListItem], visual_px: i32) -> u64 {
    let mut hash = ARCADE_LIST_HASH_OFFSET;
    let Some((first, end)) = arcade_visible_window_range_px(items.len(), visual_px) else {
        return hash;
    };
    arcade_hash_usize(&mut hash, first);
    arcade_hash_usize(&mut hash, end);
    for (idx, item) in items.iter().enumerate().take(end + 1).skip(first) {
        arcade_hash_usize(&mut hash, idx);
        arcade_hash_bytes(&mut hash, item.title.as_bytes());
        arcade_hash_usize(&mut hash, item.count.unwrap_or(usize::MAX));
        arcade_hash_bytes(&mut hash, &[item.active as u8]);
    }
    hash
}

fn arcade_game_hash(game: &ArcadeGameEntry) -> u64 {
    let mut hash = ARCADE_LIST_HASH_OFFSET;
    arcade_hash_game(&mut hash, game);
    hash
}

fn arcade_visual_px(visual_index: f32) -> i32 {
    if !visual_index.is_finite() {
        return 0;
    }
    (visual_index * ARCADE_ROW_HEIGHT as f32).round().max(0.0) as i32
}

fn arcade_anchor_for_visual_px(len: usize, visual_px: i32) -> usize {
    if len == 0 {
        return 0;
    }
    let row_h = ARCADE_ROW_HEIGHT.max(1);
    let anchor = (visual_px.max(0) + row_h / 2).div_euclid(row_h);
    (anchor as usize).min(len - 1)
}

fn arcade_row_y(idx: usize, visual_px: i32) -> isize {
    ArcadeListRenderer::centered_selection_y() as isize + idx as isize * ARCADE_ROW_HEIGHT as isize
        - visual_px.max(0) as isize
}

fn arcade_visible_window_range_px(len: usize, visual_px: i32) -> Option<(usize, usize)> {
    if len == 0 {
        return None;
    }
    let row_h = ARCADE_ROW_HEIGHT.max(1);
    let visual_px = visual_px.max(0);
    let floor = visual_px.div_euclid(row_h);
    let ceil = (visual_px + row_h - 1).div_euclid(row_h);
    let first = (floor as isize - 7).max(0) as usize;
    let last = (ceil as isize + 8).max(0) as usize;
    Some((first.min(len - 1), last.min(len - 1)))
}

fn arcade_hash_game(hash: &mut u64, game: &ArcadeGameEntry) {
    arcade_hash_bytes(hash, game.title.as_bytes());
    arcade_hash_bytes(hash, &[game.is_new as u8]);
}

fn arcade_hash_usize(hash: &mut u64, value: usize) {
    arcade_hash_bytes(hash, &(value as u64).to_le_bytes());
}

fn arcade_hash_u64(hash: &mut u64, value: u64) {
    arcade_hash_bytes(hash, &value.to_le_bytes());
}

fn arcade_hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= *byte as u64;
        *hash = hash.wrapping_mul(ARCADE_LIST_HASH_PRIME);
    }
    *hash ^= 0xff;
    *hash = hash.wrapping_mul(ARCADE_LIST_HASH_PRIME);
}

fn arc_str_eq(left: &Arc<str>, right: &Arc<str>) -> bool {
    Arc::ptr_eq(left, right) || left.as_ref() == right.as_ref()
}

pub(crate) fn draw_arcade_row_background(row: &mut [Pixel], idx: usize) {
    let bg = if idx.is_multiple_of(2) {
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

const fn rgb565_from_rgb888(r: u8, g: u8, b: u8) -> Rgb565Pixel {
    Rgb565Pixel(((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3))
}

fn invert_rgb565(pixel: Rgb565Pixel) -> Rgb565Pixel {
    Rgb565Pixel(!pixel.0)
}

fn selected_aperture_pixel(pixel: Rgb565Pixel) -> Rgb565Pixel {
    if is_arcade_row_background_pixel(pixel) {
        ARCADE_SELECTION_FILL_COLOR_565
    } else {
        invert_rgb565(pixel)
    }
}

fn is_arcade_row_background_pixel(pixel: Rgb565Pixel) -> bool {
    matches!(
        pixel,
        ARCADE_LIST_BG_COLOR_565
            | ARCADE_LIST_ALT_BG_COLOR_565
            | ARCADE_LIST_ROW_BORDER_COLOR_565
            | ARCADE_NEW_BADGE_FILL_565
    )
}

fn arcade_selection_inversion_enabled() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| {
        !matches!(
            std::env::var("MISTER_ARCADE_SELECTION_INVERT")
                .ok()
                .as_deref()
                .map(str::trim)
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("0" | "false" | "off" | "no")
        )
    })
}

fn copy_pixel_to_rgb565_row(src: &[Pixel], dst: &mut [Rgb565Pixel]) {
    for (src, dst) in src.iter().zip(dst.iter_mut()) {
        *dst = pixel_to_rgb565(*src);
    }
}

fn clipped_title(title: &str, max_chars: usize) -> Cow<'_, str> {
    if max_chars == 0 {
        return Cow::Borrowed("");
    }
    let mut chars = title.char_indices();
    for _ in 0..max_chars {
        if chars.next().is_none() {
            return Cow::Borrowed(title);
        }
    }
    let Some((cut, _)) = chars.next() else {
        return Cow::Borrowed(title);
    };
    let mut out = String::with_capacity(cut + 3);
    out.push_str(&title[..cut]);
    out.push_str("...");
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::arcade_game;

    fn game(system_id: &str, path: &str, title: &str) -> ArcadeGameEntry {
        arcade_game(title).system_id(system_id).path(path).build()
    }

    fn games(system_id: &str, count: usize) -> Vec<ArcadeGameEntry> {
        (0..count)
            .map(|idx| {
                game(
                    system_id,
                    &format!("/media/fat/games/{system_id}/{idx}.rom"),
                    &format!("Game {idx}"),
                )
            })
            .collect()
    }

    fn surface_in_viewport_order(renderer: &ArcadeListRenderer) -> Vec<Rgb565Pixel> {
        let mut pixels = Vec::with_capacity(ARCADE_LIST_W * ARCADE_LIST_H);
        for y in 0..ARCADE_LIST_H {
            let src_y = (renderer.surface_y + y) % ARCADE_LIST_H;
            let src = src_y * ARCADE_LIST_W;
            pixels.extend_from_slice(&renderer.surface[src..src + ARCADE_LIST_W]);
        }
        pixels
    }

    #[test]
    fn search_geometry_right_aligns_to_render_width() {
        assert_eq!(
            ArcadeListGeometry::search_for_render_w(960),
            ArcadeListGeometry {
                x: 442,
                y: ARCADE_SEARCH_LIST_Y,
            }
        );
        assert_eq!(
            ArcadeListGeometry::search_for_render_w(1280),
            ArcadeListGeometry {
                x: 762,
                y: ARCADE_SEARCH_LIST_Y,
            }
        );
    }

    #[test]
    fn arcade_anchor_hash_tracks_visible_row_fields_only() {
        let base = game("arcade", "/media/fat/_Arcade/a.mra", "Alpha");

        assert_eq!(
            arcade_anchor_hash(Some(&base)),
            arcade_anchor_hash(Some(&game("console", "/media/fat/_Arcade/a.mra", "Alpha")))
        );
        assert_eq!(
            arcade_anchor_hash(Some(&base)),
            arcade_anchor_hash(Some(&game("arcade", "/media/fat/_Arcade/b.mra", "Alpha")))
        );
        assert_ne!(
            arcade_anchor_hash(Some(&base)),
            arcade_anchor_hash(Some(&game("arcade", "/media/fat/_Arcade/a.mra", "Beta")))
        );
        let mut with_badge = base.clone();
        with_badge.is_new = true;
        assert_ne!(
            arcade_anchor_hash(Some(&base)),
            arcade_anchor_hash(Some(&with_badge))
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
                arcade_game(format!("Game {idx}"))
                    .path(format!("/media/fat/_Arcade/{idx}.mra"))
                    .build()
            })
            .collect::<Vec<_>>();

        assert!(matches!(
            renderer.draw(ArcadeGameView::contiguous(&games), 7, 7.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
        assert!(renderer
            .draw(ArcadeGameView::contiguous(&games), 7, 7.0, false)
            .is_none());

        games[3].title = "Changed visible row".into();

        assert!(matches!(
            renderer.draw(ArcadeGameView::contiguous(&games), 7, 7.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
    }

    #[test]
    fn forced_present_reuses_surface_when_draw_key_is_unchanged() {
        let mut renderer = ArcadeListRenderer::new();
        let games = games("arcade", 20);

        assert!(matches!(
            renderer.draw(ArcadeGameView::contiguous(&games), 7, 7.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
        let before = renderer.surface.clone();

        assert!(matches!(
            renderer.draw(ArcadeGameView::contiguous(&games), 7, 7.0, true),
            Some(ArcadeListUpdate::Full(_))
        ));

        assert_eq!(renderer.surface, before);
    }

    #[test]
    fn scrolled_settled_surface_matches_fresh_full_redraw() {
        let games = games("intellivision", 20);
        let mut scrolled = ArcadeListRenderer::new();
        assert!(matches!(
            scrolled.draw(ArcadeGameView::contiguous(&games), 0, 0.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));

        for visual_px in 1..=ARCADE_ROW_HEIGHT {
            let visual_index = visual_px as f32 / ARCADE_ROW_HEIGHT as f32;
            scrolled.draw(ArcadeGameView::contiguous(&games), 1, visual_index, false);
        }
        assert_eq!(scrolled.surface_y, ARCADE_ROW_HEIGHT as usize);
        let scrolled_pixels = surface_in_viewport_order(&scrolled);

        let mut fresh = ArcadeListRenderer::new();
        assert!(matches!(
            fresh.draw(ArcadeGameView::contiguous(&games), 1, 1.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
        let fresh_pixels = surface_in_viewport_order(&fresh);

        assert_eq!(scrolled_pixels, fresh_pixels);
    }

    #[test]
    fn scrolled_settled_surface_matches_fresh_full_redraw_after_upward_motion() {
        let games = games("intellivision", 20);
        let mut scrolled = ArcadeListRenderer::new();
        assert!(matches!(
            scrolled.draw(ArcadeGameView::contiguous(&games), 2, 2.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));

        for visual_px in (ARCADE_ROW_HEIGHT..2 * ARCADE_ROW_HEIGHT).rev() {
            let visual_index = visual_px as f32 / ARCADE_ROW_HEIGHT as f32;
            scrolled.draw(ArcadeGameView::contiguous(&games), 1, visual_index, false);
        }
        let scrolled_pixels = surface_in_viewport_order(&scrolled);

        let mut fresh = ArcadeListRenderer::new();
        assert!(matches!(
            fresh.draw(ArcadeGameView::contiguous(&games), 1, 1.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
        let fresh_pixels = surface_in_viewport_order(&fresh);

        assert_eq!(scrolled_pixels, fresh_pixels);
    }

    #[test]
    fn arcade_row_title_uses_gradient_pixels() {
        let mut renderer = ArcadeListRenderer::new();
        let row = renderer.render_row("MAGIK", false, 0);
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

    #[test]
    fn arcade_layer_copy_bands_cover_full_surface_without_fade_split() {
        assert_eq!(ARCADE_LIST_LAYER_COPY_BANDS, [(0, ARCADE_LIST_H)]);
    }

    #[test]
    fn selection_frame_stays_fixed_while_content_scrolls() {
        let mut renderer = ArcadeListRenderer::new();
        let games = (0..4)
            .map(|idx| {
                game(
                    "intellivision",
                    &format!("/media/fat/games/Intellivision/{idx}.int"),
                    &format!("Game {idx}"),
                )
            })
            .collect::<Vec<_>>();

        assert!(matches!(
            renderer.draw(ArcadeGameView::contiguous(&games), 1, 0.5, false),
            Some(ArcadeListUpdate::Full(_))
        ));

        let rect = renderer.selection_rect();
        assert_eq!(
            rect.y0,
            ARCADE_LIST_Y + ArcadeListRenderer::centered_selection_y()
        );
        assert_eq!(rect.y1 - rect.y0, ARCADE_ROW_HEIGHT as usize);

        assert!(matches!(
            renderer.draw(ArcadeGameView::contiguous(&games), 2, 1.25, false),
            Some(ArcadeListUpdate::Scroll { .. })
        ));
        let rect = renderer.selection_rect();
        assert_eq!(
            rect.y0,
            ARCADE_LIST_Y + ArcadeListRenderer::centered_selection_y()
        );
    }

    #[test]
    fn arcade_present_segments_invert_selected_row_inner_and_skip_frame_pixels() {
        let mut segments = Vec::new();

        for_each_arcade_list_present_segment(0, ARCADE_LIST_H, |kind, x, y, w, h| {
            segments.push((kind, x, y, w, h));
        });

        assert_eq!(
            segments,
            vec![
                (ArcadeListPresentKind::Normal, 0, 0, ARCADE_LIST_W, 240),
                (
                    ArcadeListPresentKind::Inverted,
                    ARCADE_SELECTION_FRAME_THICKNESS,
                    243,
                    ARCADE_LIST_W - ARCADE_SELECTION_FRAME_THICKNESS * 2,
                    42
                ),
                (ArcadeListPresentKind::Normal, 0, 288, ARCADE_LIST_W, 192),
            ]
        );

        let copied_px = segments.iter().map(|(_, _, _, w, h)| w * h).sum::<usize>();
        let skipped_px = ARCADE_LIST_W * ARCADE_LIST_H - copied_px;
        let frame_px = ARCADE_LIST_W * ARCADE_SELECTION_FRAME_THICKNESS * 2
            + (ARCADE_ROW_HEIGHT as usize - ARCADE_SELECTION_FRAME_THICKNESS * 2)
                * ARCADE_SELECTION_FRAME_THICKNESS
                * 2;

        assert_eq!(skipped_px, frame_px);
    }

    #[test]
    fn arcade_present_segments_keep_fixed_selection_aperture_for_partial_bands() {
        let mut segments = Vec::new();

        for_each_arcade_list_present_segment(250, 20, |kind, x, y, w, h| {
            segments.push((kind, x, y, w, h));
        });

        assert_eq!(
            segments,
            vec![(
                ArcadeListPresentKind::Inverted,
                ARCADE_SELECTION_FRAME_THICKNESS,
                250,
                ARCADE_LIST_W - ARCADE_SELECTION_FRAME_THICKNESS * 2,
                20
            )]
        );
    }

    #[test]
    fn rgb565_inversion_flips_all_color_bits() {
        assert_eq!(invert_rgb565(Rgb565Pixel(0xffff)), Rgb565Pixel(0x0000));
        assert_eq!(invert_rgb565(Rgb565Pixel(0x0000)), Rgb565Pixel(0xffff));
        assert_eq!(invert_rgb565(Rgb565Pixel(0x1234)), Rgb565Pixel(!0x1234));
    }

    #[test]
    fn selected_aperture_uses_fixed_fill_for_row_chrome_and_inverts_foreground() {
        assert_eq!(
            selected_aperture_pixel(ARCADE_LIST_BG_COLOR_565),
            ARCADE_SELECTION_FILL_COLOR_565
        );
        assert_eq!(
            selected_aperture_pixel(ARCADE_LIST_ALT_BG_COLOR_565),
            ARCADE_SELECTION_FILL_COLOR_565
        );
        assert_eq!(
            selected_aperture_pixel(ARCADE_LIST_ROW_BORDER_COLOR_565),
            ARCADE_SELECTION_FILL_COLOR_565
        );
        assert_eq!(
            selected_aperture_pixel(ARCADE_NEW_BADGE_FILL_565),
            ARCADE_SELECTION_FILL_COLOR_565
        );
        assert_eq!(
            selected_aperture_pixel(rgb565_from_rgb888(0xff, 0xf6, 0xff)),
            invert_rgb565(rgb565_from_rgb888(0xff, 0xf6, 0xff))
        );
    }

    #[test]
    fn preparing_inverted_selection_chunk_flattens_chrome_without_mutating_surface() {
        let mut renderer = ArcadeListRenderer::new();
        for (idx, pixel) in renderer.surface.iter_mut().enumerate() {
            *pixel = Rgb565Pixel(idx as u16);
        }
        renderer.surface_y = 5;
        let x = ARCADE_SELECTION_FRAME_THICKNESS;
        let y = ArcadeListRenderer::centered_selection_y() + ARCADE_SELECTION_FRAME_THICKNESS;
        let w = 4;
        let h = 2;
        let src_y = (renderer.surface_y + y) % ARCADE_LIST_H;
        let src = src_y * ARCADE_LIST_W + x;
        renderer.surface[src] = ARCADE_LIST_BG_COLOR_565;
        renderer.surface[src + 1] = ARCADE_LIST_ALT_BG_COLOR_565;
        renderer.surface[src + 2] = ARCADE_LIST_ROW_BORDER_COLOR_565;
        renderer.surface[src + 3] = rgb565_from_rgb888(0xff, 0xf6, 0xff);
        let before = renderer.surface.clone();

        let inverted = renderer.prepare_inverted_surface_chunk(x, y, w, h).to_vec();

        assert_eq!(renderer.surface, before);
        assert_eq!(inverted[0], ARCADE_SELECTION_FILL_COLOR_565);
        assert_eq!(inverted[1], ARCADE_SELECTION_FILL_COLOR_565);
        assert_eq!(inverted[2], ARCADE_SELECTION_FILL_COLOR_565);
        assert_eq!(inverted[3], invert_rgb565(before[src + 3]));
        for row in 0..h {
            let src_y = (renderer.surface_y + y + row) % ARCADE_LIST_H;
            for col in 0..w {
                let src = before[src_y * ARCADE_LIST_W + x + col];
                assert_eq!(inverted[row * w + col], selected_aperture_pixel(src));
            }
        }
    }

    #[test]
    fn row_cache_prune_keeps_recent_rows() {
        let mut cache = HashMap::new();
        for idx in 0..ARCADE_ROW_CACHE_MAX {
            cache.insert(
                idx,
                CachedArcadeRow {
                    title: format!("Game {idx}").into(),
                    is_new: false,
                    pixels: Vec::new(),
                    last_used: idx as u64,
                },
            );
        }

        prune_arcade_row_cache(&mut cache);

        assert_eq!(cache.len(), ARCADE_ROW_CACHE_PRUNE_TO);
        assert!(cache.values().all(|row| row.last_used >= 32));
        assert!(cache.contains_key(&(ARCADE_ROW_CACHE_MAX - 1)));
    }

    #[test]
    fn redraws_when_visible_row_new_badge_changes() {
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
            renderer.draw(ArcadeGameView::contiguous(&games), 7, 7.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
        assert!(renderer
            .draw(ArcadeGameView::contiguous(&games), 7, 7.0, false)
            .is_none());

        games[3].is_new = true;

        assert!(matches!(
            renderer.draw(ArcadeGameView::contiguous(&games), 7, 7.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
    }

    fn rgb565_luma(pixel: Rgb565Pixel) -> u32 {
        let value = pixel.0 as u32;
        let r = ((value >> 11) & 0x1f) << 3;
        let g = ((value >> 5) & 0x3f) << 2;
        let b = (value & 0x1f) << 3;
        r * 30 + g * 59 + b * 11
    }
}
