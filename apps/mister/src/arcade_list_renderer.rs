// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use crate::arcade_catalog::{
    ARCADE_LIST_VISIBLE_H, ARCADE_ROW_HEIGHT, ArcadeGameEntry, ArcadeGameView,
};
use crate::bitmap_text::{ConsoleFont, ConsoleGlyphRowFilter, ConsoleTypeface, TextGradient};
use crate::framebuffer::mapped::{MappedRgb565Framebuffer, Pixel, pixel_to_rgb565};
use crate::framebuffer::present::{copy_dense_rect_565, copy_strided_rect_565};
use crate::framebuffer::scanout_slots::ScanoutSlotsRgb565Framebuffer;
use crate::framebuffer::target::{
    DirtyRect, PhysicalLayerBacking, PhysicalLayerCopyDecision, PhysicalLayerCopyTrace,
    PhysicalLayerView, UiFrameTarget, shift_physical_rect,
};
use crate::ui_display::{
    CrtContentRect, CrtFontExperiment, CrtFontFamily, CrtUiMetrics, ResolvedOutputRoute, UiDisplay,
    UiLayoutGeometry,
};
use mister_magik_framebuffer_scenes::{
    OutputRotation, Rgb565OutputLayout, Rgb565Rect, Rgb565RegionLayout, Rgb565RegionSurfaceMut,
    Rgb565SurfaceMut,
};
use slint::platform::software_renderer::Rgb565Pixel;

pub use crate::arcade_physical_layer::{
    PersistentArcadeLayerDiagnostic, PersistentOrientedArcadeLayer,
    PersistentOrientedArcadeLayerKey,
};

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
const CRT_PORTRAIT_TITLE_CLEARANCE_ROWS: usize = 3;
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

pub const fn crt_arcade_row_height(base_row_height: i32, portrait: bool) -> i32 {
    if portrait {
        base_row_height.saturating_mul(2)
    } else {
        base_row_height
    }
}

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
    title_font_px: f32,
    meta_font_px: f32,
    title_typeface: ConsoleTypeface,
    meta_typeface: ConsoleTypeface,
    glyph_row_filter: ConsoleGlyphRowFilter,
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
        if display.output_route() == ResolvedOutputRoute::Crt240p60
            && !display.is_native_composition()
        {
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
            title_font_px: ARCADE_LIST_FONT_PX,
            meta_font_px: ARCADE_LIST_META_FONT_PX,
            title_typeface: ConsoleTypeface::Nocive15,
            meta_typeface: ConsoleTypeface::PressStart2P,
            glyph_row_filter: ConsoleGlyphRowFilter::Native,
            crt_palette: false,
        }
    }

    const fn crt(metrics: CrtUiMetrics) -> Self {
        Self::crt_with_raster(metrics, ArcadeListRasterMetrics::native_crt())
    }

    fn crt_for_display(metrics: CrtUiMetrics, display: &UiDisplay) -> Self {
        let mut style =
            Self::crt_with_raster(metrics, ArcadeListRasterMetrics::for_display(display));
        if display.output_route() == ResolvedOutputRoute::Crt240p60
            && !display.is_native_composition()
        {
            style.glyph_row_filter = match display.crt_font_experiment() {
                CrtFontExperiment::CoverageMax => ConsoleGlyphRowFilter::PairwiseMax,
                CrtFontExperiment::DominantRow => ConsoleGlyphRowFilter::PairwiseDominant,
                CrtFontExperiment::Xerxes => {
                    style.title_typeface = ConsoleTypeface::Xerxes10;
                    ConsoleGlyphRowFilter::Native
                }
                CrtFontExperiment::XerxesPerfect => {
                    style.title_font_px = 32.0;
                    style.title_typeface = ConsoleTypeface::Xerxes10Perfect;
                    ConsoleGlyphRowFilter::Native
                }
                CrtFontExperiment::YesterdayPerfect => {
                    style.title_font_px = 32.0;
                    style.title_typeface = ConsoleTypeface::Yesterday10Perfect;
                    ConsoleGlyphRowFilter::Native
                }
                CrtFontExperiment::Bacteria => {
                    style.title_font_px = 32.0;
                    style.title_typeface = ConsoleTypeface::Bacteria12;
                    ConsoleGlyphRowFilter::Native
                }
                CrtFontExperiment::BacteriaHalf => {
                    style.title_font_px = 16.0;
                    style.title_typeface = ConsoleTypeface::Bacteria12Half;
                    ConsoleGlyphRowFilter::Native
                }
                CrtFontExperiment::Baseline | CrtFontExperiment::PhaseEven => {
                    style.title_font_px = 32.0;
                    style.title_typeface = ConsoleTypeface::Yesterday10Perfect;
                    ConsoleGlyphRowFilter::Native
                }
            };
        } else if matches!(
            display.output_route(),
            ResolvedOutputRoute::Crt240p60 | ResolvedOutputRoute::Crt288p50
        ) {
            style.title_font_px = 16.0;
            style.title_typeface = ConsoleTypeface::Yesterday10;
            style.glyph_row_filter = ConsoleGlyphRowFilter::Native;
        }
        style
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
            title_font_px: ARCADE_LIST_FONT_PX,
            meta_font_px: 12.0,
            title_typeface: ConsoleTypeface::Nocive15,
            meta_typeface: match metrics.font_family {
                CrtFontFamily::Spleen6x12 => ConsoleTypeface::Spleen6x12Small,
            },
            glyph_row_filter: ConsoleGlyphRowFilter::Native,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrtArcadeLayout {
    pub header: CrtContentRect,
    pub list: CrtContentRect,
    pub footer: CrtContentRect,
    pub search_keyboard: Option<CrtContentRect>,
}

impl CrtArcadeLayout {
    pub fn for_layout(layout: UiLayoutGeometry, metrics: CrtUiMetrics, search: bool) -> Self {
        let content = layout.content_rect();
        let grid_x = metrics.grid_x.max(1) as usize;
        let grid_y = metrics.grid_y.max(1) as usize;
        let header_height = (metrics.header_height.max(1) as usize).min(content.height);
        let footer_height = (metrics.footer_height.max(1) as usize).min(content.height);

        if layout.is_portrait() {
            let header = CrtContentRect {
                x: content.x,
                y: content.y,
                width: content.width,
                height: header_height,
            };
            let footer = CrtContentRect {
                x: content.x,
                y: content.bottom().saturating_sub(footer_height),
                width: content.width,
                height: footer_height,
            };
            let title_clearance = (crt_arcade_row_height(metrics.game_row_height, true).max(1)
                as usize)
                .saturating_mul(CRT_PORTRAIT_TITLE_CLEARANCE_ROWS);
            let body_y = header
                .bottom()
                .saturating_add(grid_y)
                .saturating_add(title_clearance)
                .min(footer.y);
            let body_bottom = footer.y.saturating_sub(grid_y).max(body_y);
            let body_height = body_bottom.saturating_sub(body_y);
            let (list, search_keyboard) = if search {
                let keyboard_width = content.width * 2 / 5;
                let keyboard = CrtContentRect {
                    x: content.x,
                    y: body_y,
                    width: keyboard_width,
                    height: body_height,
                };
                let list_x = keyboard.right().saturating_add(grid_x).min(content.right());
                (
                    CrtContentRect {
                        x: list_x,
                        y: body_y,
                        width: content.right().saturating_sub(list_x),
                        height: body_height,
                    },
                    Some(keyboard),
                )
            } else {
                (
                    CrtContentRect {
                        x: content.x,
                        y: body_y,
                        width: content.width,
                        height: body_height,
                    },
                    None,
                )
            };
            return Self {
                header,
                list,
                footer,
                search_keyboard,
            };
        }

        let margin_x = grid_x * 2;
        let header = CrtContentRect {
            x: content.x.saturating_add(margin_x),
            y: content.y.saturating_add(grid_y * 2),
            width: content.width.saturating_sub(margin_x * 2),
            height: header_height,
        };
        let footer = CrtContentRect {
            x: header.x,
            y: content
                .bottom()
                .saturating_sub(footer_height.saturating_add(grid_y * 2)),
            width: header.width,
            height: footer_height,
        };
        let geometry = ArcadeListGeometry::crt_for_content(content, metrics, search);
        let list = CrtContentRect {
            x: geometry.x,
            y: geometry.y,
            width: geometry.width,
            height: geometry.visible_height_with_metrics(content.bottom(), Some(metrics)),
        };
        let search_keyboard = search.then(|| CrtContentRect {
            x: content.x.saturating_add(grid_x),
            y: content.y.saturating_add(header_height + grid_y * 3),
            width: content.width * 2 / 5,
            height: content
                .height
                .saturating_sub(header_height + footer_height + grid_y * 6),
        });
        Self {
            header,
            list,
            footer,
            search_keyboard,
        }
    }

    pub const fn list_geometry(self) -> ArcadeListGeometry {
        ArcadeListGeometry {
            x: self.list.x,
            y: self.list.y,
            width: self.list.width,
        }
    }
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
    favourite_launch_refs_revision: u64,
    surface: Vec<Rgb565Pixel>,
    surface_nonfill_runs: Vec<Vec<(usize, usize)>>,
    surface_selected_text_runs: Vec<Vec<(usize, usize)>>,
    band_scratch: Vec<Pixel>,
    selection_invert_scratch: Vec<Rgb565Pixel>,
    previous_selection_normal: Vec<Rgb565Pixel>,
    previous_selection_normal_rect: Option<DirtyRect>,
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
    crt_base_style: Option<ArcadeListStyle>,
    oriented_viewport_layout: Option<Rgb565OutputLayout>,
    oriented_viewport_rect: DirtyRect,
    persistent_oriented_layer: PersistentOrientedArcadeLayer,
    last_update_reason: ArcadeListUpdateReason,
    persistent_composition_trace: PersistentArcadeCompositionTrace,
}

/// Style identity carried by the persistent physical Arcade layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PersistentArcadeLayerStyle {
    Hdmi,
    Crt,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ArcadeListUpdateKind {
    #[default]
    None,
    Full,
    Scroll,
}

impl ArcadeListUpdateKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Full => "full",
            Self::Scroll => "scroll",
        }
    }

    fn from_update(update: &ArcadeListUpdate) -> Self {
        match update {
            ArcadeListUpdate::Full(_) => Self::Full,
            ArcadeListUpdate::Scroll { .. } => Self::Scroll,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ArcadeListUpdateReason {
    #[default]
    None,
    Forced,
    Initial,
    VisibleContentChanged,
    StationaryContentChanged,
    LargeDelta,
    ScrollDelta,
}

impl ArcadeListUpdateReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Forced => "forced",
            Self::Initial => "initial",
            Self::VisibleContentChanged => "visible-content-changed",
            Self::StationaryContentChanged => "stationary-content-changed",
            Self::LargeDelta => "large-delta",
            Self::ScrollDelta => "scroll-delta",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PersistentArcadeRebuildReason {
    #[default]
    None,
    Initial,
    Geometry,
    VisibleHeight,
    Output,
    Style,
    CatalogGeneration,
    BufferSize,
    Invalidated,
    RequestedFull,
    LayoutChanged,
    CrtStyle,
    MissingSelectionCapture,
    ZeroDelta,
    LargeDelta,
    SelectionRestoreFailed,
    ShiftFailed,
}

impl PersistentArcadeRebuildReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Initial => "initial",
            Self::Geometry => "geometry",
            Self::VisibleHeight => "visible-height",
            Self::Output => "output",
            Self::Style => "style",
            Self::CatalogGeneration => "catalog-generation",
            Self::BufferSize => "buffer-size",
            Self::Invalidated => "invalidated",
            Self::RequestedFull => "requested-full",
            Self::LayoutChanged => "layout-changed",
            Self::CrtStyle => "crt-style",
            Self::MissingSelectionCapture => "missing-selection-capture",
            Self::ZeroDelta => "zero-delta",
            Self::LargeDelta => "large-delta",
            Self::SelectionRestoreFailed => "selection-restore-failed",
            Self::ShiftFailed => "shift-failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PersistentArcadeCompositionTrace {
    pub requested_update: ArcadeListUpdateKind,
    pub requested_reason: ArcadeListUpdateReason,
    pub effective_update: ArcadeListUpdateKind,
    pub rebuild_reason: PersistentArcadeRebuildReason,
    pub elapsed_us: u64,
    pub written_pixels: u64,
    pub allocated_bytes: u64,
}

pub type PersistentArcadeCopyDecision = PhysicalLayerCopyDecision;
pub type PersistentArcadeCopyTrace = PhysicalLayerCopyTrace;

enum ArcadeOrientedTarget<'a> {
    Output {
        pixels: &'a mut [Rgb565Pixel],
        layout: Rgb565OutputLayout,
    },
    Region {
        backing: &'a mut PhysicalLayerBacking,
        layout: Rgb565RegionLayout,
    },
}

impl ArcadeOrientedTarget<'_> {
    fn output_layout(&self) -> Rgb565OutputLayout {
        match self {
            Self::Output { layout, .. } => *layout,
            Self::Region { layout, .. } => layout.output(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn copy_rect_strided(
        &mut self,
        destination_x: usize,
        destination_y: usize,
        width: usize,
        height: usize,
        source: &[Rgb565Pixel],
        source_stride: usize,
        source_x: usize,
        source_y: usize,
    ) -> bool {
        match self {
            Self::Output { pixels, layout } => Rgb565SurfaceMut::new(pixels, *layout)
                .expect("launcher output layout matches its cached target")
                .copy_rect_strided(
                    destination_x,
                    destination_y,
                    width,
                    height,
                    source,
                    source_stride,
                    source_x,
                    source_y,
                ),
            Self::Region { backing, layout } => {
                Rgb565RegionSurfaceMut::new(backing.pixels_mut(), *layout)
                    .expect("physical layer backing matches its region layout")
                    .copy_rect_strided(
                        destination_x,
                        destination_y,
                        width,
                        height,
                        source,
                        source_stride,
                        source_x,
                        source_y,
                    )
            }
        }
    }

    fn shift(
        &mut self,
        physical_rect: Rgb565Rect,
        dx: isize,
        dy: isize,
        fill: Rgb565Pixel,
    ) -> bool {
        match self {
            Self::Output { pixels, layout } => shift_physical_rect(
                pixels,
                layout.physical_stride(),
                layout.physical_height(),
                DirtyRect {
                    x0: physical_rect.x0,
                    y0: physical_rect.y0,
                    x1: physical_rect.x1,
                    y1: physical_rect.y1,
                },
                dx,
                dy,
                fill,
            ),
            Self::Region { backing, layout } => {
                physical_rect == layout.physical_rect() && backing.shift(dx, dy, fill)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArcadeListCompositionStats {
    pub composed: bool,
    pub restored_pixels: u32,
    pub foreground_pixels: u32,
    pub elapsed_us: u64,
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

pub use mister_magik_mister_runtime::framebuffer::latch_state::PhysicalLayerUpdate as ArcadeListUpdate;

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
        let crt_base_style = crt_metrics.map(|_| style);
        Self {
            title_font: ConsoleFont::new_with_typeface_and_row_filter(
                style.title_font_px,
                style.title_typeface,
                style.glyph_row_filter,
            ),
            meta_font: ConsoleFont::new_with_typeface_and_row_filter(
                style.meta_font_px,
                style.meta_typeface,
                style.glyph_row_filter,
            ),
            row_cache: HashMap::new(),
            favourite_launch_refs: HashSet::new(),
            favourite_launch_refs_revision: u64::MAX,
            surface: vec![style.background_565; ARCADE_LIST_W * ARCADE_LIST_H],
            surface_nonfill_runs: vec![Vec::new(); ARCADE_LIST_H],
            surface_selected_text_runs: vec![Vec::new(); ARCADE_LIST_H],
            band_scratch: Vec::new(),
            selection_invert_scratch: Vec::new(),
            previous_selection_normal: Vec::new(),
            previous_selection_normal_rect: None,
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
            crt_base_style,
            oriented_viewport_layout: None,
            oriented_viewport_rect: DirtyRect {
                x0: 0,
                y0: 0,
                x1: 0,
                y1: 0,
            },
            persistent_oriented_layer: PersistentOrientedArcadeLayer::new(),
            last_update_reason: ArcadeListUpdateReason::None,
            persistent_composition_trace: PersistentArcadeCompositionTrace::default(),
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
        let visible_height = geometry.visible_height_with_metrics(render_h, self.crt_metrics);
        self.set_geometry_for_visible_height(geometry, visible_height);
    }

    pub fn set_geometry_for_visible_height(
        &mut self,
        geometry: ArcadeListGeometry,
        visible_height: usize,
    ) {
        let visible_height = visible_height.min(ARCADE_LIST_H);
        if self.visible_height != visible_height {
            self.oriented_viewport_layout = None;
            self.persistent_oriented_layer.invalidate();
        }
        self.visible_height = visible_height;
        if self.geometry != geometry {
            self.oriented_viewport_layout = None;
            self.persistent_oriented_layer.invalidate();
            if self.width != geometry.width {
                self.width = geometry.width;
                self.surface = vec![self.style.background_565; self.width * ARCADE_LIST_H];
                self.surface_nonfill_runs = vec![Vec::new(); ARCADE_LIST_H];
                self.surface_selected_text_runs = vec![Vec::new(); ARCADE_LIST_H];
                self.row_cache.clear();
                self.row_fingerprint_cache.clear();
            }
            self.geometry = geometry;
            self.last_draw = None;
            self.last_filter_draw = None;
            self.surface_y = 0;
        }
    }

    pub fn set_crt_portrait_rows(&mut self, portrait: bool) {
        let Some(mut style) = self.crt_base_style else {
            return;
        };
        style.row_height = crt_arcade_row_height(style.row_height, portrait);
        if portrait {
            style.separator_top = 0;
            style.separator_bottom = 0;
        }
        if self.style.row_height == style.row_height
            && self.style.separator_top == style.separator_top
            && self.style.separator_bottom == style.separator_bottom
        {
            return;
        }
        self.style = style;
        self.row_cache.clear();
        self.row_fingerprint_cache.clear();
        self.invalidate_presented_layer();
    }

    pub fn invalidate_presented_layer(&mut self) {
        self.last_draw = None;
        self.last_filter_draw = None;
        self.surface_y = 0;
        self.oriented_viewport_layout = None;
        self.persistent_oriented_layer.invalidate();
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

    pub fn set_favourite_launch_refs_if_changed<'a>(
        &mut self,
        revision: u64,
        refs: impl IntoIterator<Item = &'a str>,
    ) {
        if self.favourite_launch_refs_revision == revision {
            return;
        }
        self.set_favourite_launch_refs(refs);
        self.favourite_launch_refs_revision = revision;
    }

    pub fn draw(
        &mut self,
        games: ArcadeGameView<'_>,
        _selected: usize,
        visual_index: f32,
        force: bool,
    ) -> Option<ArcadeListUpdate> {
        self.previous_selection_normal_rect = None;
        self.last_filter_draw = None;
        self.last_update_reason = ArcadeListUpdateReason::None;
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
        let resolved_missing_visible_hash = previous.as_ref().is_some_and(|previous| {
            previous.len == key.len
                && previous.visual_px == key.visual_px
                && previous.anchor_hash == key.anchor_hash
                && previous.visible_hash.is_none()
                && key.visible_hash.is_some()
        });
        if !force && resolved_missing_visible_hash {
            // Moving frames omit the expensive visible-window hash. The first
            // stationary frame merely resolves that missing observation; it
            // does not prove that list content changed.
            self.last_draw = Some(key);
            return None;
        }
        if force && self.last_draw.as_ref() == Some(&key) {
            self.last_update_reason = ArcadeListUpdateReason::Forced;
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
                && matches!(
                    (previous.visible_hash, key.visible_hash),
                    (Some(previous), Some(current)) if previous != current
                )
        });
        let can_reuse_scrolled_surface = same_len && !visible_content_changed_at_same_position;
        if !force
            && previous.is_some()
            && can_reuse_scrolled_surface
            && content_delta != 0
            && (content_delta.unsigned_abs() as usize) < self.visible_height
        {
            self.capture_previous_selection_normal();
        }
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
            self.last_update_reason = ArcadeListUpdateReason::Forced;
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        if previous.is_none() {
            self.last_update_reason = ArcadeListUpdateReason::Initial;
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        if !can_reuse_scrolled_surface {
            self.last_update_reason = ArcadeListUpdateReason::VisibleContentChanged;
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        if content_delta == 0 {
            self.last_update_reason = ArcadeListUpdateReason::StationaryContentChanged;
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        if content_delta.unsigned_abs() as usize >= self.visible_height {
            self.last_update_reason = ArcadeListUpdateReason::LargeDelta;
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        self.last_update_reason = ArcadeListUpdateReason::ScrollDelta;
        Some(ArcadeListUpdate::Scroll {
            delta_x: 0,
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
        self.previous_selection_normal_rect = None;
        self.last_draw = None;
        self.last_update_reason = ArcadeListUpdateReason::None;
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
            self.last_update_reason = ArcadeListUpdateReason::Forced;
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        let content_delta = previous
            .as_ref()
            .map(|previous| previous.visual_px - key.visual_px)
            .unwrap_or(0);
        let can_reuse_scrolled_surface = previous.as_ref().is_some_and(|previous| {
            previous.len == key.len && previous.content_hash == key.content_hash
        });
        if !force
            && previous.is_some()
            && can_reuse_scrolled_surface
            && content_delta != 0
            && (content_delta.unsigned_abs() as usize) < self.visible_height
        {
            self.capture_previous_selection_normal();
        }
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
        if force {
            self.last_update_reason = ArcadeListUpdateReason::Forced;
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        if previous.is_none() {
            self.last_update_reason = ArcadeListUpdateReason::Initial;
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        if !can_reuse_scrolled_surface {
            self.last_update_reason = ArcadeListUpdateReason::VisibleContentChanged;
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        if content_delta == 0 {
            self.last_update_reason = ArcadeListUpdateReason::StationaryContentChanged;
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        if content_delta.unsigned_abs() as usize >= self.visible_height {
            self.last_update_reason = ArcadeListUpdateReason::LargeDelta;
            return Some(ArcadeListUpdate::Full(self.dirty_rect()));
        }
        self.last_update_reason = ArcadeListUpdateReason::ScrollDelta;
        Some(ArcadeListUpdate::Scroll {
            delta_x: 0,
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
                self.rebuild_surface_nonfill_runs(dst_y);
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
        let mut changed_rows = Vec::with_capacity(copy_h);
        for row_y in 0..copy_h {
            let src = (src_y + row_y) * self.width;
            let viewport_y = band_y + dst_y + row_y;
            let dst_y = (self.surface_y + viewport_y) % self.visible_height;
            let dst = dst_y * self.width;
            self.surface[dst..dst + self.width].copy_from_slice(&row[src..src + self.width]);
            changed_rows.push(dst_y);
        }
        for row in changed_rows {
            self.rebuild_surface_nonfill_runs(row);
        }
    }

    fn fill_surface_band(&mut self, band_y: usize, band_h: usize, color: Rgb565Pixel) {
        for row in 0..band_h {
            let dst_y = (self.surface_y + band_y + row) % self.visible_height;
            let dst = dst_y * self.width;
            self.surface[dst..dst + self.width].fill(color);
            self.surface_nonfill_runs[dst_y].clear();
        }
    }

    fn rebuild_surface_nonfill_runs(&mut self, row: usize) {
        if row >= self.surface_nonfill_runs.len() {
            return;
        }
        let surface_row = &self.surface[row * self.width..(row + 1) * self.width];
        let mut runs = std::mem::take(&mut self.surface_nonfill_runs[row]);
        let mut selected_text_runs = std::mem::take(&mut self.surface_selected_text_runs[row]);
        runs.clear();
        selected_text_runs.clear();
        let mut x = 0;
        while x < self.width {
            while x < self.width
                && is_arcade_unselected_overlay_fill_pixel(surface_row[x], self.style)
            {
                x += 1;
            }
            let run_start = x;
            while x < self.width
                && !is_arcade_unselected_overlay_fill_pixel(surface_row[x], self.style)
            {
                x += 1;
            }
            if run_start < x {
                runs.push((run_start, x));
            }
        }
        let mut x = 0;
        while x < self.width {
            while x < self.width
                && is_arcade_row_background_pixel_with_style(surface_row[x], self.style)
            {
                x += 1;
            }
            let run_start = x;
            while x < self.width
                && !is_arcade_row_background_pixel_with_style(surface_row[x], self.style)
            {
                x += 1;
            }
            if run_start < x {
                selected_text_runs.push((run_start, x));
            }
        }
        self.surface_nonfill_runs[row] = runs;
        self.surface_selected_text_runs[row] = selected_text_runs;
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
            self.rebuild_surface_nonfill_runs(dst_y);
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
        let mut target = ArcadeOrientedTarget::Output {
            pixels: target.cached_565_mut(),
            layout: output_layout,
        };
        self.compose_layer_to_oriented_target(&mut target, redraw_selection_frame);
    }

    fn compose_layer_to_oriented_target(
        &mut self,
        target: &mut ArcadeOrientedTarget<'_>,
        redraw_selection_frame: bool,
    ) -> u64 {
        self.compose_viewport_band_to_oriented_target(target, 0, self.visible_height);
        if redraw_selection_frame {
            self.compose_selection_frame_to_oriented_target(target);
        }
        let output_layout = target.output_layout();
        self.oriented_viewport_layout = Some(output_layout);
        (self.width.saturating_mul(self.visible_height) as u64).saturating_add(
            redraw_selection_frame
                .then(|| self.selection_frame_write_pixels())
                .unwrap_or(0),
        )
    }

    pub fn compose_layer_update_to_oriented_cached(
        &mut self,
        target: &mut UiFrameTarget,
        output_layout: Rgb565OutputLayout,
        update: ArcadeListUpdate,
        redraw_selection_frame: bool,
    ) {
        let mut target = ArcadeOrientedTarget::Output {
            pixels: target.cached_565_mut(),
            layout: output_layout,
        };
        let _ = self.compose_layer_update_to_oriented_target(
            &mut target,
            update,
            redraw_selection_frame,
        );
    }

    fn compose_layer_update_to_oriented_target(
        &mut self,
        target: &mut ArcadeOrientedTarget<'_>,
        update: ArcadeListUpdate,
        redraw_selection_frame: bool,
    ) -> (ArcadeListUpdateKind, PersistentArcadeRebuildReason, u64) {
        let output_layout = target.output_layout();
        let ArcadeListUpdate::Scroll { delta_y, .. } = update else {
            let written = self.compose_layer_to_oriented_target(target, redraw_selection_frame);
            return (
                ArcadeListUpdateKind::Full,
                PersistentArcadeRebuildReason::RequestedFull,
                written,
            );
        };
        let (dx, dy) = output_layout.logical_delta_to_physical(0, delta_y);
        let Some(key) = self.oriented_viewport_layout else {
            let written = self.compose_layer_to_oriented_target(target, true);
            return (
                ArcadeListUpdateKind::Full,
                PersistentArcadeRebuildReason::LayoutChanged,
                written,
            );
        };
        let selection_requires_normalization = arcade_selection_inversion_enabled();
        let fallback_reason = if key != output_layout {
            Some(PersistentArcadeRebuildReason::LayoutChanged)
        } else if self.style.crt_palette {
            Some(PersistentArcadeRebuildReason::CrtStyle)
        } else if selection_requires_normalization && self.previous_selection_normal_rect.is_none()
        {
            Some(PersistentArcadeRebuildReason::MissingSelectionCapture)
        } else if delta_y == 0 {
            Some(PersistentArcadeRebuildReason::ZeroDelta)
        } else if delta_y.unsigned_abs() as usize >= self.visible_height {
            Some(PersistentArcadeRebuildReason::LargeDelta)
        } else {
            None
        };
        if let Some(reason) = fallback_reason {
            let written = self.compose_layer_to_oriented_target(target, true);
            return (ArcadeListUpdateKind::Full, reason, written);
        }
        let restored_pixels = self
            .previous_selection_normal_rect
            .map(|rect| rect.width().saturating_mul(rect.rows() as usize) as u64)
            .unwrap_or(0);
        if selection_requires_normalization
            && !self.restore_previous_selection_normal_to_oriented(target)
        {
            let written = self.compose_layer_to_oriented_target(target, true);
            return (
                ArcadeListUpdateKind::Full,
                PersistentArcadeRebuildReason::SelectionRestoreFailed,
                written,
            );
        }
        let logical = mister_magik_framebuffer_scenes::Rgb565Rect {
            x0: self.geometry.x,
            y0: self.geometry.y,
            x1: self.geometry.x + self.width,
            y1: self.geometry.y + self.visible_height,
        };
        let physical = output_layout.logical_rect_to_physical(logical);
        if !target.shift(physical, dx, dy, self.style.background_565) {
            let written = self.compose_layer_to_oriented_target(target, true);
            return (
                ArcadeListUpdateKind::Full,
                PersistentArcadeRebuildReason::ShiftFailed,
                written,
            );
        }
        let exposed = delta_y.unsigned_abs() as usize;
        let exposed_y = if delta_y < 0 {
            self.visible_height.saturating_sub(exposed)
        } else {
            0
        };
        self.compose_viewport_band_to_oriented_target(target, exposed_y, exposed);
        let selection_y = self.selection_y();
        self.compose_viewport_band_to_oriented_target(
            target,
            selection_y,
            self.style.row_height.max(1) as usize,
        );
        self.compose_selection_frame_to_oriented_target(target);
        self.previous_selection_normal_rect = None;
        self.oriented_viewport_layout = Some(output_layout);
        let shifted_pixels = physical
            .x1
            .saturating_sub(physical.x0)
            .saturating_mul(physical.y1.saturating_sub(physical.y0))
            as u64;
        let exposed_pixels = self.width.saturating_mul(exposed) as u64;
        let selection_pixels =
            self.width
                .saturating_mul(self.style.row_height.max(1) as usize) as u64;
        (
            ArcadeListUpdateKind::Scroll,
            PersistentArcadeRebuildReason::None,
            restored_pixels
                .saturating_add(shifted_pixels)
                .saturating_add(exposed_pixels)
                .saturating_add(selection_pixels)
                .saturating_add(self.selection_frame_write_pixels()),
        )
    }

    fn selection_frame_write_pixels(&self) -> u64 {
        let rect = self.selection_rect();
        let height = rect.y1.saturating_sub(rect.y0).min(ARCADE_LIST_H);
        self.width
            .saturating_mul(self.style.selection_frame_y)
            .saturating_mul(2)
            .saturating_add(
                self.style
                    .selection_frame_x
                    .saturating_mul(height)
                    .saturating_mul(2),
            ) as u64
    }

    pub fn compose_persistent_oriented_layer(
        &mut self,
        output_layout: Rgb565OutputLayout,
        update: ArcadeListUpdate,
        catalog_generation: u64,
    ) -> ArcadeListUpdate {
        let started = Instant::now();
        let requested_update = ArcadeListUpdateKind::from_update(&update);
        let requested_reason = self.last_update_reason;
        let mut layer = std::mem::take(&mut self.persistent_oriented_layer);
        let style = if self.style.crt_palette {
            PersistentArcadeLayerStyle::Crt
        } else {
            PersistentArcadeLayerStyle::Hdmi
        };
        let next_key = PersistentOrientedArcadeLayerKey {
            geometry: self.geometry,
            visible_height: self.visible_height,
            output: output_layout,
            style,
            catalog_generation,
            ring_origin: self.surface_y,
        };
        let expected_region = Rgb565RegionLayout::new(
            output_layout,
            Rgb565Rect {
                x0: self.geometry.x,
                y0: self.geometry.y,
                x1: self.geometry.x + self.width,
                y1: self.geometry.y + self.visible_height,
            },
        )
        .expect("Arcade layer geometry is within the launcher output");
        let ensure_reason = match layer.key() {
            None => PersistentArcadeRebuildReason::Initial,
            Some(current) if current.geometry != next_key.geometry => {
                PersistentArcadeRebuildReason::Geometry
            }
            Some(current) if current.visible_height != next_key.visible_height => {
                PersistentArcadeRebuildReason::VisibleHeight
            }
            Some(current) if current.output != next_key.output => {
                PersistentArcadeRebuildReason::Output
            }
            Some(current) if current.style != next_key.style => {
                PersistentArcadeRebuildReason::Style
            }
            Some(current) if current.catalog_generation != next_key.catalog_generation => {
                PersistentArcadeRebuildReason::CatalogGeneration
            }
            Some(_) if layer.content().len() != expected_region.len() => {
                PersistentArcadeRebuildReason::BufferSize
            }
            Some(_) if layer.needs_full_rebuild() => PersistentArcadeRebuildReason::Invalidated,
            Some(_) => PersistentArcadeRebuildReason::None,
        };
        let changed = layer.ensure(
            self.geometry,
            self.visible_height,
            output_layout,
            style,
            catalog_generation,
            self.surface_y,
        );
        let effective_update = if changed || layer.needs_full_rebuild() {
            ArcadeListUpdate::Full(self.dirty_rect())
        } else {
            update
        };
        let region_layout = layer
            .region_layout()
            .expect("ensured physical Arcade layer has a region layout");
        let mut target = ArcadeOrientedTarget::Region {
            backing: layer.backing_mut(),
            layout: region_layout,
        };
        let (effective_composition, composition_reason, written_pixels) = self
            .compose_layer_update_to_oriented_target(
                &mut target,
                effective_update,
                matches!(effective_update, ArcadeListUpdate::Full(_)),
            );
        layer.set_selection_aperture(self.selection_y(), self.style.row_height.max(1) as usize);
        layer.mark_full_rebuild_complete();
        let allocated_bytes = layer.allocated_bytes() as u64;
        self.persistent_oriented_layer = layer;
        self.persistent_composition_trace = PersistentArcadeCompositionTrace {
            requested_update,
            requested_reason,
            effective_update: effective_composition,
            rebuild_reason: if ensure_reason != PersistentArcadeRebuildReason::None {
                ensure_reason
            } else {
                composition_reason
            },
            elapsed_us: started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
            written_pixels,
            allocated_bytes,
        };
        effective_update
    }

    pub fn persistent_composition_trace(&self) -> PersistentArcadeCompositionTrace {
        self.persistent_composition_trace
    }

    pub fn persistent_oriented_layer_view(&self) -> Option<PhysicalLayerView<'_>> {
        self.persistent_oriented_layer.view()
    }

    pub fn persistent_oriented_layer_diagnostic(&self) -> PersistentArcadeLayerDiagnostic {
        self.persistent_oriented_layer.diagnostic()
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
    ) -> ArcadeListCompositionStats {
        self.compose_layer_over_backdrop_to_oriented_cached_with_state(
            target,
            backdrop,
            output_layout,
            redraw_selection_frame,
            false,
        )
    }

    pub fn compose_layer_over_backdrop_to_oriented_cached_with_state(
        &mut self,
        target: &mut UiFrameTarget,
        backdrop: &[Rgb565Pixel],
        output_layout: Rgb565OutputLayout,
        redraw_selection_frame: bool,
        backdrop_is_fresh: bool,
    ) -> ArcadeListCompositionStats {
        let started = Instant::now();
        if backdrop.len() < output_layout.len()
            || target.cached_565().len() < output_layout.len()
            || self.geometry.x.saturating_add(self.width) > output_layout.logical_width()
            || self.geometry.y.saturating_add(self.visible_height) > output_layout.logical_height()
        {
            return ArcadeListCompositionStats::default();
        }

        let selection_y = self.selection_y();
        let selection_bottom = selection_y + self.style.row_height.max(1) as usize;
        let cached = target.cached_565_mut();
        let identity_layout = matches!(output_layout.rotation(), OutputRotation::None)
            && output_layout.physical_stride() == output_layout.logical_width();
        if identity_layout {
            // CRT240p is an upright contiguous surface. Restore every
            // unselected row from the stationary backdrop before drawing the
            // current runs so glyphs from overwritten ring-buffer rows cannot
            // survive a scroll step.
            if !backdrop_is_fresh {
                for viewport_y in 0..self.visible_height {
                    let destination_start = (self.geometry.y + viewport_y)
                        * output_layout.physical_stride()
                        + self.geometry.x;
                    let destination =
                        &mut cached[destination_start..destination_start + self.width];
                    destination.copy_from_slice(
                        &backdrop[destination_start..destination_start + self.width],
                    );
                }
            }
            for viewport_y in 0..self.visible_height {
                let source_y = (self.surface_y + viewport_y) % self.visible_height;
                let source_start = source_y * self.width;
                let destination_start = (self.geometry.y + viewport_y)
                    * output_layout.physical_stride()
                    + self.geometry.x;
                let destination = &mut cached[destination_start..destination_start + self.width];
                let selected = viewport_y >= selection_y && viewport_y < selection_bottom;
                let surface_row = &self.surface[source_start..source_start + self.width];
                if selected && self.style.crt_palette {
                    destination.fill(self.style.selection_fill_565);
                    for &(run_start, run_end) in &self.surface_selected_text_runs[source_y] {
                        destination[run_start..run_end].fill(self.style.selection_text_565);
                    }
                } else if selected {
                    for x in 0..self.width {
                        destination[x] =
                            selected_aperture_pixel_with_style(surface_row[x], self.style);
                    }
                } else {
                    for &(run_start, run_end) in &self.surface_nonfill_runs[source_y] {
                        destination[run_start..run_end]
                            .copy_from_slice(&surface_row[run_start..run_end]);
                    }
                }
            }
            if redraw_selection_frame {
                self.compose_selection_frame_to_oriented_cached(cached, output_layout);
            }
            return ArcadeListCompositionStats {
                composed: true,
                restored_pixels: if backdrop_is_fresh {
                    0
                } else {
                    self.width
                        .saturating_mul(self.visible_height)
                        .min(u32::MAX as usize) as u32
                },
                foreground_pixels: self
                    .surface_nonfill_runs
                    .iter()
                    .flat_map(|runs| runs.iter())
                    .map(|(start, end)| end.saturating_sub(*start))
                    .sum::<usize>()
                    .min(u32::MAX as usize) as u32,
                elapsed_us: started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
            };
        }
        if self.style.crt_palette {
            let stats = self.compose_rotated_crt_layer_over_backdrop(
                cached,
                backdrop,
                output_layout,
                backdrop_is_fresh,
            );
            if redraw_selection_frame {
                self.compose_selection_frame_to_oriented_cached(cached, output_layout);
            }
            return ArcadeListCompositionStats {
                elapsed_us: started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
                ..stats
            };
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
                } else if backdrop_is_fresh
                    && is_arcade_unselected_fill_pixel_with_style(pixel, self.style)
                {
                    cached[offset]
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
        ArcadeListCompositionStats {
            composed: true,
            restored_pixels: if backdrop_is_fresh {
                0
            } else {
                self.width
                    .saturating_mul(self.visible_height)
                    .min(u32::MAX as usize) as u32
            },
            foreground_pixels: self
                .width
                .saturating_mul(self.visible_height)
                .min(u32::MAX as usize) as u32,
            elapsed_us: started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
        }
    }

    fn compose_rotated_crt_layer_over_backdrop(
        &mut self,
        cached: &mut [Rgb565Pixel],
        backdrop: &[Rgb565Pixel],
        output_layout: Rgb565OutputLayout,
        backdrop_is_fresh: bool,
    ) -> ArcadeListCompositionStats {
        if self.oriented_viewport_layout != Some(output_layout) {
            let physical = output_layout.logical_rect_to_physical(
                mister_magik_framebuffer_scenes::Rgb565Rect {
                    x0: self.geometry.x,
                    y0: self.geometry.y,
                    x1: self.geometry.x + self.width,
                    y1: self.geometry.y + self.visible_height,
                },
            );
            self.oriented_viewport_rect = DirtyRect {
                x0: physical.x0,
                y0: physical.y0,
                x1: physical.x1,
                y1: physical.y1,
            };
            self.oriented_viewport_layout = Some(output_layout);
        }
        let physical = self.oriented_viewport_rect;
        let mut restored_pixels = 0_usize;
        if !backdrop_is_fresh {
            for physical_y in physical.y0..physical.y1 {
                let start = physical_y * output_layout.physical_stride() + physical.x0;
                let end = start + physical.width();
                cached[start..end].copy_from_slice(&backdrop[start..end]);
            }
            restored_pixels = physical.width().saturating_mul(physical.rows() as usize);
        }

        let selection_y = self.selection_y().min(self.visible_height);
        let selection_bottom = selection_y
            .saturating_add(self.style.row_height.max(1) as usize)
            .min(self.visible_height);
        let selection =
            output_layout.logical_rect_to_physical(mister_magik_framebuffer_scenes::Rgb565Rect {
                x0: self.geometry.x,
                y0: self.geometry.y + selection_y,
                x1: self.geometry.x + self.width,
                y1: self.geometry.y + selection_bottom,
            });
        let mut foreground_pixels = 0_usize;
        for physical_y in selection.y0..selection.y1 {
            let start = physical_y * output_layout.physical_stride() + selection.x0;
            let end = start + selection.width();
            cached[start..end].fill(self.style.selection_fill_565);
            foreground_pixels = foreground_pixels.saturating_add(selection.width());
        }

        for viewport_y in 0..self.visible_height {
            let source_y = (self.surface_y + viewport_y) % self.visible_height;
            let selected = viewport_y >= selection_y && viewport_y < selection_bottom;
            let runs = if selected {
                &self.surface_selected_text_runs[source_y]
            } else {
                &self.surface_nonfill_runs[source_y]
            };
            for &(run_start, run_end) in runs {
                for local_x in run_start..run_end {
                    let logical_x = self.geometry.x + local_x;
                    let logical_y = self.geometry.y + viewport_y;
                    let (physical_x, physical_y) = match output_layout.rotation() {
                        OutputRotation::Clockwise90 => {
                            (output_layout.logical_height() - 1 - logical_y, logical_x)
                        }
                        OutputRotation::CounterClockwise90 => {
                            (logical_y, output_layout.logical_width() - 1 - logical_x)
                        }
                        OutputRotation::None => unreachable!("rotated CRT compositor"),
                    };
                    cached[physical_y * output_layout.physical_stride() + physical_x] = if selected
                    {
                        self.style.selection_text_565
                    } else {
                        self.surface[source_y * self.width + local_x]
                    };
                    foreground_pixels = foreground_pixels.saturating_add(1);
                }
            }
        }
        ArcadeListCompositionStats {
            composed: true,
            restored_pixels: restored_pixels.min(u32::MAX as usize) as u32,
            foreground_pixels: foreground_pixels.min(u32::MAX as usize) as u32,
            elapsed_us: 0,
        }
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
        let source_start = src_y * self.width + x;
        let source_end = source_start.saturating_add(w.saturating_mul(h));
        if x == 0 && w == self.width && source_end <= self.surface.len() {
            prepare_selected_aperture_pixels(
                &mut self.selection_invert_scratch,
                &self.surface[source_start..source_end],
                self.style,
            );
        } else {
            for row in 0..h {
                let src = (src_y + row) * self.width + x;
                let dst = row * w;
                prepare_selected_aperture_pixels(
                    &mut self.selection_invert_scratch[dst..dst + w],
                    &self.surface[src..src + w],
                    self.style,
                );
            }
        }
        &self.selection_invert_scratch
    }

    fn capture_previous_selection_normal(&mut self) {
        let rect = self.selection_rect();
        let viewport_y = rect.y0.saturating_sub(self.geometry.y);
        let width = rect.width().min(self.width);
        let height = (rect.rows() as usize).min(self.visible_height.saturating_sub(viewport_y));
        if width == 0 || height == 0 {
            self.previous_selection_normal_rect = None;
            return;
        }
        let mut normal = std::mem::take(&mut self.previous_selection_normal);
        normal.resize(width.saturating_mul(height), Rgb565Pixel(0));
        for row in 0..height {
            let source_y = (self.surface_y + viewport_y + row) % self.visible_height;
            let source = source_y * self.width;
            let destination = row * width;
            normal[destination..destination + width]
                .copy_from_slice(&self.surface[source..source + width]);
        }
        self.previous_selection_normal = normal;
        self.previous_selection_normal_rect = Some(DirtyRect {
            x0: rect.x0,
            y0: rect.y0,
            x1: rect.x0 + width,
            y1: rect.y0 + height,
        });
    }

    fn restore_previous_selection_normal_to_oriented(
        &mut self,
        target: &mut ArcadeOrientedTarget<'_>,
    ) -> bool {
        let Some(rect) = self.previous_selection_normal_rect else {
            return false;
        };
        let width = rect.width();
        let height = rect.rows() as usize;
        if self.previous_selection_normal.len() != width.saturating_mul(height) {
            return false;
        }
        target.copy_rect_strided(
            rect.x0,
            rect.y0,
            width,
            height,
            &self.previous_selection_normal,
            width,
            0,
            0,
        )
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

    fn compose_viewport_band_to_oriented_target(
        &mut self,
        target: &mut ArcadeOrientedTarget<'_>,
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
                    self.compose_surface_rect_to_oriented_target(target, x, y, w, h)
                }
                ArcadeListPresentKind::Inverted => {
                    if self.style.crt_palette || arcade_selection_inversion_enabled() {
                        self.compose_inverted_surface_rect_to_oriented_target(target, x, y, w, h);
                    } else {
                        self.compose_surface_rect_to_oriented_target(target, x, y, w, h);
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
    fn compose_surface_rect_to_oriented_target(
        &mut self,
        target: &mut ArcadeOrientedTarget<'_>,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % self.visible_height;
            let copy_h = (h - copied).min(self.visible_height - src_y);
            let copied_rect = target.copy_rect_strided(
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
    fn compose_inverted_surface_rect_to_oriented_target(
        &mut self,
        target: &mut ArcadeOrientedTarget<'_>,
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
            let copied_rect =
                target.copy_rect_strided(target_x, target_y, w, copy_h, inverted, w, 0, 0);
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
        pixels: &mut [Rgb565Pixel],
        output_layout: Rgb565OutputLayout,
    ) {
        let mut target = ArcadeOrientedTarget::Output {
            pixels,
            layout: output_layout,
        };
        self.compose_selection_frame_to_oriented_target(&mut target);
    }

    fn compose_selection_frame_to_oriented_target(
        &mut self,
        target: &mut ArcadeOrientedTarget<'_>,
    ) {
        let rect = self.selection_rect();
        let color = self.style.selection_frame_565;
        let thickness_x = self.style.selection_frame_x;
        let thickness_y = self.style.selection_frame_y;
        let h = rect.y1.saturating_sub(rect.y0).min(ARCADE_LIST_H);
        self.selection_horizontal
            .resize(self.width * thickness_y, color);
        self.selection_horizontal.fill(color);
        let _ = target.copy_rect_strided(
            rect.x0,
            rect.y0,
            self.width,
            thickness_y,
            &self.selection_horizontal,
            self.width,
            0,
            0,
        );
        let _ = target.copy_rect_strided(
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
        let _ = target.copy_rect_strided(
            rect.x0,
            rect.y0,
            thickness_x,
            h,
            &self.selection_vertical,
            thickness_x,
            0,
            0,
        );
        let _ = target.copy_rect_strided(
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

fn prepare_selected_aperture_pixels(
    destination: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    style: ArcadeListStyle,
) {
    let count = destination.len().min(source.len());
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    if count > 0 && arcade_selection_neon_enabled() {
        unsafe extern "C" {
            fn mister_magik_arcade_selection_rgb565(
                destination: *mut u16,
                source: *const u16,
                count: usize,
                background: u16,
                alternate_background: u16,
                border: u16,
                badge_fill: u16,
                selection_fill: u16,
                selection_foreground: u16,
                fixed_foreground: u8,
            );
        }
        // SAFETY: both slices contain at least `count` aligned RGB565 pixels.
        unsafe {
            mister_magik_arcade_selection_rgb565(
                destination.as_mut_ptr().cast(),
                source.as_ptr().cast(),
                count,
                style.background_565.0,
                style.alternate_background_565.0,
                style.border_565.0,
                style.badge_fill_565.0,
                style.selection_fill_565.0,
                style.selection_text_565.0,
                u8::from(style.crt_palette),
            );
        }
        return;
    }
    for (destination, source) in destination[..count].iter_mut().zip(&source[..count]) {
        *destination = selected_aperture_pixel_with_style(*source, style);
    }
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn arcade_selection_neon_enabled() -> bool {
    use std::sync::OnceLock;

    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        !std::env::var("MISTER_ARCADE_SELECTION_SCALAR").is_ok_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
    })
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

fn is_arcade_unselected_overlay_fill_pixel(pixel: Rgb565Pixel, style: ArcadeListStyle) -> bool {
    is_arcade_unselected_fill_pixel_with_style(pixel, style)
        || (style.crt_palette && pixel == style.border_565)
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

    fn native_crt_240_display() -> UiDisplay {
        let plan = crate::ui_display::UiDisplayPlan::from_geometry_with_route_and_composition(
            crate::ui_display::ResolvedOutputRoute::Crt240p60
                .progressive_geometry()
                .expect("CRT240 display geometry"),
            crate::ui_display::ResolvedOutputRoute::Crt240p60,
            "test-native-crt240",
            crate::ui_display::UiFramebufferSizePolicy::Auto,
            crate::ui_display::Crt240Composition::Native240,
        );
        UiDisplay::for_plan(plan)
    }

    fn native_crt_288_display() -> UiDisplay {
        let plan = crate::ui_display::UiDisplayPlan::from_geometry_with_route_and_composition(
            crate::ui_display::ResolvedOutputRoute::Crt288p50
                .progressive_geometry()
                .expect("CRT288 display geometry"),
            crate::ui_display::ResolvedOutputRoute::Crt288p50,
            "test-native-crt288",
            crate::ui_display::UiFramebufferSizePolicy::Auto,
            crate::ui_display::Crt240Composition::Native240,
        );
        UiDisplay::for_plan(plan)
    }

    #[test]
    fn native_240_portrait_arcade_uses_route_owned_safe_area() {
        let display = native_crt_240_display();
        let metrics = CrtUiMetrics::for_display(&display);
        for orientation in [
            crate::ui_display::ScreenOrientation::MonitorClockwise,
            crate::ui_display::ScreenOrientation::MonitorCounterclockwise,
        ] {
            let ui_layout = UiLayoutGeometry::for_display(&display, orientation);
            assert_eq!(
                ui_layout.content_rect(),
                CrtContentRect {
                    x: 12,
                    y: 32,
                    width: 216,
                    height: 576,
                }
            );
            let normal = CrtArcadeLayout::for_layout(ui_layout, metrics, false);
            assert_eq!(
                normal.header,
                CrtContentRect {
                    x: 12,
                    y: 32,
                    width: 216,
                    height: 48
                }
            );
            assert_eq!(
                normal.list,
                CrtContentRect {
                    x: 12,
                    y: 180,
                    width: 216,
                    height: 404
                }
            );
            assert_eq!(
                normal.footer,
                CrtContentRect {
                    x: 12,
                    y: 588,
                    width: 216,
                    height: 20
                }
            );
            assert_eq!(normal.search_keyboard, None);

            let search = CrtArcadeLayout::for_layout(ui_layout, metrics, true);
            assert_eq!(
                search.search_keyboard,
                Some(CrtContentRect {
                    x: 12,
                    y: 180,
                    width: 86,
                    height: 404,
                })
            );
            assert_eq!(
                search.list,
                CrtContentRect {
                    x: 106,
                    y: 180,
                    width: 122,
                    height: 404
                }
            );
        }
    }

    #[test]
    fn native_288_portrait_arcade_preserves_asymmetric_pal_safe_area() {
        let display = native_crt_288_display();
        let metrics = CrtUiMetrics::for_display(&display);
        for (orientation, expected_x) in [
            (crate::ui_display::ScreenOrientation::MonitorClockwise, 15),
            (
                crate::ui_display::ScreenOrientation::MonitorCounterclockwise,
                20,
            ),
        ] {
            let ui_layout = UiLayoutGeometry::for_display(&display, orientation);
            assert_eq!(
                ui_layout.content_rect(),
                CrtContentRect {
                    x: expected_x,
                    y: 32,
                    width: 253,
                    height: 576,
                }
            );
            let normal = CrtArcadeLayout::for_layout(ui_layout, metrics, false);
            assert_eq!(
                normal.header,
                CrtContentRect {
                    x: expected_x,
                    y: 32,
                    width: 253,
                    height: 56,
                }
            );
            assert_eq!(
                normal.list,
                CrtContentRect {
                    x: expected_x,
                    y: 207,
                    width: 253,
                    height: 372,
                }
            );
            assert_eq!(
                normal.footer,
                CrtContentRect {
                    x: expected_x,
                    y: 584,
                    width: 253,
                    height: 24,
                }
            );

            let search = CrtArcadeLayout::for_layout(ui_layout, metrics, true);
            assert_eq!(
                search.search_keyboard,
                Some(CrtContentRect {
                    x: expected_x,
                    y: 207,
                    width: 101,
                    height: 372,
                })
            );
            assert_eq!(
                search.list,
                CrtContentRect {
                    x: expected_x + 109,
                    y: 207,
                    width: 144,
                    height: 372,
                }
            );
        }
    }

    #[test]
    fn landscape_arcade_layout_preserves_existing_geometry() {
        let display = native_crt_240_display();
        let metrics = CrtUiMetrics::for_display(&display);
        let ui_layout =
            UiLayoutGeometry::for_display(&display, crate::ui_display::ScreenOrientation::Normal);
        let content = ui_layout.content_rect();
        for search in [false, true] {
            let layout = CrtArcadeLayout::for_layout(ui_layout, metrics, search);
            let legacy = ArcadeListGeometry::crt_for_content(content, metrics, search);
            assert_eq!(layout.list_geometry(), legacy);
            assert_eq!(
                layout.list.height,
                legacy.visible_height_with_metrics(content.bottom(), Some(metrics))
            );
            assert_eq!(
                layout.header,
                CrtContentRect {
                    x: 48,
                    y: 20,
                    width: 544,
                    height: 48
                }
            );
            assert_eq!(
                layout.footer,
                CrtContentRect {
                    x: 48,
                    y: 200,
                    width: 544,
                    height: 20
                }
            );
        }
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
    fn oriented_scroll_shift_uses_the_physical_axis_for_each_quarter_turn() {
        let output = Rgb565OutputLayout::new(4, 3, 4, OutputRotation::Clockwise90).unwrap();
        let rect = output.logical_rect_to_physical(mister_magik_framebuffer_scenes::Rgb565Rect {
            x0: 0,
            y0: 0,
            x1: 4,
            y1: 3,
        });
        let mut pixels = (0..output.len())
            .map(|value| Rgb565Pixel(value as u16))
            .collect::<Vec<_>>();
        let before = pixels.clone();
        assert!(shift_physical_rect(
            &mut pixels,
            output.physical_stride(),
            output.physical_height(),
            DirtyRect {
                x0: rect.x0,
                y0: rect.y0,
                x1: rect.x1,
                y1: rect.y1,
            },
            -1,
            0,
            Rgb565Pixel(0xffff),
        ));
        assert_eq!(
            pixels[rect.y0 * output.physical_stride() + rect.x0],
            before[rect.y0 * output.physical_stride() + rect.x0 + 1]
        );
        assert_eq!(
            pixels[rect.y0 * output.physical_stride() + rect.x1 - 1],
            Rgb565Pixel(0xffff)
        );
    }

    #[test]
    fn portrait_scroll_update_matches_a_full_rebuild() {
        let games = games("arcade", 48);
        let geometry = ArcadeListGeometry::portrait(540, 960, false);
        let output =
            Rgb565OutputLayout::new(540, 960, 960, OutputRotation::CounterClockwise90).unwrap();
        let mut incremental = ArcadeListRenderer::new();
        incremental.set_geometry_for_visible_height(geometry, 504);
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(
            output.physical_stride(),
            output.physical_height(),
        ));
        let first = incremental
            .draw(ArcadeGameView::contiguous(&games), 0, 0.0, false)
            .expect("initial Arcade update");
        incremental.compose_layer_update_to_oriented_cached(&mut target, output, first, true);
        let scroll = incremental
            .draw(ArcadeGameView::contiguous(&games), 1, 1.0, false)
            .expect("scroll Arcade update");
        assert!(matches!(scroll, ArcadeListUpdate::Scroll { .. }));
        incremental.compose_layer_update_to_oriented_cached(&mut target, output, scroll, false);

        let mut full = ArcadeListRenderer::new();
        full.set_geometry_for_visible_height(geometry, 504);
        full.draw(ArcadeGameView::contiguous(&games), 1, 1.0, true);
        let mut expected = UiFrameTarget::cached(FramebufferTargetGeometry::new(
            output.physical_stride(),
            output.physical_height(),
        ));
        full.compose_layer_to_oriented_cached(&mut expected, output, true);
        assert_eq!(target.cached_565(), expected.cached_565());
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
        let sentinel = Rgb565Pixel(0x1234);
        let backdrop = vec![Rgb565Pixel(0x4321); layout.len()];
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(640, 480));
        target.cached_565_mut().fill(sentinel);
        assert!(
            renderer
                .compose_layer_over_backdrop_to_oriented_cached(
                    &mut target,
                    &backdrop,
                    layout,
                    true,
                )
                .composed
        );

        renderer.draw(
            ArcadeGameView::contiguous(&games),
            1,
            metrics.game_row_height as f32,
            false,
        );
        target.cached_565_mut().fill(sentinel);
        assert!(
            renderer
                .compose_layer_over_backdrop_to_oriented_cached(
                    &mut target,
                    &backdrop,
                    layout,
                    true,
                )
                .composed
        );

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

    fn scalar_crt_backdrop_composition(
        renderer: &ArcadeListRenderer,
        destination: &mut [Rgb565Pixel],
        backdrop: &[Rgb565Pixel],
        output: Rgb565OutputLayout,
        backdrop_is_fresh: bool,
    ) {
        let selection_y = renderer.selection_y();
        let selection_bottom = selection_y + renderer.style.row_height.max(1) as usize;
        for viewport_y in 0..renderer.visible_height {
            let source_y = (renderer.surface_y + viewport_y) % renderer.visible_height;
            let source_row = source_y * renderer.width;
            let selected = viewport_y >= selection_y && viewport_y < selection_bottom;
            for local_x in 0..renderer.width {
                let offset = output.physical_offset(
                    renderer.geometry.x + local_x,
                    renderer.geometry.y + viewport_y,
                );
                let pixel = renderer.surface[source_row + local_x];
                destination[offset] = if selected {
                    selected_aperture_pixel_with_style(pixel, renderer.style)
                } else if is_arcade_unselected_overlay_fill_pixel(pixel, renderer.style) {
                    if backdrop_is_fresh {
                        destination[offset]
                    } else {
                        backdrop[offset]
                    }
                } else {
                    pixel
                };
            }
        }
    }

    fn crt_overlay_selection(renderer: &ArcadeListRenderer) -> Rgb565Rect {
        let selection_y = renderer.selection_y();
        Rgb565Rect {
            x0: renderer.geometry.x,
            y0: renderer.geometry.y + selection_y,
            x1: renderer.geometry.x + renderer.width,
            y1: renderer.geometry.y + selection_y + renderer.style.row_height.max(1) as usize,
        }
    }

    fn crt_overlay_foreground_spans(renderer: &ArcadeListRenderer) -> Vec<Rgb565Rect> {
        let selection_y = renderer.selection_y();
        let selection_bottom = selection_y + renderer.style.row_height.max(1) as usize;
        let mut spans = Vec::new();
        for viewport_y in 0..renderer.visible_height {
            if viewport_y >= selection_y && viewport_y < selection_bottom {
                continue;
            }
            let source_y = (renderer.surface_y + viewport_y) % renderer.visible_height;
            for &(x0, x1) in &renderer.surface_nonfill_runs[source_y] {
                spans.push(Rgb565Rect {
                    x0: renderer.geometry.x + x0,
                    y0: renderer.geometry.y + viewport_y,
                    x1: renderer.geometry.x + x1,
                    y1: renderer.geometry.y + viewport_y + 1,
                });
            }
        }
        spans
    }

    fn restore_crt_overlay_rects(
        destination: &mut [Rgb565Pixel],
        backdrop: &[Rgb565Pixel],
        output: Rgb565OutputLayout,
        rects: impl IntoIterator<Item = Rgb565Rect>,
    ) {
        for rect in rects {
            for y in rect.y0..rect.y1 {
                for x in rect.x0..rect.x1 {
                    let offset = output.physical_offset(x, y);
                    destination[offset] = backdrop[offset];
                }
            }
        }
    }

    fn crt_overlay_key(
        renderer: &ArcadeListRenderer,
        output: Rgb565OutputLayout,
        style_revision: u64,
    ) -> crate::crt_arcade_overlay::CrtArcadeOverlayKey {
        crate::crt_arcade_overlay::CrtArcadeOverlayKey {
            backdrop_revision: 7,
            layout: output,
            viewport: Rgb565Rect {
                x0: renderer.geometry.x,
                y0: renderer.geometry.y,
                x1: renderer.geometry.x + renderer.width,
                y1: renderer.geometry.y + renderer.visible_height,
            },
            style_revision,
            catalog_generation: 11,
            ring_origin: renderer.surface_y,
            selection: crt_overlay_selection(renderer),
        }
    }

    #[test]
    fn retained_crt_overlay_plan_matches_full_compositor_across_routes_and_wraps() {
        let mut games = games("arcade", 48);
        games[1].is_new = true;
        games[14].is_new = true;
        let favourite = games[2].mra_path.to_string();
        for display in [native_crt_240_display(), native_crt_288_display()] {
            let metrics = CrtUiMetrics::for_display(&display);
            for orientation in [
                crate::ui_display::ScreenOrientation::MonitorClockwise,
                crate::ui_display::ScreenOrientation::MonitorCounterclockwise,
            ] {
                let layout = UiLayoutGeometry::for_display(&display, orientation);
                let arcade = CrtArcadeLayout::for_layout(layout, metrics, false);
                let output = layout.output_layout();
                let backdrop = (0..output.len())
                    .map(|index| Rgb565Pixel((index as u16).rotate_left(5) ^ 0x6b4d))
                    .collect::<Vec<_>>();
                let mut renderer = ArcadeListRenderer::new_for_crt_display(metrics, &display);
                renderer.set_crt_portrait_rows(true);
                renderer
                    .set_geometry_for_visible_height(arcade.list_geometry(), arcade.list.height);
                renderer.set_favourite_launch_refs([favourite.as_str()]);
                let row_height = renderer.style.row_height.max(1) as usize;
                renderer.draw(ArcadeGameView::contiguous(&games), 8, 8.0, true);
                renderer.surface_y = renderer.visible_height - row_height;

                let mut incremental = UiFrameTarget::cached(FramebufferTargetGeometry::new(
                    output.physical_width(),
                    output.physical_height(),
                ));
                incremental.cached_565_mut().copy_from_slice(&backdrop);
                assert!(
                    renderer
                        .compose_layer_over_backdrop_to_oriented_cached_with_state(
                            &mut incremental,
                            &backdrop,
                            output,
                            false,
                            true,
                        )
                        .composed
                );

                let style_revision =
                    u64::from(metrics.game_row_height as u16) | ((display.output_h() as u64) << 16);
                let first_key = crt_overlay_key(&renderer, output, style_revision);
                let first_spans = crt_overlay_foreground_spans(&renderer);
                let mut retained = crate::crt_arcade_overlay::CrtArcadeOverlayState::new();
                retained.commit(first_key, &first_spans);

                let update = renderer
                    .draw(ArcadeGameView::contiguous(&games), 9, 9.0, false)
                    .expect("one-row CRT scroll");
                let ArcadeListUpdate::Scroll { delta_y, .. } = update else {
                    panic!("expected a retained CRT scroll update");
                };
                assert_eq!(renderer.surface_y, 0, "ring origin did not wrap");
                let next_key = crt_overlay_key(&renderer, output, style_revision);
                let plan = retained.plan(
                    next_key,
                    crate::crt_arcade_overlay::CrtArcadeOverlayUpdate::Scroll { delta_y },
                );
                assert!(!plan.full_rebuild);

                let mut restore = plan.stale_glyph_spans.clone();
                restore.extend_from_slice(&plan.exposed_stripes);
                restore.extend(plan.selection_union);
                restore_crt_overlay_rects(incremental.cached_565_mut(), &backdrop, output, restore);
                assert!(
                    renderer
                        .compose_layer_over_backdrop_to_oriented_cached_with_state(
                            &mut incremental,
                            &backdrop,
                            output,
                            false,
                            true,
                        )
                        .composed
                );

                let mut full = UiFrameTarget::cached(FramebufferTargetGeometry::new(
                    output.physical_width(),
                    output.physical_height(),
                ));
                full.cached_565_mut().copy_from_slice(&backdrop);
                assert!(
                    renderer
                        .compose_layer_over_backdrop_to_oriented_cached_with_state(
                            &mut full, &backdrop, output, false, false,
                        )
                        .composed
                );
                assert_eq!(
                    incremental.cached_565(),
                    full.cached_565(),
                    "CRT retained parity failed for route={:?}, orientation={orientation:?}",
                    display.output_route()
                );

                let next_spans = crt_overlay_foreground_spans(&renderer);
                retained.commit(next_key, &next_spans);
                assert_eq!(retained.key(), Some(next_key));
            }
        }
    }

    #[test]
    fn rotated_crt_backdrop_compositor_matches_scalar_reference_without_allocating() {
        let display = native_crt_240_display();
        let metrics = CrtUiMetrics::for_display(&display);
        let games = games("arcade", 48);
        for orientation in [
            crate::ui_display::ScreenOrientation::MonitorClockwise,
            crate::ui_display::ScreenOrientation::MonitorCounterclockwise,
        ] {
            let layout = UiLayoutGeometry::for_display(&display, orientation);
            for search in [false, true] {
                let arcade = CrtArcadeLayout::for_layout(layout, metrics, search);
                let mut renderer = ArcadeListRenderer::new_for_crt_display(metrics, &display);
                renderer.set_crt_portrait_rows(true);
                renderer
                    .set_geometry_for_visible_height(arcade.list_geometry(), arcade.list.height);
                renderer.draw(ArcadeGameView::contiguous(&games), 4, 0.0, true);
                let output = layout.output_layout();
                let backdrop = (0..output.len())
                    .map(|index| Rgb565Pixel(index as u16 ^ 0x5a5a))
                    .collect::<Vec<_>>();
                for backdrop_is_fresh in [false, true] {
                    let mut optimized = UiFrameTarget::cached(FramebufferTargetGeometry::new(
                        output.physical_width(),
                        output.physical_height(),
                    ));
                    optimized.cached_565_mut().copy_from_slice(&backdrop);
                    let mut scalar = backdrop.clone();
                    scalar_crt_backdrop_composition(
                        &renderer,
                        &mut scalar,
                        &backdrop,
                        output,
                        backdrop_is_fresh,
                    );
                    let stats = renderer.compose_layer_over_backdrop_to_oriented_cached_with_state(
                        &mut optimized,
                        &backdrop,
                        output,
                        false,
                        backdrop_is_fresh,
                    );
                    assert!(stats.composed);
                    assert_eq!(stats.restored_pixels == 0, backdrop_is_fresh);
                    assert!(stats.foreground_pixels > 0);
                    assert_eq!(optimized.cached_565(), scalar);
                }

                renderer.draw(
                    ArcadeGameView::contiguous(&games),
                    5,
                    crt_arcade_row_height(metrics.game_row_height, true) as f32,
                    false,
                );
                let capacities = (
                    renderer.surface.capacity(),
                    renderer.surface_nonfill_runs.capacity(),
                    renderer.surface_selected_text_runs.capacity(),
                    renderer.band_scratch.capacity(),
                );
                let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(
                    output.physical_width(),
                    output.physical_height(),
                ));
                for _ in 0..32 {
                    let stats = renderer.compose_layer_over_backdrop_to_oriented_cached_with_state(
                        &mut target,
                        &backdrop,
                        output,
                        false,
                        false,
                    );
                    assert!(stats.composed);
                }
                assert_eq!(
                    capacities,
                    (
                        renderer.surface.capacity(),
                        renderer.surface_nonfill_runs.capacity(),
                        renderer.surface_selected_text_runs.capacity(),
                        renderer.band_scratch.capacity(),
                    )
                );
            }
        }
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
    fn first_stationary_frame_after_scroll_only_resolves_visible_hash() {
        let games = games("arcade", 48);
        let mut renderer = ArcadeListRenderer::new();
        assert!(matches!(
            renderer.draw(ArcadeGameView::contiguous(&games), 0, 0.0, false),
            Some(ArcadeListUpdate::Full(_))
        ));
        let visual_index = 1.0 / ARCADE_ROW_HEIGHT as f32;
        assert!(matches!(
            renderer.draw(ArcadeGameView::contiguous(&games), 0, visual_index, false,),
            Some(ArcadeListUpdate::Scroll { .. })
        ));

        assert_eq!(
            renderer.draw(ArcadeGameView::contiguous(&games), 0, visual_index, false,),
            None
        );
        assert_eq!(renderer.last_update_reason, ArcadeListUpdateReason::None);
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
                    delta_x: 0,
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
        assert_eq!(crt.style.meta_typeface, ConsoleTypeface::Spleen6x12Small);
        assert_eq!(crt.style.meta_font_px, 12.0);
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
        assert_eq!(hdmi.style.meta_font_px, ARCADE_LIST_META_FONT_PX);
        assert!(!hdmi.style.crt_palette);
        assert_eq!(hdmi.style.background.0, ARCADE_LIST_BG_COLOR.0);
        assert_eq!(hdmi.style.badge_fill.0, ARCADE_NEW_BADGE_FILL.0);
    }

    #[test]
    fn crt_portrait_rows_are_double_height_without_separators() {
        let mut crt = ArcadeListRenderer::new_for_crt(24);
        assert_eq!(crt.style.row_height, 24);
        assert_eq!(crt.style.separator_top, 1);

        crt.set_crt_portrait_rows(true);

        assert_eq!(crt.style.row_height, 48);
        assert_eq!(crt.style.separator_top, 0);
        assert_eq!(crt.style.separator_bottom, 0);

        crt.set_crt_portrait_rows(false);

        assert_eq!(crt.style.row_height, 24);
        assert_eq!(crt.style.separator_top, 1);
    }

    #[test]
    fn coverage_max_filters_only_the_crt_240_arcade_glyphs() {
        let baseline_display = crt_240_display();
        let coverage_display =
            crt_240_display().with_crt_font_experiment(CrtFontExperiment::CoverageMax);
        let baseline = ArcadeListRenderer::new_for_crt_display(
            CrtUiMetrics::for_display(&baseline_display),
            &baseline_display,
        );
        let coverage = ArcadeListRenderer::new_for_crt_display(
            CrtUiMetrics::for_display(&coverage_display),
            &coverage_display,
        );
        let hdmi = ArcadeListRenderer::new();

        assert_eq!(
            baseline.style.glyph_row_filter,
            ConsoleGlyphRowFilter::Native
        );
        assert_eq!(
            coverage.style.glyph_row_filter,
            ConsoleGlyphRowFilter::PairwiseMax
        );
        assert_eq!(hdmi.style.glyph_row_filter, ConsoleGlyphRowFilter::Native);
    }

    #[test]
    fn dominant_row_filters_only_the_crt_240_arcade_glyphs() {
        let dominant_display =
            crt_240_display().with_crt_font_experiment(CrtFontExperiment::DominantRow);
        let dominant = ArcadeListRenderer::new_for_crt_display(
            CrtUiMetrics::for_display(&dominant_display),
            &dominant_display,
        );
        let hdmi = ArcadeListRenderer::new();

        assert_eq!(
            dominant.style.glyph_row_filter,
            ConsoleGlyphRowFilter::PairwiseDominant
        );
        assert_eq!(hdmi.style.glyph_row_filter, ConsoleGlyphRowFilter::Native);
    }

    #[test]
    fn xerxes_replaces_only_the_crt_240_arcade_title_typeface() {
        let baseline_display = crt_240_display();
        let xerxes_display = crt_240_display().with_crt_font_experiment(CrtFontExperiment::Xerxes);
        let baseline = ArcadeListRenderer::new_for_crt_display(
            CrtUiMetrics::for_display(&baseline_display),
            &baseline_display,
        );
        let xerxes = ArcadeListRenderer::new_for_crt_display(
            CrtUiMetrics::for_display(&xerxes_display),
            &xerxes_display,
        );
        let hdmi = ArcadeListRenderer::new();

        assert_eq!(
            baseline.style.title_typeface,
            ConsoleTypeface::Yesterday10Perfect
        );
        assert_eq!(xerxes.style.title_typeface, ConsoleTypeface::Xerxes10);
        assert_eq!(xerxes.style.glyph_row_filter, ConsoleGlyphRowFilter::Native);
        assert_eq!(hdmi.style.title_typeface, ConsoleTypeface::Nocive15);
    }

    #[test]
    fn xerxes_perfect_uses_the_exact_32px_crt240_title_resource() {
        let display = crt_240_display().with_crt_font_experiment(CrtFontExperiment::XerxesPerfect);
        let renderer =
            ArcadeListRenderer::new_for_crt_display(CrtUiMetrics::for_display(&display), &display);

        assert_eq!(renderer.style.title_font_px, 32.0);
        assert_eq!(
            renderer.style.title_typeface,
            ConsoleTypeface::Xerxes10Perfect
        );
        assert_eq!(
            renderer.style.glyph_row_filter,
            ConsoleGlyphRowFilter::Native
        );
        assert_eq!(renderer.style.row_height, 32);
    }

    #[test]
    fn yesterday_perfect_uses_the_exact_32px_crt240_title_resource() {
        let display =
            crt_240_display().with_crt_font_experiment(CrtFontExperiment::YesterdayPerfect);
        let renderer =
            ArcadeListRenderer::new_for_crt_display(CrtUiMetrics::for_display(&display), &display);

        assert_eq!(renderer.style.title_font_px, 32.0);
        assert_eq!(
            renderer.style.title_typeface,
            ConsoleTypeface::Yesterday10Perfect
        );
        assert_eq!(
            renderer.style.glyph_row_filter,
            ConsoleGlyphRowFilter::Native
        );
        assert_eq!(renderer.style.row_height, 32);
    }

    #[test]
    fn bacteria_uses_the_exact_32px_crt240_title_resource() {
        let bacteria_display =
            crt_240_display().with_crt_font_experiment(CrtFontExperiment::Bacteria);
        let bacteria = ArcadeListRenderer::new_for_crt_display(
            CrtUiMetrics::for_display(&bacteria_display),
            &bacteria_display,
        );
        let hdmi = ArcadeListRenderer::new();

        assert_eq!(bacteria.style.title_font_px, 32.0);
        assert_eq!(bacteria.style.title_typeface, ConsoleTypeface::Bacteria12);
        assert_eq!(
            bacteria.style.glyph_row_filter,
            ConsoleGlyphRowFilter::Native
        );
        assert_eq!(bacteria.style.row_height, 32);
        assert_eq!(hdmi.style.title_font_px, ARCADE_LIST_FONT_PX);
        assert_eq!(hdmi.style.title_typeface, ConsoleTypeface::Nocive15);
    }

    #[test]
    fn bacteria_half_uses_the_native_16px_crt240_title_resource() {
        let display = crt_240_display().with_crt_font_experiment(CrtFontExperiment::BacteriaHalf);
        let renderer =
            ArcadeListRenderer::new_for_crt_display(CrtUiMetrics::for_display(&display), &display);

        assert_eq!(renderer.style.title_font_px, 16.0);
        assert_eq!(
            renderer.style.title_typeface,
            ConsoleTypeface::Bacteria12Half
        );
        assert_eq!(
            renderer.style.glyph_row_filter,
            ConsoleGlyphRowFilter::Native
        );
        assert_eq!(renderer.style.row_height, 32);
    }

    #[test]
    fn crt240_baseline_restores_yesterday_titles_and_terminus_metadata() {
        let display = crt_240_display();
        let renderer =
            ArcadeListRenderer::new_for_crt_display(CrtUiMetrics::for_display(&display), &display);

        assert_eq!(renderer.style.title_font_px, 32.0);
        assert_eq!(
            renderer.style.title_typeface,
            ConsoleTypeface::Yesterday10Perfect
        );
        assert_eq!(
            renderer.style.meta_typeface,
            ConsoleTypeface::Spleen6x12Small
        );
        assert_eq!(
            renderer.style.glyph_row_filter,
            ConsoleGlyphRowFilter::Native
        );
    }

    #[test]
    fn native_crt240_uses_yesterday_title_resource_and_raster_steps() {
        let display = native_crt_240_display();
        let renderer =
            ArcadeListRenderer::new_for_crt_display(CrtUiMetrics::for_display(&display), &display);

        assert_eq!(renderer.style.title_font_px, 16.0);
        assert_eq!(renderer.style.title_typeface, ConsoleTypeface::Yesterday10);
        assert_eq!(
            renderer.style.meta_typeface,
            ConsoleTypeface::Spleen6x12Small
        );
        assert_eq!(renderer.style.scroll_quantum_y, 1);
        assert_eq!(renderer.style.separator_top, 1);
        assert_eq!(renderer.style.selection_frame_y, 1);
    }

    #[test]
    fn crt_new_badges_are_centered_inside_every_route_row() {
        for (row_height, font_family, raster) in [
            (
                32,
                CrtFontFamily::Spleen6x12,
                ArcadeListRasterMetrics {
                    scroll_quantum_y: 2,
                    separator_y: 2,
                    selection_frame_x: 1,
                    selection_frame_y: 2,
                },
            ),
            (
                19,
                CrtFontFamily::Spleen6x12,
                ArcadeListRasterMetrics::native_crt(),
            ),
            (
                32,
                CrtFontFamily::Spleen6x12,
                ArcadeListRasterMetrics::native_crt(),
            ),
            (
                39,
                CrtFontFamily::Spleen6x12,
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
                (text_top - top).abs_diff(bottom - text_bottom) <= 2,
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
    fn selected_aperture_kernel_matches_scalar_for_hdmi_and_crt() {
        for style in [
            ArcadeListStyle::hdmi(),
            ArcadeListStyle::crt(CrtUiMetrics::for_framebuffer(640, 480)),
        ] {
            let source = [
                style.background_565,
                style.alternate_background_565,
                style.border_565,
                style.badge_fill_565,
                Rgb565Pixel(0x0000),
                Rgb565Pixel(0xffff),
                Rgb565Pixel(0x1234),
                Rgb565Pixel(0xabcd),
                Rgb565Pixel(0x55aa),
            ];
            let mut actual = [Rgb565Pixel(0); 9];
            prepare_selected_aperture_pixels(&mut actual, &source, style);
            let expected = source.map(|pixel| selected_aperture_pixel_with_style(pixel, style));
            assert_eq!(actual, expected);
        }
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

    #[test]
    fn persistent_oriented_layer_tracks_aperture_and_rebuilds_for_layout_changes() {
        let geometry = ArcadeListGeometry::portrait(720, 1280, false);
        for rotation in [
            OutputRotation::Clockwise90,
            OutputRotation::CounterClockwise90,
        ] {
            let output = Rgb565OutputLayout::new(720, 1280, 1280, rotation).unwrap();
            let mut layer = PersistentOrientedArcadeLayer::new();
            assert!(layer.ensure(
                geometry,
                640,
                output,
                PersistentArcadeLayerStyle::Hdmi,
                17,
                0,
            ));
            assert!(layer.needs_full_rebuild());
            assert_eq!(
                layer.content().len(),
                layer.physical_rect().unwrap().width()
                    * layer.physical_rect().unwrap().rows() as usize
            );
            assert!(layer.content().len() < output.len());
            assert_eq!(layer.allocated_bytes(), layer.content().len() * 2);
            let aperture = layer
                .set_selection_aperture(12, ARCADE_ROW_HEIGHT as usize)
                .expect("selection aperture");
            assert_eq!(layer.selection_aperture(), Some(aperture));
            layer.mark_full_rebuild_complete();
            assert!(!layer.needs_full_rebuild());
            assert!(!layer.ensure(
                geometry,
                640,
                output,
                PersistentArcadeLayerStyle::Hdmi,
                17,
                0,
            ));
            assert!(!layer.ensure(
                geometry,
                640,
                output,
                PersistentArcadeLayerStyle::Hdmi,
                17,
                1,
            ));
            assert!(!layer.needs_full_rebuild());
            assert_eq!(layer.key().unwrap().ring_origin, 1);
        }
    }

    #[test]
    fn persistent_oriented_layer_matches_full_compositor_for_styles_flags_and_ring_wraps() {
        let geometry = ArcadeListGeometry::portrait(720, 1280, false);
        let mut games = games("arcade", 16);
        games[1].is_new = true;
        games[14].is_new = true;
        let favourite = games[2].mra_path.to_string();
        for style in [
            PersistentArcadeLayerStyle::Hdmi,
            PersistentArcadeLayerStyle::Crt,
        ] {
            for rotation in [
                OutputRotation::Clockwise90,
                OutputRotation::CounterClockwise90,
            ] {
                let output = Rgb565OutputLayout::new(720, 1280, 1280, rotation).unwrap();
                let mut renderer = match style {
                    PersistentArcadeLayerStyle::Hdmi => ArcadeListRenderer::new(),
                    PersistentArcadeLayerStyle::Crt => ArcadeListRenderer::new_for_crt_metrics(
                        CrtUiMetrics::for_framebuffer(640, 480),
                    ),
                };
                renderer.set_geometry_for_visible_height(geometry, 640);
                renderer.set_favourite_launch_refs([favourite.as_str()]);
                let mut layer = PersistentOrientedArcadeLayer::new();
                for (selected, visual_index, ring_origin) in [(15, 15.0, 15), (0, 0.0, 0)] {
                    assert!(
                        renderer
                            .draw(
                                ArcadeGameView::contiguous(&games),
                                selected,
                                visual_index,
                                true
                            )
                            .is_some()
                    );
                    let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(
                        output.physical_stride(),
                        output.physical_height(),
                    ));
                    renderer.compose_layer_to_oriented_cached(&mut target, output, true);
                    layer.ensure(geometry, 640, output, style, 99, ring_origin);
                    let rect = layer.physical_rect().unwrap();
                    for row in 0..rect.rows() as usize {
                        let source = (rect.y0 + row) * output.physical_stride() + rect.x0;
                        let destination = row * rect.width();
                        layer.content_mut()[destination..destination + rect.width()]
                            .copy_from_slice(&target.cached_565()[source..source + rect.width()]);
                    }
                    let view = layer.view().unwrap();
                    for row in 0..rect.rows() as usize {
                        let source = (rect.y0 + row) * output.physical_stride() + rect.x0;
                        assert_eq!(
                            view.row(rect, row).unwrap(),
                            &target.cached_565()[source..source + rect.width()],
                            "physical layer parity failed for style={style:?}, rotation={rotation:?}, ring={ring_origin}, row={row}"
                        );
                    }
                    layer.mark_full_rebuild_complete();
                }
            }
        }
    }

    #[test]
    fn persistent_oriented_layer_scroll_matches_a_fresh_full_composition() {
        let geometry = ArcadeListGeometry::portrait(720, 1280, false);
        let games = games("arcade", 40);
        for rotation in [
            OutputRotation::Clockwise90,
            OutputRotation::CounterClockwise90,
        ] {
            let output = Rgb565OutputLayout::new(720, 1280, 1280, rotation).unwrap();
            let mut incremental = ArcadeListRenderer::new();
            incremental.set_geometry_for_visible_height(geometry, 640);
            let first = incremental
                .draw(ArcadeGameView::contiguous(&games), 12, 12.0, true)
                .unwrap();
            incremental.compose_persistent_oriented_layer(output, first, 5);
            let initial_trace = incremental.persistent_composition_trace();
            assert_eq!(initial_trace.requested_update, ArcadeListUpdateKind::Full);
            assert_eq!(initial_trace.effective_update, ArcadeListUpdateKind::Full);
            assert_eq!(
                initial_trace.rebuild_reason,
                PersistentArcadeRebuildReason::Initial
            );
            assert!(initial_trace.written_pixels > 0);
            let scroll = incremental
                .draw(ArcadeGameView::contiguous(&games), 13, 13.0, false)
                .unwrap();
            assert!(matches!(scroll, ArcadeListUpdate::Scroll { .. }));
            incremental.compose_persistent_oriented_layer(output, scroll, 5);
            let scroll_trace = incremental.persistent_composition_trace();
            assert_eq!(scroll_trace.requested_update, ArcadeListUpdateKind::Scroll);
            assert_eq!(
                scroll_trace.requested_reason,
                ArcadeListUpdateReason::ScrollDelta
            );
            assert_eq!(scroll_trace.effective_update, ArcadeListUpdateKind::Scroll);
            assert_eq!(
                scroll_trace.rebuild_reason,
                PersistentArcadeRebuildReason::None
            );
            assert!(scroll_trace.written_pixels > 0);

            let mut reference = ArcadeListRenderer::new();
            reference.set_geometry_for_visible_height(geometry, 640);
            reference
                .draw(ArcadeGameView::contiguous(&games), 13, 13.0, true)
                .unwrap();
            let mut expected = UiFrameTarget::cached(FramebufferTargetGeometry::new(
                output.physical_stride(),
                output.physical_height(),
            ));
            reference.compose_layer_to_oriented_cached(&mut expected, output, true);

            let view = incremental.persistent_oriented_layer_view().unwrap();
            let rect = view.rect();
            for row in 0..rect.rows() as usize {
                let source = (rect.y0 + row) * output.physical_stride() + rect.x0;
                assert_eq!(
                    view.row(rect, row).unwrap(),
                    &expected.cached_565()[source..source + rect.width()],
                    "incremental physical layer mismatch for {rotation:?}, row={row}"
                );
            }
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
