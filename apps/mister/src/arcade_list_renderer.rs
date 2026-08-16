// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use crate::arcade_catalog::{
    ARCADE_LIST_VISIBLE_H, ARCADE_ROW_HEIGHT, ArcadeGameEntry, ArcadeGameView,
};
use crate::bitmap_text::{ConsoleFont, ConsoleTypeface, TextGradient};
use crate::framebuffer::mapped::{MappedRgb565Framebuffer, Pixel, pixel_to_rgb565};
use crate::framebuffer::present::{copy_dense_rect_565, copy_strided_rect_565};
use crate::framebuffer::scanout_slots::ScanoutSlotsRgb565Framebuffer;
use crate::framebuffer::target::{DirtyRect, UiFrameTarget};
use crate::ui_display::{
    CrtContentRect, CrtFontFamily, CrtUiMetrics, ResolvedOutputRoute, UiDisplay,
};
use mister_magik_framebuffer_scenes::{OutputRotation, Rgb565OutputLayout, Rgb565SurfaceMut};
use slint::platform::software_renderer::Rgb565Pixel;

pub const ARCADE_LIST_X: usize = 8;
pub const ARCADE_LIST_Y: usize = 56;
// Wider than the half-screen pane on purpose: the list can borrow boundary
// space while the preview stays centered in the remaining black area.
pub const ARCADE_LIST_W: usize = 510;
pub const ARCADE_SEARCH_LIST_W: usize = 464;
pub const ARCADE_LIST_H: usize = ARCADE_LIST_VISIBLE_H as usize;
pub const ARCADE_SEARCH_LIST_Y: usize = 56;
pub const ARCADE_LIST_FONT_PX: f32 = 16.0;
pub const ARCADE_LIST_META_FONT_PX: f32 = 8.0;
pub const ARCADE_LIST_BG_COLOR: Pixel = Pixel(0x001a1424);
pub const ARCADE_LIST_BG_COLOR_565: Rgb565Pixel = rgb565_from_rgb888(0x1a, 0x14, 0x24);
const ARCADE_LIST_ALT_BG_COLOR_565: Rgb565Pixel = rgb565_from_rgb888(0x15, 0x0f, 0x20);
const ARCADE_LIST_ROW_BORDER_COLOR_565: Rgb565Pixel = rgb565_from_rgb888(0x25, 0x1c, 0x34);
const ARCADE_SELECTION_FILL_COLOR_565: Rgb565Pixel = rgb565_from_rgb888(0xe7, 0xe3, 0xec);
pub const ARCADE_TITLE_GRADIENT: TextGradient =
    TextGradient::new(Pixel(0x00fff6ff), Pixel(0x00dbd1e6), Pixel(0x00938a9b));
pub const ARCADE_FILTER_ACTIVE_GRADIENT: TextGradient =
    TextGradient::new(Pixel(0x0006d6a0), Pixel(0x0005b98a), Pixel(0x00047764));
pub const ARCADE_ROW_CACHE_MAX: usize = 128;
const ARCADE_ROW_CACHE_PRUNE_TO: usize = 96;
const ARCADE_ROW_FINGERPRINT_CACHE_MAX: usize = 512;
const ARCADE_ROW_FINGERPRINT_CACHE_PRUNE_TO: usize = 384;
const ARCADE_LIST_LAYER_COPY_BANDS: [(usize, usize); 1] = [(0, ARCADE_LIST_H)];
const ARCADE_HDMI_SELECTION_FRAME_THICKNESS: usize = 3;
const ARCADE_SELECTION_FRAME_COLOR: Rgb565Pixel = rgb565_from_rgb888(0x06, 0xd6, 0xa0);
static REQUESTED_FILTER_CONTENT_HASH: AtomicU64 = AtomicU64::new(0);
static RENDERED_FILTER_CONTENT_HASH: AtomicU64 = AtomicU64::new(0);
const ARCADE_NEW_BADGE_FILL: Pixel = Pixel(0x0006d6a0);
const ARCADE_NEW_BADGE_FILL_565: Rgb565Pixel = rgb565_from_rgb888(0x06, 0xd6, 0xa0);
const ARCADE_NEW_BADGE_TEXT: Pixel = Pixel(0x00120d1a);

#[derive(Clone, Copy)]
struct ArcadeListStyle {
    row_height: i32,
    scroll_quantum_y: i32,
    separator_top: usize,
    separator_bottom: usize,
    selection_frame_x: usize,
    selection_frame_y: usize,
    background: Pixel,
    background_565: Rgb565Pixel,
    alternate_background: Pixel,
    alternate_background_565: Rgb565Pixel,
    border: Pixel,
    border_565: Rgb565Pixel,
    text: Pixel,
    muted_text: Pixel,
    selection_fill_565: Rgb565Pixel,
    selection_text_565: Rgb565Pixel,
    selection_frame_565: Rgb565Pixel,
    badge_fill: Pixel,
    badge_fill_565: Rgb565Pixel,
    badge_text: Pixel,
    title_typeface: ConsoleTypeface,
    meta_typeface: ConsoleTypeface,
    crt_palette: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ArcadeListRasterMetrics {
    scroll_quantum_y: i32,
    separator_y: usize,
    selection_frame_x: usize,
    selection_frame_y: usize,
}

impl ArcadeListRasterMetrics {
    const fn native_crt() -> Self {
        Self {
            scroll_quantum_y: 1,
            separator_y: 1,
            selection_frame_x: 1,
            selection_frame_y: 1,
        }
    }

    fn for_display(display: &UiDisplay) -> Self {
        if display.output_route() == ResolvedOutputRoute::Crt240p60 {
            Self {
                scroll_quantum_y: 2,
                separator_y: 2,
                selection_frame_x: 1,
                selection_frame_y: 2,
            }
        } else {
            Self::native_crt()
        }
    }
}

impl ArcadeListStyle {
    const fn hdmi() -> Self {
        Self {
            row_height: ARCADE_ROW_HEIGHT,
            scroll_quantum_y: 1,
            separator_top: 1,
            separator_bottom: 1,
            selection_frame_x: ARCADE_HDMI_SELECTION_FRAME_THICKNESS,
            selection_frame_y: ARCADE_HDMI_SELECTION_FRAME_THICKNESS,
            background: ARCADE_LIST_BG_COLOR,
            background_565: ARCADE_LIST_BG_COLOR_565,
            alternate_background: Pixel(0x00150f20),
            alternate_background_565: ARCADE_LIST_ALT_BG_COLOR_565,
            border: Pixel(0x00251c34),
            border_565: ARCADE_LIST_ROW_BORDER_COLOR_565,
            text: Pixel(0x00fff6ff),
            muted_text: Pixel(0x00706080),
            selection_fill_565: ARCADE_SELECTION_FILL_COLOR_565,
            selection_text_565: Rgb565Pixel(0),
            selection_frame_565: ARCADE_SELECTION_FRAME_COLOR,
            badge_fill: ARCADE_NEW_BADGE_FILL,
            badge_fill_565: ARCADE_NEW_BADGE_FILL_565,
            badge_text: ARCADE_NEW_BADGE_TEXT,
            title_typeface: ConsoleTypeface::Nocive15,
            meta_typeface: ConsoleTypeface::PressStart2P,
            crt_palette: false,
        }
    }

    const fn crt(metrics: CrtUiMetrics) -> Self {
        Self::crt_with_raster(metrics, ArcadeListRasterMetrics::native_crt())
    }

    fn crt_for_display(metrics: CrtUiMetrics, display: &UiDisplay) -> Self {
        Self::crt_with_raster(metrics, ArcadeListRasterMetrics::for_display(display))
    }

    const fn crt_with_raster(metrics: CrtUiMetrics, raster: ArcadeListRasterMetrics) -> Self {
        Self {
            row_height: metrics.game_row_height,
            scroll_quantum_y: raster.scroll_quantum_y,
            separator_top: raster.separator_y,
            separator_bottom: 0,
            selection_frame_x: raster.selection_frame_x,
            selection_frame_y: raster.selection_frame_y,
            background: Pixel(0x00020817),
            background_565: rgb565_from_rgb888(0x02, 0x08, 0x17),
            alternate_background: Pixel(0x0006122b),
            alternate_background_565: rgb565_from_rgb888(0x06, 0x12, 0x2b),
            border: Pixel(0x005e59aa),
            border_565: rgb565_from_rgb888(0x5e, 0x59, 0xaa),
            text: Pixel(0x00aaa5ff),
            muted_text: Pixel(0x005e59aa),
            selection_fill_565: rgb565_from_rgb888(0x40, 0xe5, 0xe7),
            selection_text_565: rgb565_from_rgb888(0x03, 0x13, 0x2d),
            selection_frame_565: rgb565_from_rgb888(0x40, 0xe5, 0xe7),
            badge_fill: Pixel(0x0040e5e7),
            badge_fill_565: rgb565_from_rgb888(0x40, 0xe5, 0xe7),
            badge_text: Pixel(0x0003132d),
            title_typeface: ConsoleTypeface::Nocive15,
            meta_typeface: match metrics.font_family {
                CrtFontFamily::PressStart2P => ConsoleTypeface::PressStart2P,
            },
            crt_palette: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArcadeListGeometry {
    pub x: usize,
    pub y: usize,
    pub width: usize,
}

impl ArcadeListGeometry {
    pub const NORMAL: Self = Self {
        x: ARCADE_LIST_X,
        y: ARCADE_LIST_Y,
        width: ARCADE_LIST_W,
    };

    pub fn search_for_render_w(render_w: usize) -> Self {
        let x = if render_w <= 640 {
            render_w * 2 / 5 + ARCADE_LIST_X * 2
        } else {
            render_w / 2 + ARCADE_LIST_X
        }
        .min(render_w.saturating_sub(1));
        Self {
            x,
            y: ARCADE_SEARCH_LIST_Y,
            width: render_w.saturating_sub(x + ARCADE_LIST_X).max(1),
        }
    }

    pub fn normal_for_render_w(render_w: usize) -> Self {
        Self {
            x: ARCADE_LIST_X,
            y: ARCADE_LIST_Y,
            width: render_w.saturating_sub(ARCADE_LIST_X * 2).max(1),
        }
    }

    pub fn portrait(render_w: usize, render_h: usize, search: bool) -> Self {
        let margin = ARCADE_LIST_X * 2;
        let y = if search {
            56
        } else {
            64 + render_h * 38 / 100 + 12
        };
        Self {
            x: margin,
            y: y.min(render_h.saturating_sub(1)),
            width: render_w.saturating_sub(margin * 2).max(1),
        }
    }

    pub fn crt_for_content(content: CrtContentRect, metrics: CrtUiMetrics, search: bool) -> Self {
        let grid_x = metrics.grid_x.max(1) as usize;
        let grid_y = metrics.grid_y.max(1) as usize;
        let margin = grid_x * 2;
        let y = content.y + metrics.header_height.max(1) as usize + grid_y * 3;
        let x = if search {
            (content.x + content.width * 2 / 5 + margin * 2).min(content.right().saturating_sub(1))
        } else {
            content.x + margin
        };
        Self {
            x,
            y,
            width: content.right().saturating_sub(x + margin).max(1),
        }
    }

    pub fn dirty_rect(self) -> DirtyRect {
        DirtyRect {
            x0: self.x,
            y0: self.y,
            x1: self.x + self.width,
            y1: self.y + ARCADE_LIST_H,
        }
    }

    pub fn visible_height(self, render_h: usize) -> usize {
        self.visible_height_with_metrics(render_h, None)
    }

    pub fn visible_height_with_metrics(
        self,
        render_h: usize,
        metrics: Option<CrtUiMetrics>,
    ) -> usize {
        let bottom_inset = if let Some(metrics) = metrics {
            metrics.footer_height.max(1) as usize + metrics.grid_y.max(1) as usize * 3
        } else if self.y == ARCADE_LIST_Y {
            32
        } else {
            16
        };
        render_h
            .saturating_sub(self.y + bottom_inset)
            .min(ARCADE_LIST_H)
    }
}

pub struct ArcadeListRenderer {
    title_font: ConsoleFont,
    meta_font: ConsoleFont,
    row_cache: HashMap<usize, CachedArcadeRow>,
    favourite_launch_refs: HashSet<String>,
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
    filter_acknowledged_indices: Vec<usize>,
    geometry: ArcadeListGeometry,
    width: usize,
    visible_height: usize,
    style: ArcadeListStyle,
    crt_metrics: Option<CrtUiMetrics>,
}

pub struct CachedArcadeRow {
    pub title: Arc<str>,
    pub is_new: bool,
    pub is_favourite: bool,
    pub pixels: Vec<Rgb565Pixel>,
    pub last_used: u64,
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
pub struct ArcadeListItem {
    pub title: String,
    pub count: Option<usize>,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ArcadeFilterListDrawKey {
    len: usize,
    visual_px: i32,
    content_hash: u64,
    visible_hash: u64,
}

pub use mister_magik_mister_runtime::framebuffer::latch_state::DirectLayerUpdate as ArcadeListUpdate;

impl ArcadeListRenderer {
    pub fn new() -> Self {
        Self::new_with_style(ArcadeListStyle::hdmi(), None)
    }

    pub fn new_for_crt(row_height: i32) -> Self {
        let mut metrics = CrtUiMetrics::for_framebuffer(640, 480);
        metrics.game_row_height = row_height.max(1);
        Self::new_for_crt_metrics(metrics)
    }

    /// Uses the same route-owned metrics and font as the Slint CRT layer.
    pub fn new_for_crt_metrics(metrics: CrtUiMetrics) -> Self {
        Self::new_with_style(ArcadeListStyle::crt(metrics), Some(metrics))
    }

    /// Uses route-owned row metrics plus the physical scanline contract.
    pub fn new_for_crt_display(metrics: CrtUiMetrics, display: &UiDisplay) -> Self {
        Self::new_with_style(
            ArcadeListStyle::crt_for_display(metrics, display),
            Some(metrics),
        )
    }

    fn new_with_style(style: ArcadeListStyle, crt_metrics: Option<CrtUiMetrics>) -> Self {
        Self {
            title_font: ConsoleFont::new_with_typeface(ARCADE_LIST_FONT_PX, style.title_typeface),
            meta_font: ConsoleFont::new_with_typeface(
                ARCADE_LIST_META_FONT_PX,
                style.meta_typeface,
            ),
            row_cache: HashMap::new(),
            favourite_launch_refs: HashSet::new(),
            surface: vec![style.background_565; ARCADE_LIST_W * ARCADE_LIST_H],
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
            filter_acknowledged_indices: Vec::new(),
            geometry: ArcadeListGeometry::NORMAL,
            width: ARCADE_LIST_W,
            visible_height: ARCADE_LIST_H,
            style,
            crt_metrics,
        }
    }

    pub fn dirty_rect(&self) -> DirtyRect {
        let mut rect = self.geometry.dirty_rect();
        rect.y1 = rect.y0 + self.visible_height;
        rect
    }

    pub fn set_filter_acknowledged_indices(&mut self, indices: Vec<usize>) {
        self.filter_acknowledged_indices = indices;
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn present_pixels(&self, update: &ArcadeListUpdate, redraw_selection_frame: bool) -> usize {
        arcade_list_present_pixels_with_geometry(
            update,
            self.width,
            self.selection_y(),
            self.visible_height,
            self.style.row_height as usize,
            self.style.selection_frame_x,
            self.style.selection_frame_y,
            redraw_selection_frame,
        )
    }

    pub fn set_geometry(&mut self, geometry: ArcadeListGeometry) {
        self.set_geometry_for_render_h(geometry, ARCADE_LIST_Y + ARCADE_LIST_H);
    }

    pub fn set_geometry_for_render_h(&mut self, geometry: ArcadeListGeometry, render_h: usize) {
        self.visible_height = geometry.visible_height_with_metrics(render_h, self.crt_metrics);
        if self.geometry != geometry {
            if self.width != geometry.width {
                self.width = geometry.width;
                self.surface = vec![self.style.background_565; self.width * ARCADE_LIST_H];
                self.row_cache.clear();
                self.row_fingerprint_cache.clear();
            }
            self.geometry = geometry;
            self.last_draw = None;
            self.last_filter_draw = None;
            self.surface_y = 0;
        }
    }

    pub fn invalidate_presented_layer(&mut self) {
        self.last_draw = None;
        self.last_filter_draw = None;
        self.surface_y = 0;
    }

    pub fn set_favourite_launch_refs<'a>(&mut self, refs: impl IntoIterator<Item = &'a str>) {
        let refs = refs.into_iter().map(str::to_owned).collect();
        if self.favourite_launch_refs != refs {
            self.favourite_launch_refs = refs;
            self.row_cache.clear();
            self.row_fingerprint_cache.clear();
            self.invalidate_presented_layer();
        }
    }

    pub fn draw(
        &mut self,
        games: ArcadeGameView<'_>,
        _selected: usize,
        visual_index: f32,
        force: bool,
    ) -> Option<ArcadeListUpdate> {
        self.last_filter_draw = None;
        let visual_px = arcade_visual_px(
            visual_index,
            self.style.row_height,
            self.style.scroll_quantum_y,
        );
        let anchor = arcade_anchor_for_visual_px(games.len(), visual_px, self.style.row_height);
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
            self.draw_content_band(games, visual_px, 0, self.visible_height);
        } else if content_delta == 0 {
        } else if content_delta.unsigned_abs() as usize >= self.visible_height {
            self.surface_y = 0;
            self.draw_content_band(games, visual_px, 0, self.visible_height);
        } else if content_delta < 0 {
            let d = content_delta.unsigned_abs() as usize;
            self.surface_y = (self.surface_y + d) % self.visible_height;
            self.draw_content_band(games, visual_px, self.visible_height - d, d);
        } else {
            let d = content_delta as usize;
            self.surface_y = (self.surface_y + self.visible_height - d) % self.visible_height;
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
        if content_delta == 0 || content_delta.unsigned_abs() as usize >= self.visible_height {
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        Some(ArcadeListUpdate::Scroll {
            delta_y: content_delta as isize,
            rect: self.dirty_rect(),
        })
    }

    pub fn draw_filter_items(
        &mut self,
        items: &[ArcadeListItem],
        _selected: usize,
        visual_index: f32,
        force: bool,
    ) -> Option<ArcadeListUpdate> {
        self.last_draw = None;
        let visual_px = arcade_visual_px(
            visual_index,
            self.style.row_height,
            self.style.scroll_quantum_y,
        );
        let key = ArcadeFilterListDrawKey {
            len: items.len(),
            visual_px,
            content_hash: arcade_filter_content_hash(items, &self.filter_acknowledged_indices),
            visible_hash: arcade_filter_visible_window_hash(
                items,
                visual_px,
                self.style.row_height,
            ),
        };
        REQUESTED_FILTER_CONTENT_HASH.store(key.content_hash, Ordering::Relaxed);
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
        let can_reuse_scrolled_surface = previous.as_ref().is_some_and(|previous| {
            previous.len == key.len && previous.content_hash == key.content_hash
        });
        if previous.is_none() || !can_reuse_scrolled_surface || items.is_empty() {
            self.surface_y = 0;
            self.draw_filter_content_band(items, visual_px, 0, self.visible_height);
            RENDERED_FILTER_CONTENT_HASH.store(key.content_hash, Ordering::Relaxed);
        } else if content_delta == 0 {
        } else if content_delta.unsigned_abs() as usize >= self.visible_height {
            self.surface_y = 0;
            self.draw_filter_content_band(items, visual_px, 0, self.visible_height);
        } else if content_delta < 0 {
            let d = content_delta.unsigned_abs() as usize;
            self.surface_y = (self.surface_y + d) % self.visible_height;
            self.draw_filter_content_band(items, visual_px, self.visible_height - d, d);
        } else {
            let d = content_delta as usize;
            self.surface_y = (self.surface_y + self.visible_height - d) % self.visible_height;
            self.draw_filter_content_band(items, visual_px, 0, d);
        }
        if force || previous.is_none() || !can_reuse_scrolled_surface {
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        if content_delta == 0 || content_delta.unsigned_abs() as usize >= self.visible_height {
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        Some(ArcadeListUpdate::Scroll {
            delta_y: content_delta as isize,
            rect: self.dirty_rect(),
        })
    }

    pub fn selection_rect(&self) -> DirtyRect {
        let y = self.selection_y();
        DirtyRect {
            x0: self.geometry.x,
            y0: self.geometry.y + y,
            x1: self.geometry.x + self.width,
            y1: self.geometry.y + y + self.style.row_height as usize,
        }
    }

    fn default_selection_y() -> usize {
        Self::selection_y_for_height(ARCADE_LIST_H, ARCADE_ROW_HEIGHT)
    }

    fn selection_y(&self) -> usize {
        Self::selection_y_for_height(self.visible_height, self.style.row_height)
    }

    fn selection_y_for_height(height: usize, row_height: i32) -> usize {
        let row_h = row_height.max(1) as usize;
        let visible_rows = (height / row_h).max(1);
        // Keep the selection one row above the geometric midpoint so the
        // viewport favors upcoming entries without pinning to an edge.
        (visible_rows / 2).saturating_sub(1) * row_h
    }

    fn draw_content_band(
        &mut self,
        games: ArcadeGameView<'_>,
        visual_px: i32,
        band_y: usize,
        band_h: usize,
    ) {
        if band_h == 0 || band_y >= self.visible_height {
            return;
        }
        let band_h = band_h.min(self.visible_height - band_y);
        if games.is_empty() {
            let mut band = std::mem::take(&mut self.band_scratch);
            band.resize(self.width * band_h, self.style.background);
            band.fill(self.style.background);
            self.meta_font.draw_text_clipped(
                &mut band,
                self.width,
                self.width,
                0,
                band_h,
                96,
                (self.visible_height / 2).saturating_sub(band_y) as isize,
                "NO GAMES",
                self.style.muted_text,
            );
            self.copy_band_to_surface(&band, band_y, band_h);
            self.band_scratch = band;
            return;
        }
        self.fill_surface_band(band_y, band_h, self.style.background_565);
        let row_h = self.style.row_height as isize;
        let Some((first, end)) = arcade_visible_window_range_px(
            games.len(),
            visual_px,
            self.style.row_height,
            self.selection_y(),
            self.visible_height,
        ) else {
            return;
        };
        for idx in first..=end {
            let y = arcade_row_y(idx, visual_px, self.selection_y(), self.style.row_height);
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
        if band_h == 0 || band_y >= self.visible_height {
            return;
        }
        let band_h = band_h.min(self.visible_height - band_y);
        self.fill_surface_band(band_y, band_h, self.style.background_565);
        if items.is_empty() {
            let mut band = std::mem::take(&mut self.band_scratch);
            band.resize(self.width * band_h, self.style.background);
            band.fill(self.style.background);
            self.meta_font.draw_text_clipped(
                &mut band,
                self.width,
                self.width,
                0,
                band_h,
                96,
                (self.visible_height / 2).saturating_sub(band_y) as isize,
                "NO FILTERS",
                self.style.muted_text,
            );
            self.copy_band_to_surface(&band, band_y, band_h);
            self.band_scratch = band;
            return;
        }
        let row_h = self.style.row_height as isize;
        let Some((first, end)) = arcade_visible_window_range_px(
            items.len(),
            visual_px,
            self.style.row_height,
            self.selection_y(),
            self.visible_height,
        ) else {
            return;
        };
        for (idx, item) in items.iter().enumerate().take(end + 1).skip(first) {
            let y = arcade_row_y(idx, visual_px, self.selection_y(), self.style.row_height);
            let clip_y0 = y.max(band_y as isize);
            let clip_y1 = (y + row_h).min((band_y + band_h) as isize);
            if clip_y1 <= clip_y0 {
                continue;
            }
            let acknowledged = self.filter_acknowledged_indices.binary_search(&idx).is_ok();
            let row = self.render_filter_row(item, idx, acknowledged);
            let copy_h = (clip_y1 - clip_y0) as usize;
            let src_y = (clip_y0 - y) as usize;
            for row_y in 0..copy_h {
                let src = (src_y + row_y) * self.width;
                let viewport_y = clip_y0 as usize + row_y;
                let dst_y = (self.surface_y + viewport_y) % self.visible_height;
                let dst = dst_y * self.width;
                self.surface[dst..dst + self.width].copy_from_slice(&row[src..src + self.width]);
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
            !arc_str_eq(&cached.title, &game.title)
                || cached.is_new != game.is_new
                || cached.is_favourite
                    != self.favourite_launch_refs.contains(game.mra_path.as_ref())
        });
        if needs_render {
            if self.row_cache.len() >= ARCADE_ROW_CACHE_MAX {
                prune_arcade_row_cache(&mut self.row_cache);
            }
            let is_favourite = self.favourite_launch_refs.contains(game.mra_path.as_ref());
            let row = self.render_row(game.title.as_ref(), game.is_new, is_favourite, idx);
            let last_used = self.next_row_cache_epoch();
            self.row_cache.insert(
                idx,
                CachedArcadeRow {
                    title: Arc::clone(&game.title),
                    is_new: game.is_new,
                    is_favourite,
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
        let row_h = self.style.row_height as isize;
        let clip_y0 = y.max(band_y as isize);
        let clip_y1 = (y + row_h).min((band_y + band_h) as isize);
        if clip_y1 <= clip_y0 {
            return;
        }
        let copy_h = (clip_y1 - clip_y0) as usize;
        let src_y = (clip_y0 - y) as usize;
        let dst_y = (clip_y0 as usize).saturating_sub(band_y);
        for row_y in 0..copy_h {
            let src = (src_y + row_y) * self.width;
            let viewport_y = band_y + dst_y + row_y;
            let dst_y = (self.surface_y + viewport_y) % self.visible_height;
            let dst = dst_y * self.width;
            self.surface[dst..dst + self.width].copy_from_slice(&row[src..src + self.width]);
        }
    }

    fn fill_surface_band(&mut self, band_y: usize, band_h: usize, color: Rgb565Pixel) {
        for row in 0..band_h {
            let dst_y = (self.surface_y + band_y + row) % self.visible_height;
            let dst = dst_y * self.width;
            self.surface[dst..dst + self.width].fill(color);
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
        let Some((first, end)) = arcade_visible_window_range_px(
            games.len(),
            visual_px,
            self.style.row_height,
            self.selection_y(),
            self.visible_height,
        ) else {
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
            let src = row * self.width;
            let dst_y = (self.surface_y + band_y + row) % self.visible_height;
            let dst = dst_y * self.width;
            copy_pixel_to_rgb565_row(
                &band[src..src + self.width],
                &mut self.surface[dst..dst + self.width],
            );
        }
    }

    pub fn copy_layer_to_fb0(
        &mut self,
        disp: &mut MappedRgb565Framebuffer,
        redraw_selection_frame: bool,
    ) {
        self.copy_viewport_band_to_fb0(disp, 0, self.visible_height);
        if redraw_selection_frame {
            self.copy_selection_frame_to_fb0(disp);
        }
    }

    pub fn compose_layer_to_cached(
        &mut self,
        target: &mut UiFrameTarget,
        redraw_selection_frame: bool,
    ) {
        self.compose_viewport_band_to_cached(target, 0, self.visible_height);
        if redraw_selection_frame {
            self.compose_selection_frame_to_cached(target);
        }
    }

    pub fn compose_layer_to_oriented_cached(
        &mut self,
        target: &mut UiFrameTarget,
        output_layout: Rgb565OutputLayout,
        redraw_selection_frame: bool,
    ) {
        self.compose_viewport_band_to_oriented_cached(
            target.cached_565_mut(),
            output_layout,
            0,
            self.visible_height,
        );
        if redraw_selection_frame {
            self.compose_selection_frame_to_oriented_cached(target.cached_565_mut(), output_layout);
        }
    }

    /// Restores the complete viewport from a stationary physical backdrop,
    /// then draws the CRT list's non-fill pixels over it. This intentionally
    /// rewrites every viewport pixel so glyphs from a preceding scroll
    /// position cannot survive in newly exposed row fill.
    pub fn compose_layer_over_backdrop_to_oriented_cached(
        &mut self,
        target: &mut UiFrameTarget,
        backdrop: &[Rgb565Pixel],
        output_layout: Rgb565OutputLayout,
        redraw_selection_frame: bool,
    ) -> bool {
        if backdrop.len() < output_layout.len()
            || target.cached_565().len() < output_layout.len()
            || self.geometry.x.saturating_add(self.width) > output_layout.logical_width()
            || self.geometry.y.saturating_add(self.visible_height) > output_layout.logical_height()
        {
            return false;
        }

        let selection_y = self.selection_y();
        let selection_bottom = selection_y + self.style.row_height.max(1) as usize;
        let cached = target.cached_565_mut();
        let identity_layout = matches!(output_layout.rotation(), OutputRotation::None)
            && output_layout.physical_stride() == output_layout.logical_width();
        if identity_layout {
            // CRT240p is an upright contiguous surface. Restore completely
            // blank rows with one slice copy and reserve per-pixel work for
            // glyph and selection rows.
            for viewport_y in 0..self.visible_height {
                let source_y = (self.surface_y + viewport_y) % self.visible_height;
                let source_start = source_y * self.width;
                let destination_start = (self.geometry.y + viewport_y)
                    * output_layout.physical_stride()
                    + self.geometry.x;
                let destination = &mut cached[destination_start..destination_start + self.width];
                let selected = viewport_y >= selection_y && viewport_y < selection_bottom;
                let surface_row = &self.surface[source_start..source_start + self.width];
                let backdrop_row = &backdrop[destination_start..destination_start + self.width];
                if selected {
                    for x in 0..self.width {
                        destination[x] =
                            selected_aperture_pixel_with_style(surface_row[x], self.style);
                    }
                    continue;
                }

                // Restore the complete backdrop row with one bulk copy, then
                // overwrite only contiguous non-fill runs from the list
                // surface. This keeps the backdrop stationary while reducing
                // per-pixel stores for sparse glyphs and badges.
                destination.copy_from_slice(backdrop_row);
                let mut x = 0;
                while x < self.width {
                    while x < self.width
                        && is_arcade_unselected_fill_pixel_with_style(surface_row[x], self.style)
                    {
                        x += 1;
                    }
                    let run_start = x;
                    while x < self.width
                        && !is_arcade_unselected_fill_pixel_with_style(surface_row[x], self.style)
                    {
                        x += 1;
                    }
                    if run_start < x {
                        destination[run_start..x].copy_from_slice(&surface_row[run_start..x]);
                    }
                }
            }
            if redraw_selection_frame {
                self.compose_selection_frame_to_oriented_cached(cached, output_layout);
            }
            return true;
        }
        for viewport_y in 0..self.visible_height {
            let source_y = (self.surface_y + viewport_y) % self.visible_height;
            let source_row = source_y * self.width;
            let selected = viewport_y >= selection_y && viewport_y < selection_bottom;
            for x in 0..self.width {
                let logical_x = self.geometry.x + x;
                let logical_y = self.geometry.y + viewport_y;
                let offset = output_layout.physical_offset(logical_x, logical_y);
                let pixel = self.surface[source_row + x];
                cached[offset] = if selected {
                    selected_aperture_pixel_with_style(pixel, self.style)
                } else if is_arcade_unselected_fill_pixel_with_style(pixel, self.style) {
                    backdrop[offset]
                } else {
                    pixel
                };
            }
        }
        if redraw_selection_frame {
            self.compose_selection_frame_to_oriented_cached(cached, output_layout);
        }
        true
    }

    pub fn copy_layer_to_hidden(
        &mut self,
        hidden: &mut ScanoutSlotsRgb565Framebuffer,
        redraw_selection_frame: bool,
    ) {
        self.copy_viewport_band_to_hidden(hidden, 0, self.visible_height);
        if redraw_selection_frame {
            self.copy_selection_frame_to_hidden(hidden);
        }
    }

    fn copy_viewport_band_to_fb0(
        &mut self,
        disp: &mut MappedRgb565Framebuffer,
        viewport_y: usize,
        h: usize,
    ) {
        if h == 0 || viewport_y >= self.visible_height {
            return;
        }
        let h = h.min(self.visible_height - viewport_y);
        for_each_arcade_list_present_segment_with_geometry(
            self.width,
            viewport_y,
            h,
            self.selection_y(),
            self.visible_height,
            self.style.row_height as usize,
            self.style.selection_frame_x,
            self.style.selection_frame_y,
            |kind, x, y, w, h| match kind {
                ArcadeListPresentKind::Normal => {
                    self.copy_surface_rect_to_fb0(disp, x, y, w, h);
                }
                ArcadeListPresentKind::Inverted => {
                    if self.style.crt_palette || arcade_selection_inversion_enabled() {
                        self.copy_inverted_surface_rect_to_fb0(disp, x, y, w, h);
                    } else {
                        self.copy_surface_rect_to_fb0(disp, x, y, w, h);
                    }
                }
            },
        );
    }

    fn copy_surface_rect_to_fb0(
        &mut self,
        disp: &mut MappedRgb565Framebuffer,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % self.visible_height;
            let copy_h = (h - copied).min(self.visible_height - src_y);
            self.copy_surface_chunk_to_fb0(disp, x, viewport_y + copied, w, copy_h);
            copied += copy_h;
        }
    }

    fn copy_surface_chunk_to_fb0(
        &mut self,
        disp: &mut MappedRgb565Framebuffer,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        if w == 0 || h == 0 {
            return;
        }
        let src_y = (self.surface_y + viewport_y) % self.visible_height;
        if x == 0 && w == self.width {
            let src = src_y * self.width;
            copy_dense_rect_565(
                disp,
                self.geometry.x,
                self.geometry.y + viewport_y,
                self.width,
                h,
                &self.surface[src..src + h * self.width],
            );
            return;
        }
        copy_strided_rect_565(
            disp,
            self.geometry.x + x,
            self.geometry.y + viewport_y,
            w,
            h,
            &self.surface,
            self.width,
            x,
            src_y,
        );
    }

    fn copy_inverted_surface_rect_to_fb0(
        &mut self,
        disp: &mut MappedRgb565Framebuffer,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % self.visible_height;
            let copy_h = (h - copied).min(self.visible_height - src_y);
            let target_x = self.geometry.x + x;
            let target_y = self.geometry.y + viewport_y + copied;
            let inverted = self.prepare_inverted_surface_chunk(x, viewport_y + copied, w, copy_h);
            copy_dense_rect_565(disp, target_x, target_y, w, copy_h, inverted);
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
        let src_y = (self.surface_y + viewport_y) % self.visible_height;
        for row in 0..h {
            let src = (src_y + row) * self.width + x;
            let dst = row * w;
            for col in 0..w {
                self.selection_invert_scratch[dst + col] =
                    selected_aperture_pixel_with_style(self.surface[src + col], self.style);
            }
        }
        &self.selection_invert_scratch
    }

    fn copy_selection_frame_to_fb0(&mut self, disp: &mut MappedRgb565Framebuffer) {
        let rect = self.selection_rect();
        let color = self.style.selection_frame_565;
        let thickness_x = self.style.selection_frame_x;
        let thickness_y = self.style.selection_frame_y;
        let h = rect.y1.saturating_sub(rect.y0).min(ARCADE_LIST_H);
        self.selection_horizontal
            .resize(self.width * thickness_y, color);
        self.selection_horizontal.fill(color);
        copy_dense_rect_565(
            disp,
            rect.x0,
            rect.y0,
            self.width,
            thickness_y,
            &self.selection_horizontal,
        );
        copy_dense_rect_565(
            disp,
            rect.x0,
            rect.y1.saturating_sub(thickness_y),
            self.width,
            thickness_y,
            &self.selection_horizontal,
        );
        self.selection_vertical.resize(thickness_x * h, color);
        self.selection_vertical.fill(color);
        copy_dense_rect_565(
            disp,
            rect.x0,
            rect.y0,
            thickness_x,
            h,
            &self.selection_vertical,
        );
        copy_dense_rect_565(
            disp,
            rect.x1.saturating_sub(thickness_x),
            rect.y0,
            thickness_x,
            h,
            &self.selection_vertical,
        );
    }

    fn compose_viewport_band_to_cached(
        &mut self,
        target: &mut UiFrameTarget,
        viewport_y: usize,
        h: usize,
    ) {
        if h == 0 || viewport_y >= self.visible_height {
            return;
        }
        let h = h.min(self.visible_height - viewport_y);
        for_each_arcade_list_present_segment_with_geometry(
            self.width,
            viewport_y,
            h,
            self.selection_y(),
            self.visible_height,
            self.style.row_height as usize,
            self.style.selection_frame_x,
            self.style.selection_frame_y,
            |kind, x, y, w, h| match kind {
                ArcadeListPresentKind::Normal => {
                    self.compose_surface_rect_to_cached(target, x, y, w, h);
                }
                ArcadeListPresentKind::Inverted => {
                    if self.style.crt_palette || arcade_selection_inversion_enabled() {
                        self.compose_inverted_surface_rect_to_cached(target, x, y, w, h);
                    } else {
                        self.compose_surface_rect_to_cached(target, x, y, w, h);
                    }
                }
            },
        );
    }

    fn compose_viewport_band_to_oriented_cached(
        &mut self,
        target: &mut [Rgb565Pixel],
        output_layout: Rgb565OutputLayout,
        viewport_y: usize,
        h: usize,
    ) {
        if h == 0 || viewport_y >= self.visible_height {
            return;
        }
        let h = h.min(self.visible_height - viewport_y);
        for_each_arcade_list_present_segment_with_geometry(
            self.width,
            viewport_y,
            h,
            self.selection_y(),
            self.visible_height,
            self.style.row_height as usize,
            self.style.selection_frame_x,
            self.style.selection_frame_y,
            |kind, x, y, w, h| match kind {
                ArcadeListPresentKind::Normal => {
                    self.compose_surface_rect_to_oriented_cached(target, output_layout, x, y, w, h)
                }
                ArcadeListPresentKind::Inverted => {
                    if self.style.crt_palette || arcade_selection_inversion_enabled() {
                        self.compose_inverted_surface_rect_to_oriented_cached(
                            target,
                            output_layout,
                            x,
                            y,
                            w,
                            h,
                        );
                    } else {
                        self.compose_surface_rect_to_oriented_cached(
                            target,
                            output_layout,
                            x,
                            y,
                            w,
                            h,
                        );
                    }
                }
            },
        );
    }

    fn copy_viewport_band_to_hidden(
        &mut self,
        hidden: &mut ScanoutSlotsRgb565Framebuffer,
        viewport_y: usize,
        h: usize,
    ) {
        if h == 0 || viewport_y >= self.visible_height {
            return;
        }
        let h = h.min(self.visible_height - viewport_y);
        for_each_arcade_list_present_segment_with_geometry(
            self.width,
            viewport_y,
            h,
            self.selection_y(),
            self.visible_height,
            self.style.row_height as usize,
            self.style.selection_frame_x,
            self.style.selection_frame_y,
            |kind, x, y, w, h| match kind {
                ArcadeListPresentKind::Normal => {
                    self.copy_surface_rect_to_hidden(hidden, x, y, w, h);
                }
                ArcadeListPresentKind::Inverted => {
                    if self.style.crt_palette || arcade_selection_inversion_enabled() {
                        self.copy_inverted_surface_rect_to_hidden(hidden, x, y, w, h);
                    } else {
                        self.copy_surface_rect_to_hidden(hidden, x, y, w, h);
                    }
                }
            },
        );
    }

    fn compose_surface_rect_to_cached(
        &mut self,
        target: &mut UiFrameTarget,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % self.visible_height;
            let copy_h = (h - copied).min(self.visible_height - src_y);
            target.compose_rect_565_strided(
                self.geometry.x + x,
                self.geometry.y + viewport_y + copied,
                w,
                copy_h,
                &self.surface,
                self.width,
                x,
                src_y,
            );
            copied += copy_h;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn compose_surface_rect_to_oriented_cached(
        &mut self,
        target: &mut [Rgb565Pixel],
        output_layout: Rgb565OutputLayout,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % self.visible_height;
            let copy_h = (h - copied).min(self.visible_height - src_y);
            let mut surface = Rgb565SurfaceMut::new(target, output_layout)
                .expect("launcher output layout matches its cached target");
            let copied_rect = surface.copy_rect_strided(
                self.geometry.x + x,
                self.geometry.y + viewport_y + copied,
                w,
                copy_h,
                &self.surface,
                self.width,
                x,
                src_y,
            );
            debug_assert!(copied_rect);
            copied += copy_h;
        }
    }

    fn copy_surface_rect_to_hidden(
        &mut self,
        hidden: &mut ScanoutSlotsRgb565Framebuffer,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % self.visible_height;
            let copy_h = (h - copied).min(self.visible_height - src_y);
            if let Err(e) = hidden.copy_rect_565_strided(
                self.geometry.x + x,
                self.geometry.y + viewport_y + copied,
                w,
                copy_h,
                &self.surface,
                self.width,
                x,
                src_y,
            ) {
                crate::ui_errln!("arcade_list_hidden_copy_failed: {e}");
            }
            copied += copy_h;
        }
    }

    fn compose_inverted_surface_rect_to_cached(
        &mut self,
        target: &mut UiFrameTarget,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % self.visible_height;
            let copy_h = (h - copied).min(self.visible_height - src_y);
            let target_x = self.geometry.x + x;
            let target_y = self.geometry.y + viewport_y + copied;
            let inverted = self.prepare_inverted_surface_chunk(x, viewport_y + copied, w, copy_h);
            target.compose_rect_565(target_x, target_y, w, copy_h, inverted);
            copied += copy_h;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn compose_inverted_surface_rect_to_oriented_cached(
        &mut self,
        target: &mut [Rgb565Pixel],
        output_layout: Rgb565OutputLayout,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % self.visible_height;
            let copy_h = (h - copied).min(self.visible_height - src_y);
            let target_x = self.geometry.x + x;
            let target_y = self.geometry.y + viewport_y + copied;
            let inverted = self.prepare_inverted_surface_chunk(x, viewport_y + copied, w, copy_h);
            let mut surface = Rgb565SurfaceMut::new(target, output_layout)
                .expect("launcher output layout matches its cached target");
            let copied_rect =
                surface.copy_rect_strided(target_x, target_y, w, copy_h, inverted, w, 0, 0);
            debug_assert!(copied_rect);
            copied += copy_h;
        }
    }

    fn copy_inverted_surface_rect_to_hidden(
        &mut self,
        hidden: &mut ScanoutSlotsRgb565Framebuffer,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % self.visible_height;
            let copy_h = (h - copied).min(self.visible_height - src_y);
            let target_x = self.geometry.x + x;
            let target_y = self.geometry.y + viewport_y + copied;
            let inverted = self.prepare_inverted_surface_chunk(x, viewport_y + copied, w, copy_h);
            if let Err(e) =
                hidden.copy_rect_565_strided(target_x, target_y, w, copy_h, inverted, w, 0, 0)
            {
                crate::ui_errln!("arcade_list_hidden_inverted_copy_failed: {e}");
            }
            copied += copy_h;
        }
    }

    fn compose_selection_frame_to_cached(&mut self, target: &mut UiFrameTarget) {
        let rect = self.selection_rect();
        let color = self.style.selection_frame_565;
        let thickness_x = self.style.selection_frame_x;
        let thickness_y = self.style.selection_frame_y;
        let h = rect.y1.saturating_sub(rect.y0).min(ARCADE_LIST_H);
        self.selection_horizontal
            .resize(self.width * thickness_y, color);
        self.selection_horizontal.fill(color);
        target.compose_rect_565(
            rect.x0,
            rect.y0,
            self.width,
            thickness_y,
            &self.selection_horizontal,
        );
        target.compose_rect_565(
            rect.x0,
            rect.y1.saturating_sub(thickness_y),
            self.width,
            thickness_y,
            &self.selection_horizontal,
        );
        self.selection_vertical.resize(thickness_x * h, color);
        self.selection_vertical.fill(color);
        target.compose_rect_565(rect.x0, rect.y0, thickness_x, h, &self.selection_vertical);
        target.compose_rect_565(
            rect.x1.saturating_sub(thickness_x),
            rect.y0,
            thickness_x,
            h,
            &self.selection_vertical,
        );
    }

    fn compose_selection_frame_to_oriented_cached(
        &mut self,
        target: &mut [Rgb565Pixel],
        output_layout: Rgb565OutputLayout,
    ) {
        let rect = self.selection_rect();
        let color = self.style.selection_frame_565;
        let thickness_x = self.style.selection_frame_x;
        let thickness_y = self.style.selection_frame_y;
        let h = rect.y1.saturating_sub(rect.y0).min(ARCADE_LIST_H);
        self.selection_horizontal
            .resize(self.width * thickness_y, color);
        self.selection_horizontal.fill(color);
        let mut surface = Rgb565SurfaceMut::new(target, output_layout)
            .expect("launcher output layout matches its cached target");
        let _ = surface.copy_rect_strided(
            rect.x0,
            rect.y0,
            self.width,
            thickness_y,
            &self.selection_horizontal,
            self.width,
            0,
            0,
        );
        let _ = surface.copy_rect_strided(
            rect.x0,
            rect.y1.saturating_sub(thickness_y),
            self.width,
            thickness_y,
            &self.selection_horizontal,
            self.width,
            0,
            0,
        );
        self.selection_vertical.resize(thickness_x * h, color);
        self.selection_vertical.fill(color);
        let _ = surface.copy_rect_strided(
            rect.x0,
            rect.y0,
            thickness_x,
            h,
            &self.selection_vertical,
            thickness_x,
            0,
            0,
        );
        let _ = surface.copy_rect_strided(
            rect.x1.saturating_sub(thickness_x),
            rect.y0,
            thickness_x,
            h,
            &self.selection_vertical,
            thickness_x,
            0,
            0,
        );
    }

    fn copy_selection_frame_to_hidden(&mut self, hidden: &mut ScanoutSlotsRgb565Framebuffer) {
        let rect = self.selection_rect();
        let color = self.style.selection_frame_565;
        let thickness_x = self.style.selection_frame_x;
        let thickness_y = self.style.selection_frame_y;
        let h = rect.y1.saturating_sub(rect.y0).min(ARCADE_LIST_H);
        self.selection_horizontal
            .resize(self.width * thickness_y, color);
        self.selection_horizontal.fill(color);
        if let Err(e) = hidden.copy_rect_565_strided(
            rect.x0,
            rect.y0,
            self.width,
            thickness_y,
            &self.selection_horizontal,
            self.width,
            0,
            0,
        ) {
            crate::ui_errln!("arcade_list_hidden_selection_copy_failed: {e}");
        }
        if let Err(e) = hidden.copy_rect_565_strided(
            rect.x0,
            rect.y1.saturating_sub(thickness_y),
            self.width,
            thickness_y,
            &self.selection_horizontal,
            self.width,
            0,
            0,
        ) {
            crate::ui_errln!("arcade_list_hidden_selection_copy_failed: {e}");
        }
        self.selection_vertical.resize(thickness_x * h, color);
        self.selection_vertical.fill(color);
        if let Err(e) = hidden.copy_rect_565_strided(
            rect.x0,
            rect.y0,
            thickness_x,
            h,
            &self.selection_vertical,
            thickness_x,
            0,
            0,
        ) {
            crate::ui_errln!("arcade_list_hidden_selection_copy_failed: {e}");
        }
        if let Err(e) = hidden.copy_rect_565_strided(
            rect.x1.saturating_sub(thickness_x),
            rect.y0,
            thickness_x,
            h,
            &self.selection_vertical,
            thickness_x,
            0,
            0,
        ) {
            crate::ui_errln!("arcade_list_hidden_selection_copy_failed: {e}");
        }
    }

    fn render_row(
        &mut self,
        title: &str,
        is_new: bool,
        is_favourite: bool,
        idx: usize,
    ) -> Vec<Rgb565Pixel> {
        let row_height = self.style.row_height as usize;
        let mut row = vec![Pixel(0); self.width * row_height];
        draw_arcade_row_background_with_style(&mut row, self.width, idx, self.style);
        let reserved = match (is_new, is_favourite) {
            (true, true) => 96,
            (true, false) => 76,
            (false, true) => 44,
            (false, false) => 24,
        };
        let title = self
            .title_font
            .clipped_text(title, self.width.saturating_sub(reserved));
        let gradient = if self.style.crt_palette {
            TextGradient::new(self.style.text, self.style.text, self.style.text)
        } else {
            ARCADE_TITLE_GRADIENT
        };
        let title_baseline = if self.style.crt_palette {
            self.title_font
                .centered_text_baseline(&title, 0, row_height)
        } else {
            (row_height / 2 + 6) as isize
        };
        self.title_font.draw_text_clipped_gradient(
            &mut row,
            self.width,
            self.width,
            0,
            row_height,
            12,
            title_baseline,
            &title,
            gradient,
        );
        if is_new {
            draw_new_badge(
                &mut row,
                self.width,
                row_height,
                self.style.badge_fill,
                self.style.badge_text,
                self.style,
                &mut self.meta_font,
            );
        }
        if is_favourite {
            let baseline = self.meta_font.centered_text_baseline("*", 0, row_height);
            self.meta_font.draw_text_clipped_gradient(
                &mut row,
                self.width,
                self.width,
                0,
                row_height,
                self.width.saturating_sub(22) as isize,
                baseline,
                "*",
                TextGradient::new(Pixel(0x00ffd166), Pixel(0x00ffd166), Pixel(0x00ffd166)),
            );
        }
        row.into_iter().map(pixel_to_rgb565).collect()
    }

    fn render_filter_row(
        &mut self,
        item: &ArcadeListItem,
        idx: usize,
        acknowledged: bool,
    ) -> Vec<Rgb565Pixel> {
        let row_height = self.style.row_height as usize;
        let mut row = vec![Pixel(0); self.width * row_height];
        draw_arcade_row_background_with_style(&mut row, self.width, idx, self.style);
        if acknowledged {
            row.fill(Pixel(0x00203a36));
        }
        let reserved = if item.count.is_some() { 68 } else { 24 };
        let title = self
            .title_font
            .clipped_text(&item.title, self.width.saturating_sub(reserved));
        let gradient = arcade_filter_gradient(self.style, item.active);
        let title_baseline = if self.style.crt_palette {
            self.title_font
                .centered_text_baseline(&title, 0, row_height)
        } else {
            (row_height / 2 + 6) as isize
        };
        self.title_font.draw_text_clipped_gradient(
            &mut row,
            self.width,
            self.width,
            0,
            row_height,
            12,
            title_baseline,
            &title,
            gradient,
        );
        if let Some(count) = item.count {
            let count = count.to_string();
            let count_baseline = if self.style.crt_palette {
                self.meta_font.centered_text_baseline(&count, 0, row_height)
            } else {
                (row_height / 2 + 5) as isize
            };
            self.meta_font.draw_text_clipped(
                &mut row,
                self.width,
                self.width,
                0,
                row_height,
                self.width.saturating_sub(60) as isize,
                count_baseline,
                &count,
                self.style.muted_text,
            );
        }
        row.into_iter().map(pixel_to_rgb565).collect()
    }
}

pub fn rendered_filter_content_hash() -> u64 {
    RENDERED_FILTER_CONTENT_HASH.load(Ordering::Relaxed)
}

pub fn requested_filter_content_hash() -> u64 {
    REQUESTED_FILTER_CONTENT_HASH.load(Ordering::Relaxed)
}

fn draw_new_badge(
    row: &mut [Pixel],
    width: usize,
    row_height: usize,
    fill: Pixel,
    text: Pixel,
    style: ArcadeListStyle,
    font: &mut ConsoleFont,
) {
    let x = width.saturating_sub(58);
    let w = 42usize;
    let (y, h, baseline_y) = if style.crt_palette {
        let content_top = style.separator_top.min(row_height);
        let content_bottom = row_height.saturating_sub(style.separator_bottom);
        let content_height = content_bottom.saturating_sub(content_top);
        let h = 18usize.min(content_height);
        let y = content_top + content_height.saturating_sub(h) / 2;
        (y, h, font.centered_text_baseline("NEW", y, h))
    } else {
        let y = if row_height <= 32 { 4 } else { 14 };
        (y, 18, y as isize + 12)
    };
    for dy in 0..h {
        let row_y = y + dy;
        if row_y >= row_height {
            break;
        }
        let start = row_y * width + x;
        let end = (start + w).min((row_y + 1) * width);
        row[start..end].fill(fill);
    }
    font.draw_text_clipped(
        row,
        width,
        width,
        0,
        row_height,
        x as isize + 9,
        baseline_y,
        "NEW",
        text,
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArcadeListPresentKind {
    Normal,
    Inverted,
}

#[cfg(test)]
pub fn for_each_arcade_list_present_segment(
    width: usize,
    viewport_y: usize,
    h: usize,
    emit: impl FnMut(ArcadeListPresentKind, usize, usize, usize, usize),
) {
    for_each_arcade_list_present_segment_with_geometry(
        width,
        viewport_y,
        h,
        ArcadeListRenderer::default_selection_y(),
        ARCADE_LIST_H,
        ARCADE_ROW_HEIGHT as usize,
        ARCADE_HDMI_SELECTION_FRAME_THICKNESS,
        ARCADE_HDMI_SELECTION_FRAME_THICKNESS,
        emit,
    );
}

fn for_each_arcade_list_present_segment_with_geometry(
    width: usize,
    viewport_y: usize,
    h: usize,
    selection_y: usize,
    visible_height: usize,
    row_height: usize,
    selection_frame_x: usize,
    selection_frame_y: usize,
    emit: impl FnMut(ArcadeListPresentKind, usize, usize, usize, usize),
) {
    if h == 0 || viewport_y >= visible_height {
        return;
    }
    let y0 = viewport_y;
    let y1 = (viewport_y + h).min(visible_height);

    let selection_bottom = selection_y + row_height;
    let inner_top = selection_y + selection_frame_y;
    let inner_bottom = selection_bottom.saturating_sub(selection_frame_y);
    let mut emit = emit;

    emit_row_overlap(
        y0..y1,
        0..selection_y,
        0,
        width,
        ArcadeListPresentKind::Normal,
        &mut emit,
    );
    emit_row_overlap(
        y0..y1,
        inner_top..inner_bottom,
        selection_frame_x,
        width.saturating_sub(selection_frame_x * 2),
        ArcadeListPresentKind::Inverted,
        &mut emit,
    );
    emit_row_overlap(
        y0..y1,
        selection_bottom..visible_height,
        0,
        width,
        ArcadeListPresentKind::Normal,
        &mut emit,
    );
}

#[cfg(test)]
pub fn arcade_list_present_pixels(
    update: &ArcadeListUpdate,
    width: usize,
    redraw_selection_frame: bool,
) -> usize {
    arcade_list_present_pixels_with_geometry(
        update,
        width,
        ArcadeListRenderer::default_selection_y(),
        ARCADE_LIST_H,
        ARCADE_ROW_HEIGHT as usize,
        ARCADE_HDMI_SELECTION_FRAME_THICKNESS,
        ARCADE_HDMI_SELECTION_FRAME_THICKNESS,
        redraw_selection_frame,
    )
}

fn arcade_list_present_pixels_with_geometry(
    update: &ArcadeListUpdate,
    width: usize,
    selection_y: usize,
    visible_height: usize,
    row_height: usize,
    selection_frame_x: usize,
    selection_frame_y: usize,
    redraw_selection_frame: bool,
) -> usize {
    let rect = match update {
        ArcadeListUpdate::Full(rect) => *rect,
        ArcadeListUpdate::Scroll { rect, .. } => *rect,
    };
    let mut pixels = 0usize;
    for_each_arcade_list_present_segment_with_geometry(
        width,
        0,
        rect.rows() as usize,
        selection_y,
        visible_height,
        row_height,
        selection_frame_x,
        selection_frame_y,
        |_, _, _, w, h| {
            pixels += w * h;
        },
    );
    if redraw_selection_frame {
        let horizontal = width * selection_frame_y * 2;
        let vertical = selection_frame_x * row_height * 2;
        pixels += horizontal + vertical;
    }
    pixels
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

pub fn prune_arcade_row_cache(row_cache: &mut HashMap<usize, CachedArcadeRow>) {
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
    let Some((first, end)) = arcade_visible_window_range_px(
        games.len(),
        arcade_visual_px(visual_index, ARCADE_ROW_HEIGHT, 1),
        ARCADE_ROW_HEIGHT,
        ArcadeListRenderer::default_selection_y(),
        ARCADE_LIST_H,
    ) else {
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

fn arcade_filter_visible_window_hash(
    items: &[ArcadeListItem],
    visual_px: i32,
    row_height: i32,
) -> u64 {
    let mut hash = ARCADE_LIST_HASH_OFFSET;
    let Some((first, end)) = arcade_visible_window_range_px(
        items.len(),
        visual_px,
        row_height,
        ArcadeListRenderer::selection_y_for_height(ARCADE_LIST_H, row_height),
        ARCADE_LIST_H,
    ) else {
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

fn arcade_filter_content_hash(items: &[ArcadeListItem], acknowledged_indices: &[usize]) -> u64 {
    let mut hash = ARCADE_LIST_HASH_OFFSET;
    arcade_hash_usize(&mut hash, items.len());
    for (idx, item) in items.iter().enumerate() {
        arcade_hash_usize(&mut hash, idx);
        arcade_hash_bytes(&mut hash, item.title.as_bytes());
        arcade_hash_usize(&mut hash, item.count.unwrap_or(usize::MAX));
        arcade_hash_bytes(&mut hash, &[item.active as u8]);
    }
    for index in acknowledged_indices {
        arcade_hash_usize(&mut hash, *index);
    }
    hash
}

fn arcade_game_hash(game: &ArcadeGameEntry) -> u64 {
    let mut hash = ARCADE_LIST_HASH_OFFSET;
    arcade_hash_game(&mut hash, game);
    hash
}

fn arcade_visual_px(visual_index: f32, row_height: i32, quantum_y: i32) -> i32 {
    if !visual_index.is_finite() {
        return 0;
    }
    let quantum_y = quantum_y.max(1);
    ((visual_index * row_height.max(1) as f32 / quantum_y as f32)
        .round()
        .max(0.0) as i32)
        .saturating_mul(quantum_y)
}

fn arcade_anchor_for_visual_px(len: usize, visual_px: i32, row_height: i32) -> usize {
    if len == 0 {
        return 0;
    }
    let row_h = row_height.max(1);
    let anchor = (visual_px.max(0) + row_h / 2).div_euclid(row_h);
    (anchor as usize).min(len - 1)
}

fn arcade_row_y(idx: usize, visual_px: i32, selection_y: usize, row_height: i32) -> isize {
    selection_y as isize + idx as isize * row_height.max(1) as isize - visual_px.max(0) as isize
}

fn arcade_visible_window_range_px(
    len: usize,
    visual_px: i32,
    row_height: i32,
    selection_y: usize,
    visible_height: usize,
) -> Option<(usize, usize)> {
    if len == 0 {
        return None;
    }
    let row_h = i64::from(row_height.max(1));
    let visual_px = i64::from(visual_px.max(0));
    let selection_y = selection_y as i64;
    let visible_height = visible_height as i64;
    let first = (visual_px - selection_y - row_h).div_euclid(row_h) + 1;
    let last = (visual_px - selection_y + visible_height - 1).div_euclid(row_h);
    let first = first.max(0) as usize;
    let last = last.max(0) as usize;
    Some((first.min(len - 1), last.min(len - 1)))
}

fn arcade_filter_gradient(style: ArcadeListStyle, active: bool) -> TextGradient {
    if style.crt_palette {
        TextGradient::new(style.text, style.text, style.text)
    } else if active {
        ARCADE_FILTER_ACTIVE_GRADIENT
    } else {
        ARCADE_TITLE_GRADIENT
    }
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

pub fn draw_arcade_row_background(row: &mut [Pixel], width: usize, idx: usize) {
    draw_arcade_row_background_with_style(row, width, idx, ArcadeListStyle::hdmi());
}

fn draw_arcade_row_background_with_style(
    row: &mut [Pixel],
    width: usize,
    idx: usize,
    style: ArcadeListStyle,
) {
    let bg = if idx.is_multiple_of(2) {
        style.background
    } else {
        style.alternate_background
    };
    let row_height = style.row_height.max(1) as usize;
    for row_y in 0..row_height {
        let dy = row_y;
        let line = &mut row[dy * width..(dy + 1) * width];
        for px in line.iter_mut() {
            *px = bg;
        }
        if row_y < style.separator_top || row_y >= row_height.saturating_sub(style.separator_bottom)
        {
            for px in line.iter_mut() {
                *px = style.border;
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
    selected_aperture_pixel_with_style(pixel, ArcadeListStyle::hdmi())
}

fn selected_aperture_pixel_with_style(pixel: Rgb565Pixel, style: ArcadeListStyle) -> Rgb565Pixel {
    if is_arcade_row_background_pixel_with_style(pixel, style) {
        style.selection_fill_565
    } else if style.crt_palette {
        style.selection_text_565
    } else {
        invert_rgb565(pixel)
    }
}

fn is_arcade_row_background_pixel(pixel: Rgb565Pixel) -> bool {
    is_arcade_row_background_pixel_with_style(pixel, ArcadeListStyle::hdmi())
}

fn is_arcade_row_background_pixel_with_style(pixel: Rgb565Pixel, style: ArcadeListStyle) -> bool {
    matches!(
        pixel,
        value if value == style.background_565
            || value == style.alternate_background_565
            || value == style.border_565
            || value == style.badge_fill_565
    )
}

fn is_arcade_unselected_fill_pixel_with_style(pixel: Rgb565Pixel, style: ArcadeListStyle) -> bool {
    pixel == style.background_565 || pixel == style.alternate_background_565
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framebuffer::target::FramebufferTargetGeometry;
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

    fn filter_items(labels: &[&str]) -> Vec<ArcadeListItem> {
        labels
            .iter()
            .map(|title| ArcadeListItem {
                title: (*title).to_string(),
                count: Some(1),
                active: false,
            })
            .collect()
    }

    fn crt_240_display() -> UiDisplay {
        let plan = crate::ui_display::UiDisplayPlan::from_mister_ini_text(
            "[MiSTer]\ndirect_video=1\nmenu_pal=0\nforced_scandoubler=0\n",
        )
        .expect("CRT240 display plan");
        UiDisplay::for_plan(plan)
    }

    #[test]
    fn portrait_cached_composition_matches_logical_arcade_pixels() {
        let mut logical_renderer = ArcadeListRenderer::new();
        for (index, pixel) in logical_renderer.surface.iter_mut().enumerate() {
            *pixel = Rgb565Pixel(index as u16);
        }
        let mut oriented_renderer = ArcadeListRenderer::new();
        oriented_renderer
            .surface
            .copy_from_slice(&logical_renderer.surface);
        let logical_layout = Rgb565OutputLayout::new(
            540,
            960,
            540,
            mister_magik_framebuffer_scenes::OutputRotation::None,
        )
        .unwrap();
        let oriented_layout = Rgb565OutputLayout::new(
            540,
            960,
            960,
            mister_magik_framebuffer_scenes::OutputRotation::CounterClockwise90,
        )
        .unwrap();
        let mut logical_target = UiFrameTarget::cached(FramebufferTargetGeometry::new(540, 960));
        let mut oriented_target = UiFrameTarget::cached(FramebufferTargetGeometry::new(960, 540));

        logical_renderer.compose_layer_to_oriented_cached(
            &mut logical_target,
            logical_layout,
            true,
        );
        oriented_renderer.compose_layer_to_oriented_cached(
            &mut oriented_target,
            oriented_layout,
            true,
        );

        let dirty = logical_renderer.dirty_rect();
        for y in dirty.y0..dirty.y1 {
            for x in dirty.x0..dirty.x1 {
                assert_eq!(
                    logical_target.cached_565()[logical_layout.physical_offset(x, y)],
                    oriented_target.cached_565()[oriented_layout.physical_offset(x, y)],
                    "logical arcade pixel ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn backdrop_composition_restores_every_unselected_viewport_pixel() {
        let display = crt_240_display();
        let metrics = CrtUiMetrics::for_display(&display);
        let mut renderer = ArcadeListRenderer::new_for_crt_display(metrics, &display);
        let geometry = ArcadeListGeometry::crt_for_content(display.content_rect(), metrics, false);
        renderer.set_geometry_for_render_h(geometry, display.content_rect().bottom());
        let games = games("arcade", 24);
        renderer.draw(ArcadeGameView::contiguous(&games), 0, 0.0, true);

        let layout = crate::ui_display::UiLayoutGeometry::for_display(
            &display,
            crate::ui_display::ScreenOrientation::Normal,
        )
        .output_layout();
        let sentinel = Rgb565Pixel(0xdead);
        let backdrop = (0..layout.len())
            .map(|index| Rgb565Pixel((index as u16).wrapping_mul(17)))
            .collect::<Vec<_>>();
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(640, 480));
        target.cached_565_mut().fill(sentinel);
        assert!(renderer.compose_layer_over_backdrop_to_oriented_cached(
            &mut target,
            &backdrop,
            layout,
            true,
        ));

        renderer.draw(
            ArcadeGameView::contiguous(&games),
            1,
            metrics.game_row_height as f32,
            false,
        );
        target.cached_565_mut().fill(sentinel);
        assert!(renderer.compose_layer_over_backdrop_to_oriented_cached(
            &mut target,
            &backdrop,
            layout,
            true,
        ));

        let dirty = renderer.dirty_rect();
        for y in dirty.y0..dirty.y1 {
            for x in dirty.x0..dirty.x1 {
                assert_ne!(
                    target.cached_565()[layout.physical_offset(x, y)],
                    sentinel,
                    "viewport pixel ({x}, {y}) was not restored"
                );
            }
        }
        let unselected_y = dirty.y0 + metrics.game_row_height as usize * 4 + 2;
        let unselected_x = dirty.x0 + 2;
        let offset = layout.physical_offset(unselected_x, unselected_y);
        assert_eq!(target.cached_565()[offset], backdrop[offset]);
    }

    fn surface_in_viewport_order(renderer: &ArcadeListRenderer) -> Vec<Rgb565Pixel> {
        let mut pixels = Vec::with_capacity(renderer.width * ARCADE_LIST_H);
        for y in 0..ARCADE_LIST_H {
            let src_y = (renderer.surface_y + y) % ARCADE_LIST_H;
            let src = src_y * renderer.width;
            pixels.extend_from_slice(&renderer.surface[src..src + renderer.width]);
        }
        pixels
    }

    #[test]
    fn search_geometry_right_aligns_to_render_width() {
        assert_eq!(
            ArcadeListGeometry::search_for_render_w(960),
            ArcadeListGeometry {
                x: 488,
                y: ARCADE_SEARCH_LIST_Y,
                width: ARCADE_SEARCH_LIST_W,
            }
        );
        let search = ArcadeListGeometry::search_for_render_w(960);
        assert_eq!(search.x, 960 / 2 + ARCADE_LIST_X);
        assert_eq!(search.x + search.width, 960 - ARCADE_LIST_X);
        for render_w in [320, 384] {
            let search = ArcadeListGeometry::search_for_render_w(render_w);
            assert!(search.x >= render_w * 2 / 5);
            assert!(search.x + search.width <= render_w);
            assert!(search.width > 0);
        }
        assert_eq!(
            ArcadeListGeometry::search_for_render_w(1280),
            ArcadeListGeometry {
                x: 648,
                y: ARCADE_SEARCH_LIST_Y,
                width: 624,
            }
        );
    }

    #[test]
    fn crt_geometry_uses_31khz_metrics_at_640x480() {
        let content = CrtContentRect {
            x: 0,
            y: 0,
            width: 640,
            height: 480,
        };
        let metrics = CrtUiMetrics::for_framebuffer(640, 480);
        let expected_visible_height = 384;
        let geometry = ArcadeListGeometry::crt_for_content(content, metrics, false);
        assert_eq!(
            geometry,
            ArcadeListGeometry {
                x: 8,
                y: 60,
                width: 624,
            }
        );
        assert_eq!(
            geometry.visible_height_with_metrics(480, Some(metrics)),
            expected_visible_height
        );

        let search = ArcadeListGeometry::crt_for_content(content, metrics, true);
        assert_eq!(
            search,
            ArcadeListGeometry {
                x: 272,
                y: 60,
                width: 360,
            }
        );
        assert_eq!(
            search.visible_height_with_metrics(480, Some(metrics)),
            expected_visible_height
        );
    }

    #[test]
    fn crt_640_window_clips_to_sixteen_complete_24px_rows() {
        let geometry = ArcadeListGeometry::crt_for_content(
            CrtContentRect {
                x: 0,
                y: 0,
                width: 640,
                height: 480,
            },
            CrtUiMetrics::for_framebuffer(640, 480),
            false,
        );
        let mut renderer = ArcadeListRenderer::new_for_crt(24);
        renderer.set_geometry_for_render_h(geometry, 480);

        assert_eq!(renderer.visible_height, 384);
        assert_eq!(
            renderer.visible_height / renderer.style.row_height as usize,
            16
        );
        assert_eq!(
            renderer.visible_height % renderer.style.row_height as usize,
            0
        );
        assert_eq!(
            renderer.dirty_rect(),
            DirtyRect {
                x0: 8,
                y0: 60,
                x1: 632,
                y1: 444,
            }
        );
        let selection = renderer.selection_rect();
        assert_eq!(selection.y1 - selection.y0, 24);
        assert!(selection.y0 >= renderer.dirty_rect().y0);
        assert!(selection.y1 <= renderer.dirty_rect().y1);

        assert_eq!(
            arcade_visible_window_range_px(
                100,
                50 * renderer.style.row_height,
                renderer.style.row_height,
                renderer.selection_y(),
                renderer.visible_height,
            ),
            Some((43, 58))
        );
    }

    #[test]
    fn crt_240_scroll_offsets_are_quantized_to_physical_scanlines() {
        let display = crt_240_display();
        let metrics = CrtUiMetrics::for_display(&display);
        let mut renderer = ArcadeListRenderer::new_for_crt_display(metrics, &display);
        let games = games("intellivision", 20);

        assert!(matches!(
            renderer.draw(ArcadeGameView::contiguous(&games), 0, 0.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
        for raw_visual_px in 1..metrics.game_row_height {
            let visual_index = raw_visual_px as f32 / metrics.game_row_height as f32;
            let update = renderer.draw(ArcadeGameView::contiguous(&games), 0, visual_index, false);
            assert_eq!(renderer.last_draw.expect("draw key").visual_px % 2, 0);
            if let Some(ArcadeListUpdate::Scroll { delta_y, .. }) = update {
                assert_eq!(delta_y % 2, 0);
            }
        }

        let mut native_metrics = metrics;
        native_metrics.game_row_height = 19;
        let mut native = ArcadeListRenderer::new_for_crt_metrics(native_metrics);
        assert!(
            native
                .draw(ArcadeGameView::contiguous(&games), 0, 1.0 / 19.0, false,)
                .is_some()
        );
        assert_eq!(native.last_draw.expect("native draw key").visual_px, 1);
    }

    #[test]
    fn crt_240_row_separator_downsamples_to_one_framebuffer_row() {
        let display = crt_240_display();
        let metrics = CrtUiMetrics::for_display(&display);
        let mut renderer = ArcadeListRenderer::new_for_crt_display(metrics, &display);
        let row = renderer.render_row("MagiK 1984", false, false, 0);
        let width = renderer.width;
        let row_height = metrics.game_row_height as usize;
        let mut destination = vec![Rgb565Pixel(0); width * row_height / 2];
        let transform = mister_magik_fb::framebuffer::vertical_scale::VerticalRgb565Transform::new(
            width,
            row_height,
            row_height / 2,
        )
        .expect("CRT240 row transform");
        transform
            .copy_rect(
                mister_magik_fb::framebuffer::vertical_scale::Rgb565FrameView {
                    pixels: &row,
                    width,
                    height: row_height,
                    stride_pixels: width,
                },
                mister_magik_fb::framebuffer::vertical_scale::VerticalRect {
                    x0: 0,
                    y0: 0,
                    x1: width,
                    y1: row_height,
                },
                &mut destination,
                width,
            )
            .expect("row copy")
            .expect("row destination");

        let border_rows = destination
            .chunks(width)
            .enumerate()
            .filter(|(_, row)| row[0] == renderer.style.border_565)
            .map(|(y, _)| y)
            .collect::<Vec<_>>();
        assert_eq!(border_rows, vec![0]);
    }

    #[test]
    fn crt_240_selection_frame_downsamples_to_one_pixel_on_both_axes() {
        let display = crt_240_display();
        let metrics = CrtUiMetrics::for_display(&display);
        let mut renderer = ArcadeListRenderer::new_for_crt_display(metrics, &display);
        let geometry = ArcadeListGeometry::crt_for_content(display.content_rect(), metrics, false);
        renderer.set_geometry_for_render_h(geometry, display.render_h());
        let sentinel = Rgb565Pixel(0x1234);
        let mut target = UiFrameTarget::cached(
            mister_magik_fb::framebuffer::target::FramebufferTargetGeometry::new(640, 480),
        );
        target.cached_565_mut().fill(sentinel);
        renderer.compose_selection_frame_to_cached(&mut target);

        let transform = mister_magik_fb::framebuffer::vertical_scale::VerticalRgb565Transform::new(
            640, 480, 240,
        )
        .expect("CRT240 frame transform");
        let mut destination = vec![sentinel; 640 * 240];
        transform
            .copy_rect(
                mister_magik_fb::framebuffer::vertical_scale::Rgb565FrameView {
                    pixels: target.cached_565(),
                    width: 640,
                    height: 480,
                    stride_pixels: 640,
                },
                mister_magik_fb::framebuffer::vertical_scale::VerticalRect {
                    x0: 0,
                    y0: 0,
                    x1: 640,
                    y1: 480,
                },
                &mut destination,
                640,
            )
            .expect("frame copy")
            .expect("frame destination");
        let destination_rect = transform
            .destination_rect_for_source(
                mister_magik_fb::framebuffer::vertical_scale::VerticalRect {
                    x0: renderer.selection_rect().x0,
                    y0: renderer.selection_rect().y0,
                    x1: renderer.selection_rect().x1,
                    y1: renderer.selection_rect().y1,
                },
            )
            .expect("selection destination");
        let color = renderer.style.selection_frame_565;
        let center_x = (destination_rect.x0 + destination_rect.x1) / 2;
        let horizontal_rows = (destination_rect.y0..destination_rect.y1)
            .filter(|&y| destination[y * 640 + center_x] == color)
            .collect::<Vec<_>>();
        assert_eq!(horizontal_rows.len(), 2);
        assert_eq!(
            horizontal_rows[1] - horizontal_rows[0],
            destination_rect.rows() - 1
        );

        let center_y = (destination_rect.y0 + destination_rect.y1) / 2;
        let vertical_columns = (destination_rect.x0..destination_rect.x1)
            .filter(|&x| destination[center_y * 640 + x] == color)
            .collect::<Vec<_>>();
        assert_eq!(vertical_columns.len(), 2);
        assert_eq!(
            vertical_columns[1] - vertical_columns[0],
            destination_rect.width() - 1
        );
    }

    #[test]
    fn direct_layer_dirty_rect_is_bounded_at_crt_heights() {
        let mut renderer = ArcadeListRenderer::new();
        for (width, height) in [(320, 240), (384, 288)] {
            renderer
                .set_geometry_for_render_h(ArcadeListGeometry::normal_for_render_w(width), height);
            let rect = renderer.dirty_rect();
            assert!(rect.x1 <= width);
            assert!(rect.y1 <= height - 32);
            let selection = renderer.selection_rect();
            assert!(selection.y0 >= rect.y0);
            assert!(selection.y1 <= rect.y1);

            renderer
                .set_geometry_for_render_h(ArcadeListGeometry::search_for_render_w(width), height);
            let rect = renderer.dirty_rect();
            assert!(rect.x1 <= width);
            assert!(rect.y1 <= height - 32);
            let selection = renderer.selection_rect();
            assert!(selection.y0 >= rect.y0);
            assert!(selection.y1 <= rect.y1);
        }
    }

    #[test]
    fn compact_surface_reads_wrap_at_visible_height() {
        let mut renderer = ArcadeListRenderer::new();
        renderer.set_geometry_for_render_h(ArcadeListGeometry::normal_for_render_w(320), 240);
        for row in 0..renderer.visible_height {
            renderer.surface[row * renderer.width..(row + 1) * renderer.width]
                .fill(Rgb565Pixel(row as u16));
        }
        renderer.surface_y = renderer.visible_height - 1;
        let sample = renderer.prepare_inverted_surface_chunk(0, 0, 1, 2);
        assert_eq!(sample.len(), 2);
        assert_ne!(sample[0], sample[1]);
        assert_eq!(renderer.visible_height, 152);
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
        assert!(
            renderer
                .draw(ArcadeGameView::contiguous(&games), 7, 7.0, false)
                .is_none()
        );

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
    fn equal_length_filter_transition_with_position_change_matches_fresh_redraw() {
        let top = filter_items(&[
            "Games A-Z",
            "Search",
            "Decades",
            "Manufacturer",
            "Players",
            "Controls",
        ]);
        let decades = filter_items(&["1970's", "1980's", "1990's", "2000's", "2010's", "2020's"]);

        let mut transitioned = ArcadeListRenderer::new();
        assert!(matches!(
            transitioned.draw_filter_items(&top, 2, 2.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
        let update = transitioned.draw_filter_items(&decades, 0, 0.0, false);
        let transitioned_pixels = surface_in_viewport_order(&transitioned);

        let mut fresh = ArcadeListRenderer::new();
        assert!(matches!(
            fresh.draw_filter_items(&decades, 0, 0.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
        let fresh_pixels = surface_in_viewport_order(&fresh);

        assert!(matches!(update, Some(ArcadeListUpdate::Full(_))));
        assert_eq!(transitioned_pixels, fresh_pixels);
    }

    #[test]
    fn filter_row_metadata_change_forces_full_redraw() {
        let mut renderer = ArcadeListRenderer::new();
        let mut items = filter_items(&["1970's", "1980's", "1990's"]);
        assert!(matches!(
            renderer.draw_filter_items(&items, 1, 1.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));

        items[0].count = Some(99);
        assert!(matches!(
            renderer.draw_filter_items(&items, 0, 0.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));

        items[0].active = true;
        assert!(matches!(
            renderer.draw_filter_items(&items, 1, 1.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
    }

    #[test]
    fn unchanged_filter_content_keeps_incremental_scroll_path() {
        let mut renderer = ArcadeListRenderer::new();
        let items = filter_items(&["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L"]);
        assert!(matches!(
            renderer.draw_filter_items(&items, 0, 0.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));

        assert!(matches!(
            renderer.draw_filter_items(&items, 1, 1.0 / ARCADE_ROW_HEIGHT as f32, false,),
            Some(ArcadeListUpdate::Scroll { .. })
        ));
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
        let row = renderer.render_row("MAGIK", false, false, 0);
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
            ARCADE_LIST_Y + ArcadeListRenderer::default_selection_y()
        );
        assert_eq!(rect.y1 - rect.y0, ARCADE_ROW_HEIGHT as usize);

        assert!(matches!(
            renderer.draw(ArcadeGameView::contiguous(&games), 2, 1.25, false),
            Some(ArcadeListUpdate::Scroll { .. })
        ));
        let rect = renderer.selection_rect();
        assert_eq!(
            rect.y0,
            ARCADE_LIST_Y + ArcadeListRenderer::default_selection_y()
        );
    }

    #[test]
    fn arcade_present_segments_invert_selected_row_inner_and_skip_frame_pixels() {
        let mut segments = Vec::new();

        for_each_arcade_list_present_segment(
            ARCADE_LIST_W,
            0,
            ARCADE_LIST_H,
            |kind, x, y, w, h| {
                segments.push((kind, x, y, w, h));
            },
        );

        assert_eq!(
            segments,
            vec![
                (ArcadeListPresentKind::Normal, 0, 0, ARCADE_LIST_W, 192),
                (
                    ArcadeListPresentKind::Inverted,
                    ARCADE_HDMI_SELECTION_FRAME_THICKNESS,
                    195,
                    ARCADE_LIST_W - ARCADE_HDMI_SELECTION_FRAME_THICKNESS * 2,
                    42
                ),
                (ArcadeListPresentKind::Normal, 0, 240, ARCADE_LIST_W, 240),
            ]
        );

        let copied_px = segments.iter().map(|(_, _, _, w, h)| w * h).sum::<usize>();
        let skipped_px = ARCADE_LIST_W * ARCADE_LIST_H - copied_px;
        let frame_px = ARCADE_LIST_W * ARCADE_HDMI_SELECTION_FRAME_THICKNESS * 2
            + (ARCADE_ROW_HEIGHT as usize - ARCADE_HDMI_SELECTION_FRAME_THICKNESS * 2)
                * ARCADE_HDMI_SELECTION_FRAME_THICKNESS
                * 2;
        let frame_present_px = ARCADE_LIST_W * ARCADE_HDMI_SELECTION_FRAME_THICKNESS * 2
            + ARCADE_ROW_HEIGHT as usize * ARCADE_HDMI_SELECTION_FRAME_THICKNESS * 2;

        assert_eq!(skipped_px, frame_px);
        assert_eq!(
            arcade_list_present_pixels(
                &ArcadeListUpdate::Full(DirtyRect {
                    x0: 0,
                    y0: 0,
                    x1: ARCADE_LIST_W,
                    y1: ARCADE_LIST_H,
                }),
                ARCADE_LIST_W,
                true
            ),
            copied_px + frame_present_px
        );
        assert_eq!(
            arcade_list_present_pixels(
                &ArcadeListUpdate::Scroll {
                    delta_y: 12,
                    rect: DirtyRect {
                        x0: 0,
                        y0: 0,
                        x1: ARCADE_LIST_W,
                        y1: ARCADE_LIST_H,
                    },
                },
                ARCADE_LIST_W,
                false
            ),
            copied_px
        );
    }

    #[test]
    fn arcade_present_segments_keep_fixed_selection_aperture_for_partial_bands() {
        let mut segments = Vec::new();
        let viewport_y = ArcadeListRenderer::default_selection_y() + 10;

        for_each_arcade_list_present_segment(ARCADE_LIST_W, viewport_y, 20, |kind, x, y, w, h| {
            segments.push((kind, x, y, w, h));
        });

        assert_eq!(
            segments,
            vec![(
                ArcadeListPresentKind::Inverted,
                ARCADE_HDMI_SELECTION_FRAME_THICKNESS,
                viewport_y,
                ARCADE_LIST_W - ARCADE_HDMI_SELECTION_FRAME_THICKNESS * 2,
                20
            )]
        );
    }

    #[test]
    fn search_layout_rebuilds_the_renderer_at_its_own_width() {
        let mut renderer = ArcadeListRenderer::new();
        let games = games("arcade", 3);
        assert!(matches!(
            renderer.draw(ArcadeGameView::contiguous(&games), 0, 0.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));

        renderer.set_geometry(ArcadeListGeometry::search_for_render_w(960));
        assert_eq!(renderer.width(), ARCADE_SEARCH_LIST_W);
        assert_eq!(renderer.surface.len(), ARCADE_SEARCH_LIST_W * ARCADE_LIST_H);
        assert!(renderer.row_cache.is_empty());
        assert!(matches!(
            renderer.draw(ArcadeGameView::contiguous(&games), 0, 0.0, false),
            Some(ArcadeListUpdate::Full(rect)) if rect.x0 == 488 && rect.x1 == 952
        ));
        assert_eq!(
            renderer.selection_rect().x1 - renderer.selection_rect().x0,
            ARCADE_SEARCH_LIST_W
        );
    }

    #[test]
    fn search_present_segments_and_accounting_use_the_narrow_width() {
        let mut segments = Vec::new();
        for_each_arcade_list_present_segment(
            ARCADE_SEARCH_LIST_W,
            0,
            ARCADE_LIST_H,
            |_, _, _, w, h| segments.push((w, h)),
        );
        assert!(
            segments
                .iter()
                .all(|&(width, _)| width <= ARCADE_SEARCH_LIST_W)
        );

        let update = ArcadeListUpdate::Full(DirtyRect {
            x0: 488,
            y0: ARCADE_SEARCH_LIST_Y,
            x1: 952,
            y1: ARCADE_SEARCH_LIST_Y + ARCADE_LIST_H,
        });
        let expected = segments.iter().map(|&(w, h)| w * h).sum::<usize>()
            + ARCADE_SEARCH_LIST_W * ARCADE_HDMI_SELECTION_FRAME_THICKNESS * 2
            + ARCADE_HDMI_SELECTION_FRAME_THICKNESS * ARCADE_ROW_HEIGHT as usize * 2;
        assert_eq!(
            arcade_list_present_pixels(&update, ARCADE_SEARCH_LIST_W, true),
            expected
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
    fn crt_renderer_uses_compact_rows_and_card_palette_without_changing_hdmi() {
        let crt = ArcadeListRenderer::new_for_crt(24);
        let hdmi = ArcadeListRenderer::new();

        assert_eq!(crt.style.row_height, 24);
        assert_eq!(crt.style.title_typeface, ConsoleTypeface::Nocive15);
        assert_eq!(crt.style.meta_typeface, ConsoleTypeface::PressStart2P);
        assert!(crt.style.crt_palette);
        assert_eq!(crt.style.background.0, 0x00020817);
        assert_eq!(
            crt.style.selection_fill_565,
            rgb565_from_rgb888(0x40, 0xe5, 0xe7)
        );
        assert_eq!(crt.style.badge_fill.0, 0x0040e5e7);
        assert_eq!(crt.style.badge_text.0, 0x0003132d);
        assert_eq!(hdmi.style.row_height, ARCADE_ROW_HEIGHT);
        assert_eq!(hdmi.style.title_typeface, ConsoleTypeface::Nocive15);
        assert_eq!(hdmi.style.meta_typeface, ConsoleTypeface::PressStart2P);
        assert!(!hdmi.style.crt_palette);
        assert_eq!(hdmi.style.background.0, ARCADE_LIST_BG_COLOR.0);
        assert_eq!(hdmi.style.badge_fill.0, ARCADE_NEW_BADGE_FILL.0);
    }

    #[test]
    fn crt_new_badges_are_centered_inside_every_route_row() {
        for (row_height, font_family, raster) in [
            (
                32,
                CrtFontFamily::PressStart2P,
                ArcadeListRasterMetrics {
                    scroll_quantum_y: 2,
                    separator_y: 2,
                    selection_frame_x: 1,
                    selection_frame_y: 2,
                },
            ),
            (
                19,
                CrtFontFamily::PressStart2P,
                ArcadeListRasterMetrics::native_crt(),
            ),
            (
                32,
                CrtFontFamily::PressStart2P,
                ArcadeListRasterMetrics::native_crt(),
            ),
            (
                39,
                CrtFontFamily::PressStart2P,
                ArcadeListRasterMetrics::native_crt(),
            ),
        ] {
            let mut metrics = CrtUiMetrics::for_framebuffer(640, 480);
            metrics.game_row_height = row_height;
            metrics.font_family = font_family;
            let style = ArcadeListStyle::crt_with_raster(metrics, raster);
            let mut renderer = ArcadeListRenderer::new_with_style(style, Some(metrics));
            let row = renderer.render_row("MagiK", true, false, 0);
            let sample_x = renderer.width - 50;
            let badge_rows = row
                .chunks(renderer.width)
                .enumerate()
                .filter(|(_, row)| row[sample_x] == style.badge_fill_565)
                .map(|(y, _)| y)
                .collect::<Vec<_>>();
            let top = *badge_rows.first().expect("badge top");
            let bottom = *badge_rows.last().expect("badge bottom");
            let top_padding = top - style.separator_top;
            let bottom_padding = row_height as usize - style.separator_bottom - bottom - 1;
            assert!(top_padding.abs_diff(bottom_padding) <= 1);

            let badge_text = pixel_to_rgb565(style.badge_text);
            let text_rows = row
                .chunks(renderer.width)
                .enumerate()
                .filter(|(_, row)| row.iter().any(|pixel| *pixel == badge_text))
                .map(|(y, _)| y)
                .collect::<Vec<_>>();
            let text_top = *text_rows.first().expect("badge text top");
            let text_bottom = *text_rows.last().expect("badge text bottom");
            assert!(
                (text_top - top).abs_diff(bottom - text_bottom) <= 1,
                "row height {row_height}"
            );
        }
    }

    #[test]
    fn crt_palette_behavior_remains_distinct_from_hdmi() {
        let crt = ArcadeListStyle::crt(CrtUiMetrics::for_framebuffer(640, 480));
        let hdmi = ArcadeListStyle::hdmi();
        let flat_crt = TextGradient::new(crt.text, crt.text, crt.text);

        assert_eq!(arcade_filter_gradient(crt, false), flat_crt);
        assert_eq!(arcade_filter_gradient(crt, true), flat_crt);
        assert_eq!(arcade_filter_gradient(hdmi, false), ARCADE_TITLE_GRADIENT);
        assert_eq!(
            arcade_filter_gradient(hdmi, true),
            ARCADE_FILTER_ACTIVE_GRADIENT
        );

        let crt_text = pixel_to_rgb565(crt.text);
        assert_eq!(
            selected_aperture_pixel_with_style(crt.background_565, crt),
            crt.selection_fill_565
        );
        assert_eq!(
            selected_aperture_pixel_with_style(crt_text, crt),
            crt.selection_text_565
        );
        let hdmi_text = pixel_to_rgb565(hdmi.text);
        assert_eq!(
            selected_aperture_pixel_with_style(hdmi_text, hdmi),
            invert_rgb565(hdmi_text)
        );
    }

    #[test]
    fn preparing_inverted_selection_chunk_flattens_chrome_without_mutating_surface() {
        let mut renderer = ArcadeListRenderer::new();
        for (idx, pixel) in renderer.surface.iter_mut().enumerate() {
            *pixel = Rgb565Pixel(idx as u16);
        }
        renderer.surface_y = 5;
        let x = ARCADE_HDMI_SELECTION_FRAME_THICKNESS;
        let y = ArcadeListRenderer::default_selection_y() + ARCADE_HDMI_SELECTION_FRAME_THICKNESS;
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
                    is_favourite: false,
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
        assert!(
            renderer
                .draw(ArcadeGameView::contiguous(&games), 7, 7.0, false)
                .is_none()
        );

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
