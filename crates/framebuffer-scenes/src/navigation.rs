// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Portable RGB565 navigation-transition geometry, buffers, and rasterization.

use crate::Rgb565Pixel;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const SMOOTH_CURVE_INTERVALS: usize = 256;
static SMOOTH_CURVE_Q16: OnceLock<[u16; SMOOTH_CURVE_INTERVALS + 1]> = OnceLock::new();

pub fn warm_navigation_transition_rasterizer() {
    let _ = SMOOTH_CURVE_Q16.get_or_init(build_smooth_curve_q16);
}

pub const PROGRESS_MAX: u16 = u16::MAX;
pub const SUPER_SCALER_COVER_PROGRESS: u16 = 31_457;
const CRT_SWEEP_END_Q16: u16 = 13_107;
const CRT_CLEAR_START_Q16: u16 = 52_428;
const CRT_SCANLINE_PERIOD_ROWS: usize = 3;
const SYSTEM_ROW_OFFSCREEN_MARGIN: isize = 24;
const SYSTEM_ROW_DISTANCE_MARGIN: isize = 12;
// The system-browser reveal owns roughly 755 ms of the 1.44 second transition.
// These Q16 windows therefore correspond to a 220 ms spring and 36 ms stagger.
const SYSTEM_ROW_TRAVEL_Q16: u16 = 19_100;
const SYSTEM_ROW_STAGGER_Q16: u16 = 3_125;
const SYSTEM_SELECTED_ROW_START_Q16: u16 = 6_000;
const SUPER_SCALER_TEXTURE_FADE_START_Q16: u16 = 8_000;
const SUPER_SCALER_TEXTURE_FADE_END_Q16: u16 = 16_000;
const DEFAULT_PREPARATION_TIMEOUT_US: u64 = 5_000_000;
const NAVIGATION_TRANSITION_DURATION_US: u64 = 300_000;
const SETTINGS_PAGE_SOURCE_TRAVEL_DIVISOR: isize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationTransitionEdge {
    HomeToConsoles,
    HomeToArcade,
    ConsolesToSystem,
}

impl NavigationTransitionEdge {
    pub const fn enters_system_browser(self) -> bool {
        matches!(self, Self::HomeToArcade | Self::ConsolesToSystem)
    }

    pub const fn duration_us(self) -> u64 {
        NAVIGATION_TRANSITION_DURATION_US
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::HomeToConsoles => "home-consoles",
            Self::HomeToArcade => "home-arcade",
            Self::ConsolesToSystem => "consoles-system",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
        [
            Self::HomeToConsoles,
            Self::HomeToArcade,
            Self::ConsolesToSystem,
        ]
        .into_iter()
        .find(|edge| edge.label() == normalized)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationTransitionDirection {
    Forward,
    Reverse,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum NavigationTransitionRenderer {
    #[default]
    SuperScaler,
    SettingsPage,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsPageTransitionAxis {
    #[default]
    Horizontal,
    Vertical,
    VerticalReversed,
}

impl NavigationTransitionDirection {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Reverse => "reverse",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NavigationTransitionRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl NavigationTransitionRect {
    pub const fn right(self) -> u16 {
        self.x.saturating_add(self.width)
    }

    pub const fn bottom(self) -> u16 {
        self.y.saturating_add(self.height)
    }

    pub const fn fits(self, width: usize, height: usize) -> bool {
        self.width > 0
            && self.height > 0
            && self.right() as usize <= width
            && self.bottom() as usize <= height
    }
}

pub fn hdmi_navigation_geometry(
    frame_width: usize,
    frame_height: usize,
    selected_index: usize,
    scroll_x: i32,
    root_menu: bool,
    edge: NavigationTransitionEdge,
    selected_label: &str,
) -> NavigationTransitionGeometry {
    const OUTER_PADDING: f32 = 18.0;
    const HEADER_HEIGHT: f32 = 42.0;
    const HEADER_GAP: f32 = 14.0;
    const FOOTER_HEIGHT: f32 = 30.0;
    const FOOTER_GAP: f32 = 14.0;
    const ROOT_TILE_GAP: f32 = 16.0;
    const CANONICAL_VIEWPORT_WIDTH: f32 = 924.0;
    const CANONICAL_NARROW_TILE_WIDTH: f32 = 191.0;
    const CANONICAL_NARROW_TILE_GAP: f32 = 16.0;
    let viewport_width = frame_width.saturating_sub(36) as f32;
    let (tile_width, tile_pitch, displayed_scroll_x) = if root_menu {
        let tile_width = (viewport_width - 3.0 * ROOT_TILE_GAP) / 4.0;
        (tile_width, tile_width + ROOT_TILE_GAP, 0.0)
    } else {
        let scale = viewport_width / CANONICAL_VIEWPORT_WIDTH;
        let tile_width = CANONICAL_NARROW_TILE_WIDTH * scale;
        let tile_gap = CANONICAL_NARROW_TILE_GAP * scale;
        (
            tile_width,
            tile_width + tile_gap,
            scroll_x.max(0) as f32 * scale,
        )
    };
    let card_height = (frame_height as f32
        - OUTER_PADDING * 2.0
        - HEADER_HEIGHT
        - HEADER_GAP
        - FOOTER_GAP
        - FOOTER_HEIGHT)
        .max(1.0);
    let unclamped_x = OUTER_PADDING + selected_index as f32 * tile_pitch - displayed_scroll_x;
    let max_x = (frame_width as f32 - tile_width).max(0.0);
    let card_x = unclamped_x.round().clamp(0.0, max_x);
    let card_y = OUTER_PADDING + HEADER_HEIGHT + HEADER_GAP;
    let source_card = NavigationTransitionRect {
        x: card_x as u16,
        y: card_y.round() as u16,
        width: tile_width.round().max(1.0) as u16,
        height: card_height.round().max(1.0) as u16,
    };
    let label_width = source_card.width.saturating_sub(32).max(1);
    let label_columns = selected_label.chars().count().max(1);
    let title_lines = label_columns
        .saturating_mul(16)
        .div_ceil(label_width as usize)
        .clamp(1, 3);
    let title_height = (title_lines * 16) as u16;
    let title_group_height = title_height.saturating_add(14);
    let source_label = NavigationTransitionRect {
        x: source_card.x.saturating_add(16),
        y: source_card
            .y
            .saturating_add(source_card.height / 2)
            .saturating_sub(title_group_height / 2),
        width: label_width,
        height: title_height.min(source_card.height),
    };
    let destination_title = if edge.enters_system_browser() {
        NavigationTransitionRect {
            x: 16,
            y: 16,
            width: label_columns
                .saturating_mul(24)
                .min(frame_width.saturating_sub(32))
                .min(u16::MAX as usize) as u16,
            height: 24,
        }
    } else {
        NavigationTransitionRect {
            x: 32,
            y: 24,
            width: label_columns
                .saturating_mul(16)
                .min(frame_width.saturating_sub(160))
                .min(u16::MAX as usize) as u16,
            height: 16,
        }
    };
    let source_detail = NavigationTransitionRect {
        x: source_label.x,
        y: source_label.bottom().saturating_add(3),
        width: source_label.width,
        height: 10,
    };
    let destination_detail = NavigationTransitionRect {
        x: destination_title.x,
        y: destination_title.bottom().saturating_add(2),
        width: destination_title.width.max(80),
        height: 10,
    };
    let (label_ascii, label_len) = navigation_label_ascii(selected_label);
    NavigationTransitionGeometry {
        label_signature: navigation_label_signature(selected_label),
        label_ascii,
        label_len,
        source_card,
        source_label,
        source_detail,
        destination_title,
        destination_detail,
        destination_list: NavigationTransitionRect {
            x: scale_hdmi_x(8, frame_width),
            y: scale_hdmi_y(56, frame_height),
            width: scale_hdmi_x(510, frame_width),
            height: scale_hdmi_y(452, frame_height),
        },
        destination_selected_row: NavigationTransitionRect {
            x: scale_hdmi_x(8, frame_width),
            y: scale_hdmi_y(248, frame_height),
            width: scale_hdmi_x(510, frame_width),
            height: scale_hdmi_y(48, frame_height),
        },
        destination_preview: NavigationTransitionRect {
            x: scale_hdmi_x(560, frame_width),
            y: scale_hdmi_y(102, frame_height),
            width: scale_hdmi_x(320, frame_width),
            height: scale_hdmi_y(320, frame_height),
        },
        destination_footer: NavigationTransitionRect {
            x: scale_hdmi_x(8, frame_width),
            y: scale_hdmi_y(512, frame_height),
            width: scale_hdmi_x(510, frame_width),
            height: scale_hdmi_y(20, frame_height),
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrtNavigationLayout {
    pub content_x: usize,
    pub content_y: usize,
    pub content_width: usize,
    pub content_height: usize,
    pub grid_x: usize,
    pub grid_y: usize,
    pub header_height: usize,
    pub footer_height: usize,
    pub heading_font_height: usize,
    pub title_font_height: usize,
    pub detail_font_height: usize,
    pub game_row_height: usize,
}

impl CrtNavigationLayout {
    const fn content_right(self) -> usize {
        self.content_x.saturating_add(self.content_width)
    }

    const fn content_bottom(self) -> usize {
        self.content_y.saturating_add(self.content_height)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn crt_navigation_geometry(
    frame_width: usize,
    frame_height: usize,
    layout: CrtNavigationLayout,
    selected_index: usize,
    item_count: usize,
    root_menu: bool,
    edge: NavigationTransitionEdge,
    selected_label: &str,
) -> NavigationTransitionGeometry {
    let grid_x = layout.grid_x.max(1);
    let grid_y = layout.grid_y.max(1);
    let header_height = layout.header_height.max(1);
    let footer_height = layout.footer_height.max(1);
    let title_height = layout.title_font_height.max(1);
    let detail_height = layout.detail_font_height.max(1);
    let card_x = layout.content_x.saturating_add(grid_x * 2);
    let card_width = layout.content_width.saturating_sub(grid_x * 4).max(1);
    let viewport_y = layout
        .content_y
        .saturating_add(grid_y * 3)
        .saturating_add(header_height);
    let viewport_height = layout
        .content_height
        .saturating_sub(header_height)
        .saturating_sub(footer_height)
        .saturating_sub(grid_y * 6)
        .max(1);
    let card_height = if root_menu {
        viewport_height.saturating_sub(grid_y * 3) / 4
    } else {
        viewport_height.saturating_sub(grid_y * 4) * 2 / 9
    }
    .max(1);
    let card_pitch = card_height.saturating_add(grid_y);
    let bottom_window = !root_menu && item_count > 4 && selected_index >= item_count - 2;
    let max_first = item_count.saturating_sub(5);
    let first_visible = if root_menu {
        0
    } else if bottom_window {
        max_first
    } else {
        selected_index.saturating_sub(2).min(max_first)
    };
    let leading_clip = usize::from(bottom_window) * (card_pitch / 2);
    let card_y = viewport_y
        .saturating_add(selected_index.saturating_sub(first_visible) * card_pitch)
        .saturating_sub(leading_clip);
    let source_card = clipped_navigation_rect(
        card_x,
        card_y,
        card_width,
        card_height,
        frame_width,
        frame_height,
    );
    let label_x = card_x
        .saturating_add(grid_x * 3)
        .saturating_add(title_height);
    let label_width = card_x
        .saturating_add(card_width)
        .saturating_sub(label_x)
        .saturating_sub(grid_x * 2)
        .max(1);
    let text_group_height = title_height.saturating_add(detail_height);
    let label_y = card_y.saturating_add(card_height.saturating_sub(text_group_height) / 2);
    let source_label = clipped_navigation_rect(
        label_x,
        label_y,
        label_width,
        title_height,
        frame_width,
        frame_height,
    );
    let source_detail = clipped_navigation_rect(
        label_x,
        label_y.saturating_add(title_height),
        label_width,
        detail_height,
        frame_width,
        frame_height,
    );
    let destination_font_height = if edge.enters_system_browser() {
        layout.heading_font_height.max(1)
    } else {
        title_height
    };
    let title_x = layout.content_x.saturating_add(grid_x * 3);
    let title_y = layout
        .content_y
        .saturating_add(grid_y * 2)
        .saturating_add(header_height.saturating_sub(destination_font_height) / 2);
    let title_width = selected_label
        .chars()
        .count()
        .max(1)
        .saturating_mul(destination_font_height)
        .min(layout.content_right().saturating_sub(title_x).max(1));
    let destination_title = clipped_navigation_rect(
        title_x,
        title_y,
        title_width,
        destination_font_height,
        frame_width,
        frame_height,
    );
    let list_x = layout.content_x.saturating_add(grid_x * 2);
    let list_y = layout
        .content_y
        .saturating_add(header_height)
        .saturating_add(grid_y * 3);
    let list_width = layout.content_width.saturating_sub(grid_x * 4).max(1);
    let list_height = layout
        .content_bottom()
        .saturating_sub(footer_height)
        .saturating_sub(grid_y * 3)
        .saturating_sub(list_y)
        .max(1);
    let row_height = layout.game_row_height.max(1);
    let selected_row_y = list_y.saturating_add(list_height.saturating_sub(row_height) / 2);
    let destination_list = clipped_navigation_rect(
        list_x,
        list_y,
        list_width,
        list_height,
        frame_width,
        frame_height,
    );
    let (label_ascii, label_len) = navigation_label_ascii(selected_label);
    NavigationTransitionGeometry {
        label_signature: navigation_label_signature(selected_label),
        label_ascii,
        label_len,
        source_card,
        source_label,
        source_detail,
        destination_title,
        destination_detail: clipped_navigation_rect(
            destination_title.x as usize,
            destination_title.bottom() as usize,
            destination_title.width as usize,
            detail_height,
            frame_width,
            frame_height,
        ),
        destination_list,
        destination_selected_row: clipped_navigation_rect(
            list_x,
            selected_row_y,
            list_width,
            row_height,
            frame_width,
            frame_height,
        ),
        destination_preview: NavigationTransitionRect::default(),
        destination_footer: clipped_navigation_rect(
            list_x,
            layout
                .content_bottom()
                .saturating_sub(grid_y * 2)
                .saturating_sub(footer_height),
            list_width,
            footer_height,
            frame_width,
            frame_height,
        ),
    }
}

fn clipped_navigation_rect(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    frame_width: usize,
    frame_height: usize,
) -> NavigationTransitionRect {
    let x = x.min(frame_width);
    let y = y.min(frame_height);
    NavigationTransitionRect {
        x: x.min(u16::MAX as usize) as u16,
        y: y.min(u16::MAX as usize) as u16,
        width: width
            .min(frame_width.saturating_sub(x))
            .min(u16::MAX as usize) as u16,
        height: height
            .min(frame_height.saturating_sub(y))
            .min(u16::MAX as usize) as u16,
    }
}

fn scale_hdmi_x(value: usize, frame_width: usize) -> u16 {
    value
        .saturating_mul(frame_width)
        .div_ceil(960)
        .min(u16::MAX as usize) as u16
}

fn scale_hdmi_y(value: usize, frame_height: usize) -> u16 {
    value
        .saturating_mul(frame_height)
        .div_ceil(540)
        .min(u16::MAX as usize) as u16
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NavigationTransitionGeometry {
    pub label_signature: u64,
    pub label_ascii: [u8; 32],
    pub label_len: u8,
    pub source_card: NavigationTransitionRect,
    pub source_label: NavigationTransitionRect,
    pub source_detail: NavigationTransitionRect,
    pub destination_title: NavigationTransitionRect,
    pub destination_detail: NavigationTransitionRect,
    pub destination_list: NavigationTransitionRect,
    pub destination_selected_row: NavigationTransitionRect,
    pub destination_preview: NavigationTransitionRect,
    pub destination_footer: NavigationTransitionRect,
}

fn navigation_label_signature(label: &str) -> u64 {
    label.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ byte.to_ascii_lowercase() as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn navigation_label_ascii(label: &str) -> ([u8; 32], u8) {
    let mut bytes = [0u8; 32];
    let mut length = 0usize;
    for byte in label.bytes().take(bytes.len()) {
        bytes[length] = if byte.is_ascii() {
            byte.to_ascii_uppercase()
        } else {
            b'?'
        };
        length += 1;
    }
    (bytes, length as u8)
}

fn source_text_group(geometry: NavigationTransitionGeometry) -> NavigationTransitionRect {
    if geometry.source_detail.width == 0 || geometry.source_detail.height == 0 {
        return geometry.source_label;
    }
    let right = geometry
        .source_label
        .right()
        .max(geometry.source_detail.right());
    let bottom = geometry
        .source_label
        .bottom()
        .max(geometry.source_detail.bottom());
    NavigationTransitionRect {
        x: geometry.source_label.x.min(geometry.source_detail.x),
        y: geometry.source_label.y.min(geometry.source_detail.y),
        width: right.saturating_sub(geometry.source_label.x.min(geometry.source_detail.x)),
        height: bottom.saturating_sub(geometry.source_label.y.min(geometry.source_detail.y)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationTransitionRequest {
    pub edge: NavigationTransitionEdge,
    pub direction: NavigationTransitionDirection,
    pub geometry: NavigationTransitionGeometry,
    pub duration_us: u64,
    pub preparation_timeout_us: u64,
    renderer: NavigationTransitionRenderer,
    settings_axis: SettingsPageTransitionAxis,
}

impl NavigationTransitionRequest {
    pub const fn new(
        edge: NavigationTransitionEdge,
        direction: NavigationTransitionDirection,
        geometry: NavigationTransitionGeometry,
    ) -> Self {
        Self {
            edge,
            direction,
            geometry,
            duration_us: edge.duration_us(),
            preparation_timeout_us: DEFAULT_PREPARATION_TIMEOUT_US,
            renderer: NavigationTransitionRenderer::SuperScaler,
            settings_axis: SettingsPageTransitionAxis::Horizontal,
        }
    }

    pub fn settings_page(direction: NavigationTransitionDirection) -> Self {
        Self {
            edge: NavigationTransitionEdge::HomeToConsoles,
            direction,
            geometry: NavigationTransitionGeometry::default(),
            duration_us: NAVIGATION_TRANSITION_DURATION_US,
            preparation_timeout_us: DEFAULT_PREPARATION_TIMEOUT_US,
            renderer: NavigationTransitionRenderer::SettingsPage,
            settings_axis: SettingsPageTransitionAxis::Horizontal,
        }
    }

    pub fn settings_page_on_axis(
        direction: NavigationTransitionDirection,
        axis: SettingsPageTransitionAxis,
    ) -> Self {
        Self {
            settings_axis: axis,
            ..Self::settings_page(direction)
        }
    }

    #[must_use]
    pub const fn is_super_scaler(self) -> bool {
        matches!(self.renderer, NavigationTransitionRenderer::SuperScaler)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NavigationTransitionPhase {
    #[default]
    Idle,
    Capture,
    Expand,
    Covered,
    Reveal,
    Reversing,
    Settled,
}

impl NavigationTransitionPhase {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Capture => "capture",
            Self::Expand => "expand",
            Self::Covered => "covered",
            Self::Reveal => "reveal",
            Self::Reversing => "reversing",
            Self::Settled => "settled",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationTransitionFailure {
    DestinationTimeout,
    SnapshotSizeMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationTransitionEndpoint {
    Source,
    Destination,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationTransitionCompletion {
    pub endpoint: NavigationTransitionEndpoint,
    pub failure: Option<NavigationTransitionFailure>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationTransitionFrame {
    pub phase: NavigationTransitionPhase,
    pub progress_q16: u16,
    pub cover_progress_q16: u16,
    pub reveal_progress_q16: u16,
    pub owns_full_frame: bool,
    pub endpoint: Option<NavigationTransitionEndpoint>,
    pub failure: Option<NavigationTransitionFailure>,
    pub reverse_origin_q16: u16,
    pub reverse_leg_progress_q16: u16,
}

impl Default for NavigationTransitionFrame {
    fn default() -> Self {
        Self {
            phase: NavigationTransitionPhase::Idle,
            progress_q16: 0,
            cover_progress_q16: 0,
            reveal_progress_q16: 0,
            owns_full_frame: false,
            endpoint: None,
            failure: None,
            reverse_origin_q16: 0,
            reverse_leg_progress_q16: 0,
        }
    }
}

pub const fn request_cover_progress_q16(request: NavigationTransitionRequest) -> u16 {
    let forward_cover = match request.renderer {
        NavigationTransitionRenderer::SuperScaler => SUPER_SCALER_COVER_PROGRESS,
        NavigationTransitionRenderer::SettingsPage => PROGRESS_MAX / 2,
    };
    match request.direction {
        NavigationTransitionDirection::Forward => forward_cover,
        NavigationTransitionDirection::Reverse => PROGRESS_MAX - forward_cover,
    }
}

pub fn forward_progress_q16_at_elapsed(total_us: u64, elapsed_us: u64) -> u16 {
    let total_us = total_us.max(1);
    let cover_progress = SUPER_SCALER_COVER_PROGRESS;
    let cover_us = total_us.saturating_mul(cover_progress as u64) / PROGRESS_MAX as u64;
    let elapsed_us = elapsed_us.min(total_us);
    if elapsed_us <= cover_us {
        scale_progress(elapsed_us, cover_us.max(1), cover_progress)
    } else {
        cover_progress.saturating_add(scale_progress(
            elapsed_us.saturating_sub(cover_us),
            total_us.saturating_sub(cover_us).max(1),
            PROGRESS_MAX - cover_progress,
        ))
    }
}

pub fn scale_progress(elapsed_us: u64, duration_us: u64, maximum: u16) -> u16 {
    elapsed_us
        .min(duration_us)
        .saturating_mul(maximum as u64)
        .saturating_div(duration_us.max(1)) as u16
}

#[derive(Debug, Default)]
pub struct NavigationTransitionBuffers {
    width: usize,
    height: usize,
    source: Vec<Rgb565Pixel>,
    destination: Vec<Rgb565Pixel>,
    working: Vec<Rgb565Pixel>,
    scale_source_x: Vec<usize>,
    scale_source_y: Vec<usize>,
    scale_excluded_x: Vec<bool>,
    scale_dither_x: Vec<bool>,
    source_ready: bool,
    destination_ready: bool,
}

impl NavigationTransitionBuffers {
    pub fn new(width: usize, height: usize) -> Self {
        let mut buffers = Self::default();
        buffers.resize(width, height);
        buffers
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        if self.width == width && self.height == height {
            return;
        }
        let len = width.saturating_mul(height);
        self.source.resize(len, Rgb565Pixel(0));
        self.destination.resize(len, Rgb565Pixel(0));
        self.working.resize(len, Rgb565Pixel(0));
        self.scale_source_x.resize(width, 0);
        self.scale_source_y.resize(height, 0);
        self.scale_excluded_x.resize(width, false);
        self.scale_dither_x.resize(width.saturating_mul(4), false);
        self.width = width;
        self.height = height;
        self.clear_ready();
    }

    pub fn begin_capture(&mut self) {
        self.clear_ready();
    }

    pub fn capture_source(
        &mut self,
        pixels: &[Rgb565Pixel],
    ) -> Result<(), NavigationTransitionFailure> {
        self.source_ready = false;
        copy_snapshot(&mut self.source, pixels)?;
        self.source_ready = true;
        Ok(())
    }

    pub fn capture_destination(
        &mut self,
        pixels: &[Rgb565Pixel],
    ) -> Result<(), NavigationTransitionFailure> {
        self.destination_ready = false;
        copy_snapshot(&mut self.destination, pixels)?;
        self.destination_ready = true;
        Ok(())
    }

    pub fn clear_ready(&mut self) {
        self.source_ready = false;
        self.destination_ready = false;
    }

    pub const fn width(&self) -> usize {
        self.width
    }

    pub const fn height(&self) -> usize {
        self.height
    }

    pub const fn source_ready(&self) -> bool {
        self.source_ready
    }

    pub const fn destination_ready(&self) -> bool {
        self.destination_ready
    }

    pub fn source(&self) -> Option<&[Rgb565Pixel]> {
        self.source_ready.then_some(self.source.as_slice())
    }

    pub fn destination(&self) -> Option<&[Rgb565Pixel]> {
        self.destination_ready
            .then_some(self.destination.as_slice())
    }

    pub fn working_mut(&mut self) -> &mut [Rgb565Pixel] {
        self.working.as_mut_slice()
    }

    pub fn working(&self) -> &[Rgb565Pixel] {
        self.working.as_slice()
    }

    pub fn copy_source_to_working(&mut self) -> Result<usize, NavigationTransitionFailure> {
        if !self.source_ready || self.working.len() != self.source.len() {
            return Err(NavigationTransitionFailure::SnapshotSizeMismatch);
        }
        self.working.copy_from_slice(&self.source);
        Ok(self.source.len())
    }
}

fn copy_snapshot(
    destination: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
) -> Result<(), NavigationTransitionFailure> {
    if destination.len() != source.len() {
        return Err(NavigationTransitionFailure::SnapshotSizeMismatch);
    }
    destination.copy_from_slice(source);
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NavigationTransitionRenderStats {
    pub render_us: u64,
    pub base_copy_us: u64,
    pub settings_blit_us: u64,
    pub card_scale_us: u64,
    pub destination_reveal_us: u64,
    pub copied_pixels: u64,
    pub filled_pixels: u64,
    pub outline_pixels: u64,
    pub overlay_us: u64,
    pub phosphor_pixels: u64,
    pub scanline_pixels: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct NavigationTransitionFrameInput<'a> {
    pub progress_q16: u16,
    pub direction: NavigationTransitionDirection,
    pub edge: NavigationTransitionEdge,
    pub geometry: NavigationTransitionGeometry,
    pub width: usize,
    pub height: usize,
    pub source: &'a [Rgb565Pixel],
    pub destination: &'a [Rgb565Pixel],
}

pub fn render_navigation_transition(
    buffers: &mut NavigationTransitionBuffers,
    request: NavigationTransitionRequest,
    frame: NavigationTransitionFrame,
) -> Result<NavigationTransitionRenderStats, NavigationTransitionFailure> {
    let started = Instant::now();
    let mut stats = match request.renderer {
        NavigationTransitionRenderer::SuperScaler => {
            let mut stats = render_super_scaler_shell(buffers, request, frame)?;
            render_hero_label_last(buffers, request, frame, &mut stats)?;
            let overlay_started = Instant::now();
            apply_crt_scanline_overlay(
                buffers.working.as_mut_slice(),
                buffers.width,
                buffers.height,
                frame,
                &mut stats,
            );
            stats.overlay_us = overlay_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
            stats
        }
        NavigationTransitionRenderer::SettingsPage => {
            render_settings_page_push(buffers, request, frame)?
        }
    };
    stats.render_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
    Ok(stats)
}

pub fn render_navigation_transition_input(
    buffers: &mut NavigationTransitionBuffers,
    input: NavigationTransitionFrameInput<'_>,
) -> Result<NavigationTransitionRenderStats, NavigationTransitionFailure> {
    if input.source.len() != input.width.saturating_mul(input.height)
        || input.destination.len() != input.source.len()
    {
        return Err(NavigationTransitionFailure::SnapshotSizeMismatch);
    }
    buffers.resize(input.width, input.height);
    buffers.begin_capture();
    buffers.capture_source(input.source)?;
    buffers.capture_destination(input.destination)?;
    let request = NavigationTransitionRequest::new(input.edge, input.direction, input.geometry);
    let cover = request_cover_progress_q16(request);
    let frame = NavigationTransitionFrame {
        phase: match input.direction {
            NavigationTransitionDirection::Forward if input.progress_q16 >= cover => {
                NavigationTransitionPhase::Reveal
            }
            NavigationTransitionDirection::Forward => NavigationTransitionPhase::Expand,
            NavigationTransitionDirection::Reverse => NavigationTransitionPhase::Reversing,
        },
        progress_q16: input.progress_q16,
        cover_progress_q16: input.progress_q16.min(cover),
        reveal_progress_q16: input.progress_q16.saturating_sub(cover),
        owns_full_frame: true,
        reverse_origin_q16: if input.direction == NavigationTransitionDirection::Reverse {
            PROGRESS_MAX
        } else {
            0
        },
        reverse_leg_progress_q16: if input.direction == NavigationTransitionDirection::Reverse {
            PROGRESS_MAX.saturating_sub(input.progress_q16)
        } else {
            0
        },
        ..NavigationTransitionFrame::default()
    };
    render_navigation_transition(buffers, request, frame)
}

fn apply_crt_scanline_overlay(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    frame: NavigationTransitionFrame,
    stats: &mut NavigationTransitionRenderStats,
) {
    if width == 0
        || height == 0
        || working.len() != width.saturating_mul(height)
        || matches!(
            frame.phase,
            NavigationTransitionPhase::Idle | NavigationTransitionPhase::Settled
        )
        || frame.progress_q16 == 0
        || frame.progress_q16 == PROGRESS_MAX
    {
        return;
    }

    let reversing = frame.phase == NavigationTransitionPhase::Reversing;
    let clearing = reversing && frame.reverse_leg_progress_q16 >= CRT_CLEAR_START_Q16;
    let clear_y = clearing.then(|| {
        sweep_y(
            spring_ease_q16(window_q16(
                frame.reverse_leg_progress_q16,
                CRT_CLEAR_START_Q16,
                PROGRESS_MAX,
            )),
            height,
        )
    });

    let mut first_covered_row = None;
    let mut covered_rows = 0usize;
    for y in (1..height).step_by(CRT_SCANLINE_PERIOD_ROWS) {
        let covered = if reversing {
            overlay_row_covered(frame.reverse_origin_q16, y, height)
                && clear_y.is_none_or(|line_y| y as isize >= line_y)
        } else {
            overlay_row_covered(frame.progress_q16, y, height)
        };
        if covered {
            first_covered_row.get_or_insert(y);
            covered_rows += 1;
        } else if first_covered_row.is_some() {
            // Covered scanlines always form one contiguous band.
            break;
        } else {
            continue;
        }
    }
    let Some(first_covered_row) = first_covered_row else {
        return;
    };
    let start = first_covered_row * width;
    let pixels = &mut working[start..];
    darken_rgb565_rows_reference(
        pixels,
        width,
        covered_rows,
        width * CRT_SCANLINE_PERIOD_ROWS,
    );
    stats.phosphor_pixels = stats
        .phosphor_pixels
        .saturating_add((width as u64).saturating_mul(covered_rows as u64));
}

fn darken_rgb565_rows_reference(
    pixels: &mut [Rgb565Pixel],
    width: usize,
    rows: usize,
    stride: usize,
) {
    for row in 0..rows {
        let start = row * stride;
        for pixel in &mut pixels[start..start + width] {
            *pixel = darken_rgb565_7_8(*pixel);
        }
    }
}

fn overlay_row_covered(progress_q16: u16, y: usize, height: usize) -> bool {
    if progress_q16 < CRT_SWEEP_END_Q16 {
        y as isize
            <= sweep_y(
                spring_ease_q16(window_q16(progress_q16, 0, CRT_SWEEP_END_Q16)),
                height,
            )
    } else if progress_q16 <= CRT_CLEAR_START_Q16 {
        true
    } else {
        y as isize
            >= sweep_y(
                spring_ease_q16(window_q16(progress_q16, CRT_CLEAR_START_Q16, PROGRESS_MAX)),
                height,
            )
    }
}

fn sweep_y(progress_q16: u16, height: usize) -> isize {
    let start = -3_isize;
    let distance = height.saturating_add(6) as isize;
    start + distance * progress_q16 as isize / PROGRESS_MAX as isize
}

fn darken_rgb565_7_8(pixel: Rgb565Pixel) -> Rgb565Pixel {
    let value = pixel.0;
    let red = ((value >> 11) & 0x1f) * 7 / 8;
    let green = ((value >> 5) & 0x3f) * 7 / 8;
    let blue = (value & 0x1f) * 7 / 8;
    Rgb565Pixel((red << 11) | (green << 5) | blue)
}

fn render_super_scaler_shell(
    buffers: &mut NavigationTransitionBuffers,
    request: NavigationTransitionRequest,
    frame: NavigationTransitionFrame,
) -> Result<NavigationTransitionRenderStats, NavigationTransitionFailure> {
    let source = buffers
        .source
        .get(..)
        .filter(|_| buffers.source_ready)
        .ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
    let destination = buffers
        .destination
        .get(..)
        .filter(|_| buffers.destination_ready);
    let working = buffers.working.as_mut_slice();
    let scale_source_x = buffers.scale_source_x.as_mut_slice();
    let scale_source_y = buffers.scale_source_y.as_mut_slice();
    let scale_excluded_x = buffers.scale_excluded_x.as_mut_slice();
    let scale_dither_x = buffers.scale_dither_x.as_mut_slice();
    let width = buffers.width;
    let height = buffers.height;
    if working.len() != source.len() || working.len() != width.saturating_mul(height) {
        return Err(NavigationTransitionFailure::SnapshotSizeMismatch);
    }
    let mut stats = NavigationTransitionRenderStats::default();
    let full = NavigationTransitionRect {
        x: 0,
        y: 0,
        width: width.min(u16::MAX as usize) as u16,
        height: height.min(u16::MAX as usize) as u16,
    };
    let shell = Rgb565Pixel(0x1028);
    let mint = Rgb565Pixel(0x07d6);
    let violet = Rgb565Pixel(0x79b8);

    if frame.phase == NavigationTransitionPhase::Settled {
        match frame.endpoint {
            Some(NavigationTransitionEndpoint::Source) => {
                let phase_started = Instant::now();
                working.copy_from_slice(source);
                stats.base_copy_us = elapsed_us(phase_started);
                stats.copied_pixels = source.len() as u64;
                return Ok(stats);
            }
            Some(NavigationTransitionEndpoint::Destination) => {
                let phase_started = Instant::now();
                if let Some(destination) = destination {
                    working.copy_from_slice(destination);
                    stats.copied_pixels = destination.len() as u64;
                } else {
                    working.copy_from_slice(source);
                    stats.copied_pixels = source.len() as u64;
                }
                stats.base_copy_us = elapsed_us(phase_started);
                return Ok(stats);
            }
            None => {}
        }
    }
    if frame.progress_q16 == 0 {
        let phase_started = Instant::now();
        working.copy_from_slice(source);
        stats.base_copy_us = elapsed_us(phase_started);
        stats.copied_pixels = source.len() as u64;
        return Ok(stats);
    }
    if request.direction == NavigationTransitionDirection::Forward
        && frame.reveal_progress_q16 >= 62_000
        && let Some(destination) = destination
    {
        let phase_started = Instant::now();
        working.copy_from_slice(destination);
        stats.base_copy_us = elapsed_us(phase_started);
        stats.copied_pixels = destination.len() as u64;
        return Ok(stats);
    }
    if frame.reveal_progress_q16 == 0 && frame.cover_progress_q16 == PROGRESS_MAX {
        fill_super_scaler_covered_surface(working, width, height, full, shell, &mut stats);
        return Ok(stats);
    }
    let needs_source_base = match request.direction {
        NavigationTransitionDirection::Forward => frame.reveal_progress_q16 == 0,
        NavigationTransitionDirection::Reverse => false,
    };
    if needs_source_base {
        let phase_started = Instant::now();
        working.copy_from_slice(source);
        stats.base_copy_us = stats.base_copy_us.saturating_add(elapsed_us(phase_started));
        stats.copied_pixels = source.len() as u64;
    }

    match request.direction {
        NavigationTransitionDirection::Forward => {
            if frame.reveal_progress_q16 > 0 {
                if let Some(destination) = destination {
                    let phase_started = Instant::now();
                    if request.edge.enters_system_browser() {
                        compose_system_background_horizon(
                            working,
                            destination,
                            width,
                            height,
                            spring_ease_q16(window_q16(frame.reveal_progress_q16, 0, 18_000)),
                            request.geometry.destination_title.bottom() as usize,
                            shell,
                            &mut stats,
                        );
                    } else {
                        fill_super_scaler_covered_surface(
                            working, width, height, full, shell, &mut stats,
                        );
                    }
                    reveal_destination_regions(
                        working,
                        destination,
                        width,
                        height,
                        frame.reveal_progress_q16,
                        request,
                        &mut stats,
                    );
                    stats.destination_reveal_us = stats
                        .destination_reveal_us
                        .saturating_add(elapsed_us(phase_started));
                } else {
                    let phase_started = Instant::now();
                    fill_super_scaler_covered_surface(
                        working, width, height, full, shell, &mut stats,
                    );
                    stats.destination_reveal_us = stats
                        .destination_reveal_us
                        .saturating_add(elapsed_us(phase_started));
                }
            }
            if frame.reveal_progress_q16 == 0 {
                let phase_started = Instant::now();
                render_super_scaler_card_cover(
                    working,
                    source,
                    width,
                    height,
                    request,
                    full,
                    frame.cover_progress_q16,
                    shell,
                    mint,
                    violet,
                    scale_source_x,
                    scale_source_y,
                    scale_excluded_x,
                    scale_dither_x,
                    &mut stats,
                );
                stats.card_scale_us = stats
                    .card_scale_us
                    .saturating_add(elapsed_us(phase_started));
            }
        }
        NavigationTransitionDirection::Reverse => {
            if frame.reveal_progress_q16 == 0 {
                let phase_started = Instant::now();
                conceal_source_regions_inverse(
                    working,
                    source,
                    width,
                    height,
                    frame.cover_progress_q16,
                    request,
                    shell,
                    &mut stats,
                );
                stats.card_scale_us = stats
                    .card_scale_us
                    .saturating_add(elapsed_us(phase_started));
            } else if let Some(destination) = destination {
                let base_started = Instant::now();
                working.copy_from_slice(destination);
                stats.base_copy_us = stats.base_copy_us.saturating_add(elapsed_us(base_started));
                stats.copied_pixels = stats.copied_pixels.saturating_add(destination.len() as u64);
                let phase_started = Instant::now();
                render_super_scaler_card_cover(
                    working,
                    destination,
                    width,
                    height,
                    request,
                    full,
                    PROGRESS_MAX.saturating_sub(frame.reveal_progress_q16),
                    shell,
                    mint,
                    violet,
                    scale_source_x,
                    scale_source_y,
                    scale_excluded_x,
                    scale_dither_x,
                    &mut stats,
                );
                stats.card_scale_us = stats
                    .card_scale_us
                    .saturating_add(elapsed_us(phase_started));
            }
        }
    }
    Ok(stats)
}

fn render_settings_page_push(
    buffers: &mut NavigationTransitionBuffers,
    request: NavigationTransitionRequest,
    frame: NavigationTransitionFrame,
) -> Result<NavigationTransitionRenderStats, NavigationTransitionFailure> {
    let mut working = std::mem::take(&mut buffers.working);
    let result = render_settings_page_transition_into(buffers, request, frame, &mut working);
    buffers.working = working;
    result
}

pub fn render_settings_page_transition_into(
    buffers: &NavigationTransitionBuffers,
    request: NavigationTransitionRequest,
    frame: NavigationTransitionFrame,
    output: &mut [Rgb565Pixel],
) -> Result<NavigationTransitionRenderStats, NavigationTransitionFailure> {
    if !buffers.source_ready
        || buffers.source.len() != buffers.width.saturating_mul(buffers.height)
        || output.len() != buffers.source.len()
        || request.is_super_scaler()
    {
        return Err(NavigationTransitionFailure::SnapshotSizeMismatch);
    }
    let source = buffers.source.as_slice();
    let destination = buffers
        .destination_ready
        .then_some(buffers.destination.as_slice());
    let mut stats = NavigationTransitionRenderStats::default();

    if frame.progress_q16 == 0 || destination.is_none() {
        output.copy_from_slice(source);
        stats.copied_pixels = source.len() as u64;
        return Ok(stats);
    }
    let destination = destination.expect("checked destination snapshot");
    if frame.progress_q16 == PROGRESS_MAX {
        output.copy_from_slice(destination);
        stats.copied_pixels = destination.len() as u64;
        return Ok(stats);
    }

    let travel_q16 = spring_ease_q16(frame.progress_q16) as isize;
    let extent = match request.settings_axis {
        SettingsPageTransitionAxis::Horizontal => buffers.width,
        SettingsPageTransitionAxis::Vertical | SettingsPageTransitionAxis::VerticalReversed => {
            buffers.height
        }
    } as isize;
    let source_travel = extent / SETTINGS_PAGE_SOURCE_TRAVEL_DIVISOR;
    let blit_started = Instant::now();
    let (first, first_offset, second, second_offset) = match request.direction {
        NavigationTransitionDirection::Forward => {
            let source_x = -(source_travel * travel_q16 / PROGRESS_MAX as isize);
            let destination_x = extent - extent * travel_q16 / PROGRESS_MAX as isize;
            (source, source_x, destination, destination_x)
        }
        NavigationTransitionDirection::Reverse => {
            let destination_x = -source_travel + source_travel * travel_q16 / PROGRESS_MAX as isize;
            let source_x = extent * travel_q16 / PROGRESS_MAX as isize;
            (destination, destination_x, source, source_x)
        }
    };
    stats.copied_pixels = blit_settings_pair(
        output,
        first,
        first_offset,
        second,
        second_offset,
        buffers.width,
        buffers.height,
        request.settings_axis,
    );
    stats.settings_blit_us = elapsed_us(blit_started);
    Ok(stats)
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u64::MAX as u128) as u64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SnapshotBlitSpan {
    destination_start: usize,
    source_start: usize,
    len: usize,
}

impl SnapshotBlitSpan {
    fn destination_end(self) -> usize {
        self.destination_start.saturating_add(self.len)
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_settings_pair(
    output: &mut [Rgb565Pixel],
    first: &[Rgb565Pixel],
    first_offset: isize,
    second: &[Rgb565Pixel],
    second_offset: isize,
    width: usize,
    height: usize,
    axis: SettingsPageTransitionAxis,
) -> u64 {
    if output.len() != width.saturating_mul(height)
        || first.len() != output.len()
        || second.len() != output.len()
    {
        return 0;
    }
    let extent = match axis {
        SettingsPageTransitionAxis::Horizontal => width,
        SettingsPageTransitionAxis::Vertical | SettingsPageTransitionAxis::VerticalReversed => {
            height
        }
    };
    let transform_offset = |offset: isize| match axis {
        SettingsPageTransitionAxis::VerticalReversed => -offset,
        SettingsPageTransitionAxis::Horizontal | SettingsPageTransitionAxis::Vertical => offset,
    };
    let Some(first_span) = snapshot_blit_span(extent, transform_offset(first_offset)) else {
        return 0;
    };
    let Some(second_span) = snapshot_blit_span(extent, transform_offset(second_offset)) else {
        return blit_snapshot_span(output, first, width, height, axis, first_span);
    };
    let mut copied = 0_u64;
    for span in snapshot_span_without_cover(first_span, second_span)
        .into_iter()
        .flatten()
    {
        copied =
            copied.saturating_add(blit_snapshot_span(output, first, width, height, axis, span));
    }
    copied.saturating_add(blit_snapshot_span(
        output,
        second,
        width,
        height,
        axis,
        second_span,
    ))
}

fn snapshot_blit_span(extent: usize, offset: isize) -> Option<SnapshotBlitSpan> {
    let destination_start = offset.max(0) as usize;
    let source_start = offset.saturating_neg().max(0) as usize;
    if destination_start >= extent || source_start >= extent {
        return None;
    }
    Some(SnapshotBlitSpan {
        destination_start,
        source_start,
        len: (extent - destination_start).min(extent - source_start),
    })
}

fn snapshot_span_without_cover(
    span: SnapshotBlitSpan,
    cover: SnapshotBlitSpan,
) -> [Option<SnapshotBlitSpan>; 2] {
    let overlap_start = span.destination_start.max(cover.destination_start);
    let overlap_end = span.destination_end().min(cover.destination_end());
    if overlap_start >= overlap_end {
        return [Some(span), None];
    }
    let prefix_len = overlap_start.saturating_sub(span.destination_start);
    let suffix_len = span.destination_end().saturating_sub(overlap_end);
    [
        (prefix_len > 0).then_some(SnapshotBlitSpan {
            len: prefix_len,
            ..span
        }),
        (suffix_len > 0).then_some(SnapshotBlitSpan {
            destination_start: overlap_end,
            source_start: span
                .source_start
                .saturating_add(overlap_end.saturating_sub(span.destination_start)),
            len: suffix_len,
        }),
    ]
}

fn blit_snapshot_span(
    output: &mut [Rgb565Pixel],
    snapshot: &[Rgb565Pixel],
    width: usize,
    height: usize,
    axis: SettingsPageTransitionAxis,
    span: SnapshotBlitSpan,
) -> u64 {
    match axis {
        SettingsPageTransitionAxis::Horizontal => {
            blit_snapshot_x_span(output, snapshot, width, height, span)
        }
        SettingsPageTransitionAxis::Vertical | SettingsPageTransitionAxis::VerticalReversed => {
            blit_snapshot_y_span(output, snapshot, width, span)
        }
    }
}

fn blit_snapshot_y(
    output: &mut [Rgb565Pixel],
    snapshot: &[Rgb565Pixel],
    width: usize,
    height: usize,
    offset_y: isize,
) -> u64 {
    if output.len() != width.saturating_mul(height) || snapshot.len() != output.len() {
        return 0;
    }
    let Some(span) = snapshot_blit_span(height, offset_y) else {
        return 0;
    };
    blit_snapshot_y_span(output, snapshot, width, span)
}

fn blit_snapshot_y_span(
    output: &mut [Rgb565Pixel],
    snapshot: &[Rgb565Pixel],
    width: usize,
    span: SnapshotBlitSpan,
) -> u64 {
    let destination_start = span.destination_start * width;
    let source_start = span.source_start * width;
    let copy_len = span.len * width;
    output[destination_start..destination_start + copy_len]
        .copy_from_slice(&snapshot[source_start..source_start + copy_len]);
    copy_len as u64
}

fn blit_snapshot_x(
    output: &mut [Rgb565Pixel],
    snapshot: &[Rgb565Pixel],
    width: usize,
    height: usize,
    offset_x: isize,
) -> u64 {
    if output.len() != width.saturating_mul(height) || snapshot.len() != output.len() {
        return 0;
    }
    let Some(span) = snapshot_blit_span(width, offset_x) else {
        return 0;
    };
    blit_snapshot_x_span(output, snapshot, width, height, span)
}

fn blit_snapshot_x_span(
    output: &mut [Rgb565Pixel],
    snapshot: &[Rgb565Pixel],
    width: usize,
    height: usize,
    span: SnapshotBlitSpan,
) -> u64 {
    for y in 0..height {
        let destination_start = y * width + span.destination_start;
        let source_start = y * width + span.source_start;
        output[destination_start..destination_start + span.len]
            .copy_from_slice(&snapshot[source_start..source_start + span.len]);
    }
    span.len.saturating_mul(height) as u64
}

#[allow(clippy::too_many_arguments)]
fn render_super_scaler_card_cover(
    working: &mut [Rgb565Pixel],
    snapshot: &[Rgb565Pixel],
    width: usize,
    height: usize,
    request: NavigationTransitionRequest,
    full: NavigationTransitionRect,
    forward_cover_q16: u16,
    shell: Rgb565Pixel,
    mint: Rgb565Pixel,
    violet: Rgb565Pixel,
    scale_source_x: &mut [usize],
    scale_source_y: &mut [usize],
    scale_excluded_x: &mut [bool],
    scale_dither_x: &mut [bool],
    stats: &mut NavigationTransitionRenderStats,
) {
    let rect = super_scaler_card_rect(request.geometry.source_card, full, forward_cover_q16);
    let source_background =
        background_outside_rect(snapshot, width, height, request.geometry.source_card);
    fill_rect_565(
        working,
        width,
        height,
        request.geometry.source_card,
        source_background,
        stats,
    );
    fill_rect_565(working, width, height, rect, shell, stats);
    draw_super_scaler_speed_bands(working, width, height, rect, forward_cover_q16, stats);
    draw_super_scaler_impact_horizon(working, width, height, rect, forward_cover_q16, stats);
    blit_scaled_card_565(
        working,
        snapshot,
        width,
        height,
        request.geometry.source_card,
        rect,
        source_text_group(request.geometry),
        PROGRESS_MAX.saturating_sub(spring_ease_q16(window_q16(
            forward_cover_q16,
            SUPER_SCALER_TEXTURE_FADE_START_Q16,
            SUPER_SCALER_TEXTURE_FADE_END_Q16,
        ))),
        scale_source_x,
        scale_source_y,
        scale_excluded_x,
        scale_dither_x,
        stats,
    );
    draw_super_scaler_impact_envelope(working, width, height, rect, forward_cover_q16, stats);
    let far_to_right = request.geometry.source_card.x as usize
        + request.geometry.source_card.width as usize / 2
        <= width / 2;
    for (echo, lead, cutoff) in [
        (3usize, 19_500u16, 52_000u16),
        (2, 11_000, 57_000),
        (1, 4_500, 62_000),
    ] {
        if forward_cover_q16 < cutoff {
            let echo_rect = super_scaler_card_rect(
                request.geometry.source_card,
                full,
                forward_cover_q16.saturating_add(lead),
            );
            draw_velocity_echo_565(
                working,
                width,
                height,
                echo_rect,
                match echo {
                    1 => violet,
                    2 => Rgb565Pixel(0x40ed),
                    _ => Rgb565Pixel(0x28aa),
                },
                far_to_right,
                stats,
            );
        }
    }
    if forward_cover_q16 < 62_000 {
        draw_outline_565(working, width, height, rect, mint, stats);
    }
}

fn render_hero_label_last(
    buffers: &mut NavigationTransitionBuffers,
    request: NavigationTransitionRequest,
    frame: NavigationTransitionFrame,
    stats: &mut NavigationTransitionRenderStats,
) -> Result<(), NavigationTransitionFailure> {
    if frame.progress_q16 == 0
        || frame.phase == NavigationTransitionPhase::Settled
        || (request.direction == NavigationTransitionDirection::Forward
            && frame.reveal_progress_q16 >= 62_000)
    {
        return Ok(());
    }
    let source = buffers
        .source
        .get(..)
        .filter(|_| buffers.source_ready)
        .ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
    let destination = buffers
        .destination
        .get(..)
        .filter(|_| buffers.destination_ready);
    let working = buffers.working.as_mut_slice();
    let width = buffers.width;
    let height = buffers.height;
    match request.direction {
        NavigationTransitionDirection::Forward => {
            if frame.reveal_progress_q16 > 0 {
                let destination = destination.unwrap_or(source);
                if frame.reveal_progress_q16 >= 18_000 {
                    copy_rect_565(
                        working,
                        destination,
                        width,
                        height,
                        request.geometry.destination_title,
                        stats,
                    );
                } else {
                    erase_rect_from_snapshot_background(
                        working,
                        destination,
                        width,
                        height,
                        request.geometry.destination_title,
                        stats,
                    );
                    crossfade_labels(
                        working,
                        source,
                        destination,
                        width,
                        height,
                        request.geometry.source_label,
                        request.geometry.destination_title,
                        PROGRESS_MAX,
                        spring_ease_q16(window_q16(frame.reveal_progress_q16, 2_000, 18_000)),
                        false,
                        stats,
                    );
                }
                draw_destination_detail_wake(
                    working,
                    destination,
                    width,
                    height,
                    request.geometry.destination_title,
                    request.geometry.destination_detail,
                    frame.reveal_progress_q16,
                    stats,
                );
            } else {
                move_label_between_rects(
                    working,
                    source,
                    width,
                    height,
                    request.geometry.source_label,
                    request.geometry.destination_title,
                    spring_ease_q16(window_q16(frame.cover_progress_q16, 3_500, 60_000)),
                    false,
                    stats,
                );
                draw_detail_pixels_with_opacity(
                    working,
                    source,
                    width,
                    height,
                    request.geometry.source_detail,
                    PROGRESS_MAX.saturating_sub(spring_ease_q16(window_q16(
                        frame.cover_progress_q16,
                        0,
                        10_000,
                    ))),
                    stats,
                );
            }
        }
        NavigationTransitionDirection::Reverse => {
            if frame.reveal_progress_q16 > 0 {
                let destination = destination.unwrap_or(source);
                let forward_cover = PROGRESS_MAX.saturating_sub(frame.reveal_progress_q16);
                move_label_between_rects(
                    working,
                    destination,
                    width,
                    height,
                    request.geometry.source_label,
                    request.geometry.destination_title,
                    spring_ease_q16(window_q16(forward_cover, 3_500, 60_000)),
                    false,
                    stats,
                );
                draw_detail_pixels_with_opacity(
                    working,
                    destination,
                    width,
                    height,
                    request.geometry.source_detail,
                    PROGRESS_MAX.saturating_sub(spring_ease_q16(window_q16(
                        forward_cover,
                        0,
                        10_000,
                    ))),
                    stats,
                );
            } else {
                let destination = destination.unwrap_or(source);
                let forward_reveal = reverse_destination_reveal_progress(frame.cover_progress_q16);
                if forward_reveal >= 18_000 {
                    copy_rect_565(
                        working,
                        source,
                        width,
                        height,
                        request.geometry.destination_title,
                        stats,
                    );
                } else {
                    erase_rect_from_snapshot_background(
                        working,
                        source,
                        width,
                        height,
                        request.geometry.destination_title,
                        stats,
                    );
                    crossfade_labels(
                        working,
                        destination,
                        source,
                        width,
                        height,
                        request.geometry.source_label,
                        request.geometry.destination_title,
                        PROGRESS_MAX,
                        spring_ease_q16(window_q16(forward_reveal, 2_000, 18_000)),
                        false,
                        stats,
                    );
                }
                draw_destination_detail_wake(
                    working,
                    source,
                    width,
                    height,
                    request.geometry.destination_title,
                    request.geometry.destination_detail,
                    forward_reveal,
                    stats,
                );
            }
        }
    }
    Ok(())
}

fn reveal_destination_regions(
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    progress_q16: u16,
    request: NavigationTransitionRequest,
    stats: &mut NavigationTransitionRenderStats,
) {
    if working.len() != destination.len()
        || working.len() != width.saturating_mul(height)
        || width == 0
        || height == 0
    {
        return;
    }
    if progress_q16 >= 62_000 {
        working.copy_from_slice(destination);
        stats.copied_pixels = stats.copied_pixels.saturating_add(destination.len() as u64);
        return;
    }
    let header_height = if request.edge.enters_system_browser() {
        request.geometry.destination_list.y as usize
    } else {
        height.saturating_mul(15) / 100
    };
    let header_progress = spring_ease_q16(window_q16(progress_q16, 0, 10_000));
    copy_rect_horizontal_wipe(
        working,
        destination,
        width,
        height,
        NavigationTransitionRect {
            x: 0,
            y: 0,
            width: width as u16,
            height: header_height as u16,
        },
        header_progress,
        request.geometry.destination_title.x as usize,
        stats,
    );

    if request.edge.enters_system_browser() {
        let list = request.geometry.destination_list;
        let selected = request.geometry.destination_selected_row;
        let row_height = selected.height as usize;
        copy_rect_shifted_x(
            working,
            destination,
            width,
            height,
            selected,
            spring_ease_q16(window_q16(
                progress_q16,
                SYSTEM_SELECTED_ROW_START_Q16,
                SYSTEM_SELECTED_ROW_START_Q16.saturating_add(SYSTEM_ROW_TRAVEL_Q16),
            )),
            -(selected.right() as isize + SYSTEM_ROW_OFFSCREEN_MARGIN),
            stats,
        );
        for distance in 1usize..9 {
            let start = SYSTEM_SELECTED_ROW_START_Q16
                .saturating_add((distance as u16).saturating_mul(SYSTEM_ROW_STAGGER_Q16));
            for below in [true, false] {
                let Some(y) = (if below {
                    (selected.y as usize).checked_add(distance.saturating_mul(row_height))
                } else {
                    (selected.y as usize).checked_sub(distance.saturating_mul(row_height))
                }) else {
                    continue;
                };
                if y < list.y as usize || y >= list.bottom() as usize {
                    continue;
                }
                let row = NavigationTransitionRect {
                    x: list.x,
                    y: y as u16,
                    width: list.width,
                    height: row_height
                        .min(list.bottom() as usize - y)
                        .min(height.saturating_sub(y)) as u16,
                };
                copy_rect_shifted_x(
                    working,
                    destination,
                    width,
                    height,
                    row,
                    spring_ease_q16(window_q16(
                        progress_q16,
                        start,
                        start.saturating_add(SYSTEM_ROW_TRAVEL_Q16),
                    )),
                    -(row.right() as isize
                        + SYSTEM_ROW_OFFSCREEN_MARGIN
                        + distance as isize * SYSTEM_ROW_DISTANCE_MARGIN),
                    stats,
                );
            }
        }
        copy_rect_shifted_x(
            working,
            destination,
            width,
            height,
            request.geometry.destination_footer,
            spring_ease_q16(window_q16(progress_q16, 28_000, 44_000)),
            -(request.geometry.destination_footer.right() as isize),
            stats,
        );
        draw_runway_selected_row_bridge(
            working,
            width,
            height,
            selected,
            progress_q16,
            false,
            stats,
        );
        let preview = request.geometry.destination_preview;
        if preview.width > 0 && preview.height > 0 {
            draw_preview_transfer_beam(
                working,
                width,
                height,
                selected,
                preview,
                progress_q16,
                stats,
            );
            copy_rect_preview_aperture(
                working,
                destination,
                width,
                height,
                preview,
                spring_ease_q16(window_q16(progress_q16, 34_000, 60_000)),
                stats,
            );
            draw_preview_impact_flash(working, width, height, preview, progress_q16, stats);
            draw_preview_aperture_glow(
                working,
                width,
                height,
                preview,
                spring_ease_q16(window_q16(progress_q16, 34_000, 60_000)),
                stats,
            );
            draw_progressive_preview_frame(
                working,
                width,
                height,
                preview,
                preview_rail_envelope(progress_q16),
                stats,
            );
        }
    } else {
        let card_y = header_height;
        let source_center = request.geometry.source_card.x as usize
            + request.geometry.source_card.width as usize / 2;
        let selected_column = (source_center.saturating_mul(4) / width).min(3);
        for distance in 0..4 {
            for column in 0usize..4 {
                if column.abs_diff(selected_column) != distance {
                    continue;
                }
                let x0 = column * width / 4;
                let x1 = (column + 1) * width / 4;
                let start = 9_000u16.saturating_add((distance * 5_500) as u16);
                copy_rect_vertical_wipe(
                    working,
                    destination,
                    width,
                    height,
                    NavigationTransitionRect {
                        x: x0 as u16,
                        y: card_y as u16,
                        width: x1.saturating_sub(x0) as u16,
                        height: height.saturating_sub(card_y) as u16,
                    },
                    spring_ease_q16(window_q16(
                        progress_q16,
                        start,
                        start.saturating_add(31_000),
                    )),
                    false,
                    stats,
                );
            }
        }
    }
}

fn super_scaler_card_rect(
    source: NavigationTransitionRect,
    full: NavigationTransitionRect,
    progress_q16: u16,
) -> NavigationTransitionRect {
    const PRESS_END: u16 = 14_000;
    if progress_q16 < PRESS_END {
        let half = PRESS_END / 2;
        let press = if progress_q16 <= half {
            spring_ease_q16(window_q16(progress_q16, 0, half))
        } else {
            PROGRESS_MAX.saturating_sub(spring_ease_q16(window_q16(progress_q16, half, PRESS_END)))
        };
        let maximum_inset_x = (source.width / 28).clamp(4, 10);
        let maximum_inset_y = (source.height / 18).clamp(7, 28);
        let inset_x = (maximum_inset_x as u32 * press as u32 / PROGRESS_MAX as u32) as u16;
        let inset_y = (maximum_inset_y as u32 * press as u32 / PROGRESS_MAX as u32) as u16;
        return NavigationTransitionRect {
            x: source.x.saturating_add(inset_x),
            y: source.y.saturating_add(inset_y),
            width: source.width.saturating_sub(inset_x.saturating_mul(2)),
            height: source.height.saturating_sub(inset_y.saturating_mul(2)),
        };
    }
    let source_center = source.x as usize + source.width as usize / 2;
    let full_center = full.width as usize / 2;
    let selected_left = source_center <= full_center;
    let launch_end = 52_000;
    let settle_end = PROGRESS_MAX;
    let left_end = if selected_left {
        settle_end
    } else {
        launch_end
    };
    let right_end = if selected_left {
        launch_end
    } else {
        settle_end
    };
    let left_motion = spring_ease_q16(window_q16(progress_q16, PRESS_END, left_end));
    let right_motion = spring_ease_q16(window_q16(progress_q16, PRESS_END, right_end));
    let left = lerp_u16(source.x, full.x, left_motion);
    let right = lerp_u16(source.right(), full.right(), right_motion);
    let top = lerp_u16(
        source.y,
        full.y,
        spring_ease_q16(window_q16(progress_q16, PRESS_END, PROGRESS_MAX)),
    );
    let bottom = lerp_u16(
        source.bottom(),
        full.bottom(),
        spring_ease_q16(window_q16(progress_q16, PRESS_END, 54_000)),
    );
    NavigationTransitionRect {
        x: left,
        y: top,
        width: right.saturating_sub(left).max(1),
        height: bottom.saturating_sub(top).max(1),
    }
}

fn super_scaler_shell_row_color(y: usize, height: usize, shell: Rgb565Pixel) -> Rgb565Pixel {
    if height == 0 {
        return shell;
    }
    let center = height / 2;
    for band in 1usize..=3 {
        let target = height.saturating_mul(band) / 8;
        let color = match band {
            1 => Rgb565Pixel(0x28aa),
            2 => Rgb565Pixel(0x40ed),
            _ => Rgb565Pixel(0x79b8),
        };
        for band_y in [center.saturating_sub(target), center.saturating_add(target)] {
            if y >= band_y && y < band_y.saturating_add(band + 1) {
                return color;
            }
        }
    }
    shell
}

fn draw_super_scaler_speed_bands(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    if progress_q16 < 8_000 {
        return;
    }
    let local = spring_ease_q16(window_q16(progress_q16, 8_000, PROGRESS_MAX));
    let center_y = rect.y as usize + rect.height as usize / 2;
    let half_height = rect.height as usize / 2;
    for band in 1usize..=3 {
        let distance = half_height
            .saturating_mul(band)
            .saturating_mul(local as usize)
            / 4
            / PROGRESS_MAX as usize;
        let inset = rect.width as usize
            * (PROGRESS_MAX.saturating_sub(local) as usize)
            * (4usize.saturating_sub(band))
            / PROGRESS_MAX as usize
            / 10;
        let x = rect.x as usize + inset.min(rect.width as usize / 2);
        let band_width = (rect.width as usize).saturating_sub(inset.saturating_mul(2));
        let color = match band {
            1 => Rgb565Pixel(0x28aa),
            2 => Rgb565Pixel(0x40ed),
            _ => Rgb565Pixel(0x79b8),
        };
        for y in [
            center_y.saturating_sub(distance),
            center_y.saturating_add(distance),
        ] {
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: x as u16,
                    y: y.min(height.saturating_sub(1)) as u16,
                    width: band_width.max(1) as u16,
                    height: (band + 1) as u16,
                },
                color,
                stats,
            );
        }
    }
}

fn draw_super_scaler_impact_horizon(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if progress_q16 < 42_000 {
        return;
    }
    let pulse = spring_ease_q16(window_q16(progress_q16, 42_000, 52_000));
    if pulse == 0 {
        return;
    }
    let visible_width = (rect.width as usize)
        .saturating_mul(pulse as usize)
        .div_ceil(PROGRESS_MAX as usize)
        .max(1);
    let x = rect.x as usize + (rect.width as usize).saturating_sub(visible_width) / 2;
    fill_rect_565(
        working,
        width,
        height,
        NavigationTransitionRect {
            x: x as u16,
            y: rect.y.saturating_add(rect.height / 2).saturating_sub(1),
            width: visible_width as u16,
            height: if pulse > 45_000 {
                5
            } else if pulse > 24_000 {
                3
            } else {
                1
            },
        },
        if pulse > 32_000 {
            Rgb565Pixel(0x07ff)
        } else {
            Rgb565Pixel(0x79b8)
        },
        stats,
    );
}

fn draw_super_scaler_impact_envelope(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let pulse = if progress_q16 < 50_000 {
        0
    } else if progress_q16 < 54_000 {
        spring_ease_q16(window_q16(progress_q16, 50_000, 54_000))
    } else if progress_q16 <= 58_000 {
        PROGRESS_MAX
    } else {
        PROGRESS_MAX.saturating_sub(spring_ease_q16(window_q16(progress_q16, 58_000, 64_000)))
    };
    if pulse == 0 {
        return;
    }
    let cyan_layers =
        1usize.saturating_add(2usize.saturating_mul(pulse as usize) / PROGRESS_MAX as usize);
    let violet_layers = 1usize.saturating_add(pulse as usize / 40_000);
    for inset in 0..cyan_layers {
        if let Some(outline) = inset_rect(rect, inset as u16) {
            draw_outline_565(working, width, height, outline, Rgb565Pixel(0x07ff), stats);
        }
    }
    for inset in cyan_layers..cyan_layers.saturating_add(violet_layers) {
        if let Some(outline) = inset_rect(rect, inset as u16) {
            draw_outline_565(working, width, height, outline, Rgb565Pixel(0x79b8), stats);
        }
    }
}

fn inset_rect(rect: NavigationTransitionRect, inset: u16) -> Option<NavigationTransitionRect> {
    let doubled = inset.saturating_mul(2);
    (rect.width > doubled && rect.height > doubled).then_some(NavigationTransitionRect {
        x: rect.x.saturating_add(inset),
        y: rect.y.saturating_add(inset),
        width: rect.width.saturating_sub(doubled),
        height: rect.height.saturating_sub(doubled),
    })
}

fn draw_runway_selected_row_bridge(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    selected: NavigationTransitionRect,
    progress_q16: u16,
    reverse: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    if !reverse && progress_q16 > 30_000 {
        return;
    }
    let motion = spring_ease_q16(window_q16(progress_q16, 0, 18_000));
    let horizon = NavigationTransitionRect {
        x: 0,
        y: (height / 2).saturating_sub(1) as u16,
        width: width.min(u16::MAX as usize) as u16,
        height: 3,
    };
    let selected_border = NavigationTransitionRect {
        x: selected.x,
        y: selected.y,
        width: selected.width,
        height: 2,
    };
    let bridge = if reverse {
        lerp_rect(selected_border, horizon, motion)
    } else {
        lerp_rect(horizon, selected_border, motion)
    };
    fill_rect_565(working, width, height, bridge, Rgb565Pixel(0x07ff), stats);
}

fn fill_super_scaler_covered_surface(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    shell: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    fill_super_scaler_shell_rows(working, width, height, rect, shell, stats);
    draw_super_scaler_covered_horizon(working, width, height, rect, stats);
}

fn draw_super_scaler_covered_horizon(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    let y = (height / 2).saturating_sub(1);
    if y < rect.y as usize || y >= rect.bottom() as usize {
        return;
    }
    fill_rect_565(
        working,
        width,
        height,
        NavigationTransitionRect {
            x: rect.x,
            y: y as u16,
            width: rect.width,
            height: 3.min(rect.bottom().saturating_sub(y as u16)),
        },
        Rgb565Pixel(0x07ff),
        stats,
    );
}

fn fill_super_scaler_shell_rows(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    shell: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    if working.len() != width.saturating_mul(height) {
        return;
    }
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    for y in rect.y as usize..rect.bottom() as usize {
        let start = y * width + rect.x as usize;
        let end = start + rect.width as usize;
        working[start..end].fill(super_scaler_shell_row_color(y, height, shell));
        stats.filled_pixels = stats.filled_pixels.saturating_add(rect.width as u64);
    }
}

fn window_q16(progress_q16: u16, start_q16: u16, end_q16: u16) -> u16 {
    if progress_q16 <= start_q16 {
        return 0;
    }
    if progress_q16 >= end_q16 {
        return PROGRESS_MAX;
    }
    ((progress_q16 - start_q16) as u32 * PROGRESS_MAX as u32
        / end_q16.saturating_sub(start_q16).max(1) as u32) as u16
}

fn spring_ease_q16(progress_q16: u16) -> u16 {
    smooth_spring_q16(progress_q16)
}

fn smooth_spring_q16(progress_q16: u16) -> u16 {
    let curve = SMOOTH_CURVE_Q16.get_or_init(build_smooth_curve_q16);
    if progress_q16 == u16::MAX {
        return u16::MAX;
    }
    let scaled = progress_q16 as u32 * SMOOTH_CURVE_INTERVALS as u32;
    let index = (scaled / u16::MAX as u32) as usize;
    let remainder = scaled % u16::MAX as u32;
    let from = curve[index] as u32;
    let to = curve[index + 1] as u32;
    (from + (to - from) * remainder / u16::MAX as u32) as u16
}

fn build_smooth_curve_q16() -> [u16; SMOOTH_CURVE_INTERVALS + 1] {
    let mut raw = [0.0; SMOOTH_CURVE_INTERVALS + 1];
    let omega = std::f64::consts::TAU / Duration::from_millis(500).as_secs_f64();
    for (index, value) in raw.iter_mut().enumerate() {
        let time =
            Duration::from_micros(500_000_u64 * index as u64 / SMOOTH_CURVE_INTERVALS as u64)
                .as_secs_f64();
        let y = -1.0;
        let velocity = 0.0;
        let b = velocity + omega * y;
        let decay = (-omega * time).exp();
        *value = 1.0 + (y + b * time) * decay;
    }
    let final_value = raw[SMOOTH_CURVE_INTERVALS];
    let mut curve = [0_u16; SMOOTH_CURVE_INTERVALS + 1];
    for (index, value) in raw.into_iter().enumerate() {
        curve[index] = ((value / final_value) * u16::MAX as f64)
            .round()
            .clamp(0.0, u16::MAX as f64) as u16;
    }
    curve[0] = 0;
    curve[SMOOTH_CURVE_INTERVALS] = u16::MAX;
    curve
}

fn background_outside_rect(
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
) -> Rgb565Pixel {
    if source.len() != width.saturating_mul(height) || width == 0 || height == 0 {
        return Rgb565Pixel(0);
    }
    let x = (rect.x as usize).saturating_sub(2).min(width - 1);
    let y = (rect.y as usize + rect.height as usize / 2).min(height - 1);
    source[y * width + x]
}

#[allow(clippy::too_many_arguments)]
fn blit_scaled_card_565(
    destination: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    source_rect: NavigationTransitionRect,
    target_rect: NavigationTransitionRect,
    excluded_source_rect: NavigationTransitionRect,
    texture_q16: u16,
    scale_source_x: &mut [usize],
    scale_source_y: &mut [usize],
    scale_excluded_x: &mut [bool],
    scale_dither_x: &mut [bool],
    stats: &mut NavigationTransitionRenderStats,
) {
    if source.len() != width.saturating_mul(height)
        || destination.len() != source.len()
        || !source_rect.fits(width, height)
        || source_rect.width == 0
        || source_rect.height == 0
        || !target_rect.fits(width, height)
        || texture_q16 == 0
        || scale_source_x.len() < target_rect.width as usize
        || scale_source_y.len() < target_rect.height as usize
        || scale_excluded_x.len() < target_rect.width as usize
        || scale_dither_x.len() < target_rect.width as usize * 4
    {
        return;
    }
    let source_width = source_rect.width as usize;
    let source_height = source_rect.height as usize;
    let target_width = target_rect.width as usize;
    let target_height = target_rect.height as usize;
    const DITHER: [[u16; 4]; 4] = [
        [0, 32_768, 8_192, 40_960],
        [49_152, 16_384, 57_344, 24_576],
        [12_288, 45_056, 4_096, 36_864],
        [61_440, 28_672, 53_248, 20_480],
    ];
    let x_step_q16 = ((source_width as u64) << 16) / target_width as u64;
    let mut source_x_q16 = 0u64;
    for target_x in 0..target_width {
        let source_x = source_rect.x as usize + (source_x_q16 >> 16) as usize;
        source_x_q16 = source_x_q16.saturating_add(x_step_q16);
        scale_source_x[target_x] = source_x;
        scale_excluded_x[target_x] = source_x >= excluded_source_rect.x as usize
            && source_x < excluded_source_rect.right() as usize;
        let x = target_rect.x as usize + target_x;
        for y_phase in 0..4 {
            scale_dither_x[y_phase * target_width + target_x] =
                DITHER[y_phase][x & 3] < texture_q16;
        }
    }
    let y_step_q16 = ((source_height as u64) << 16) / target_height as u64;
    let mut source_y_q16 = 0u64;
    for source_y in &mut scale_source_y[..target_height] {
        *source_y = source_rect.y as usize + (source_y_q16 >> 16) as usize;
        source_y_q16 = source_y_q16.saturating_add(y_step_q16);
    }

    for (target_y, &source_y) in scale_source_y[..target_height].iter().enumerate() {
        let y = target_rect.y as usize + target_y;
        let destination_row = y * width + target_rect.x as usize;
        let source_row = source_y * width;
        let excluded_y = source_y >= excluded_source_rect.y as usize
            && source_y < excluded_source_rect.bottom() as usize;
        let dither = &scale_dither_x[(y & 3) * target_width..][..target_width];
        for target_x in 0..target_width {
            if dither[target_x] && !(excluded_y && scale_excluded_x[target_x]) {
                destination[destination_row + target_x] =
                    source[source_row + scale_source_x[target_x]];
            }
        }
    }
    stats.copied_pixels = stats
        .copied_pixels
        .saturating_add(target_width.saturating_mul(target_height) as u64);
}

#[allow(clippy::too_many_arguments)]
fn copy_rect_horizontal_wipe(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    anchor_x: usize,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    if progress_q16 == 0 {
        return;
    }
    let visible_width = rect.width as usize * progress_q16 as usize / PROGRESS_MAX as usize;
    if visible_width == 0 {
        return;
    }
    let minimum_x = rect.x as usize;
    let maximum_x = rect.right() as usize;
    let maximum_start = maximum_x.saturating_sub(visible_width).max(minimum_x);
    let x0 = anchor_x
        .saturating_sub(visible_width / 2)
        .clamp(minimum_x, maximum_start);
    copy_rect_565(
        working,
        source,
        width,
        height,
        NavigationTransitionRect {
            x: x0 as u16,
            y: rect.y,
            width: visible_width as u16,
            height: rect.height,
        },
        stats,
    );
}

#[allow(clippy::too_many_arguments)]
fn copy_rect_vertical_wipe(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    from_top: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    if progress_q16 == 0 {
        return;
    }
    let visible_height = rect.height as usize * progress_q16 as usize / PROGRESS_MAX as usize;
    if visible_height == 0 {
        return;
    }
    let y = if from_top {
        rect.y as usize
    } else {
        rect.bottom() as usize - visible_height
    };
    copy_rect_565(
        working,
        source,
        width,
        height,
        NavigationTransitionRect {
            x: rect.x,
            y: y as u16,
            width: rect.width,
            height: visible_height as u16,
        },
        stats,
    );
}

#[allow(clippy::too_many_arguments)]
fn copy_rect_shifted_x(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    initial_offset: isize,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    if progress_q16 == 0
        || working.len() != source.len()
        || working.len() != width.saturating_mul(height)
    {
        return;
    }
    let remaining = PROGRESS_MAX.saturating_sub(progress_q16) as i64;
    let offset = initial_offset as i64 * remaining / PROGRESS_MAX as i64;
    copy_rect_at_offset(working, source, width, height, rect, offset as isize, stats);
}

#[allow(clippy::too_many_arguments)]
fn copy_rect_at_offset(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    offset: isize,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    if working.len() != source.len() || working.len() != width.saturating_mul(height) {
        return;
    }
    let destination_x = rect.x as i64 + offset as i64;
    let unclipped_x0 = destination_x;
    let unclipped_x1 = destination_x + rect.width as i64;
    let destination_x0 = unclipped_x0.max(0).min(width as i64) as usize;
    let destination_x1 = unclipped_x1.max(0).min(width as i64) as usize;
    if destination_x1 <= destination_x0 {
        return;
    }
    let source_x0 = rect.x as usize + (destination_x0 as i64 - unclipped_x0) as usize;
    let copy_width = destination_x1 - destination_x0;
    for y in rect.y as usize..rect.bottom() as usize {
        let destination_start = y * width + destination_x0;
        let source_start = y * width + source_x0;
        working[destination_start..destination_start + copy_width]
            .copy_from_slice(&source[source_start..source_start + copy_width]);
        stats.copied_pixels = stats.copied_pixels.saturating_add(copy_width as u64);
    }
}

#[allow(clippy::too_many_arguments)]
fn copy_rect_preview_aperture(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    let Some(aperture) = preview_aperture_rect(rect, progress_q16) else {
        return;
    };
    copy_rect_565(working, source, width, height, aperture, stats);
}

fn preview_aperture_rect(
    rect: NavigationTransitionRect,
    progress_q16: u16,
) -> Option<NavigationTransitionRect> {
    if progress_q16 == 0 || rect.width == 0 || rect.height == 0 {
        return None;
    }
    let horizontal = spring_ease_q16(window_q16(progress_q16, 0, 16_000))
        .max(spring_ease_q16(progress_q16).min(2_048));
    let vertical = spring_ease_q16(window_q16(progress_q16, 22_000, PROGRESS_MAX));
    let visible_width = (rect.width as usize)
        .saturating_mul(horizontal as usize)
        .div_ceil(PROGRESS_MAX as usize)
        .max(1);
    let visible_height = 1usize.saturating_add(
        (rect.height as usize)
            .saturating_sub(1)
            .saturating_mul(vertical as usize)
            / PROGRESS_MAX as usize,
    );
    Some(NavigationTransitionRect {
        x: rect
            .x
            .saturating_add((rect.width as usize - visible_width).div_ceil(2) as u16),
        y: rect
            .y
            .saturating_add((rect.height as usize - visible_height).div_ceil(2) as u16),
        width: visible_width as u16,
        height: visible_height as u16,
    })
}

fn draw_preview_aperture_glow(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if progress_q16 == 0 || progress_q16 >= 60_000 {
        return;
    }
    let Some(aperture) = preview_aperture_rect(rect, progress_q16) else {
        return;
    };
    draw_outline_565(working, width, height, aperture, Rgb565Pixel(0x07ff), stats);
}

fn draw_preview_impact_flash(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if !(34_000..47_000).contains(&progress_q16) {
        return;
    }
    let pulse = if progress_q16 <= 39_000 {
        spring_ease_q16(window_q16(progress_q16, 34_000, 39_000))
    } else {
        PROGRESS_MAX.saturating_sub(spring_ease_q16(window_q16(progress_q16, 39_000, 47_000)))
    };
    let flash_width = (rect.width as usize)
        .saturating_mul(pulse as usize)
        .div_ceil(PROGRESS_MAX as usize)
        .max(1);
    fill_rect_565(
        working,
        width,
        height,
        NavigationTransitionRect {
            x: rect
                .x
                .saturating_add((rect.width as usize - flash_width).div_ceil(2) as u16),
            y: rect.y.saturating_add(rect.height / 2).saturating_sub(1),
            width: flash_width as u16,
            height: if pulse > 32_000 {
                5
            } else if pulse > 16_000 {
                3
            } else {
                2
            },
        },
        Rgb565Pixel(0x07ff),
        stats,
    );
}

fn draw_progressive_preview_frame(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    let horizontal = rect.width as usize * progress_q16 as usize / PROGRESS_MAX as usize;
    let vertical = rect.height as usize * progress_q16 as usize / PROGRESS_MAX as usize;
    if horizontal == 0 && vertical == 0 {
        return;
    }
    let x = rect.x as usize + (rect.width as usize - horizontal) / 2;
    let y = rect.y as usize + (rect.height as usize - vertical) / 2;
    let violet = Rgb565Pixel(0x79b8);
    let mint = Rgb565Pixel(0x07d6);
    for line_y in [rect.y, rect.bottom().saturating_sub(1)] {
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: x as u16,
                y: line_y,
                width: horizontal as u16,
                height: 1,
            },
            violet,
            stats,
        );
    }
    for line_x in [rect.x, rect.right().saturating_sub(1)] {
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: line_x,
                y: y as u16,
                width: 1,
                height: vertical as u16,
            },
            mint,
            stats,
        );
    }
}

fn preview_rail_envelope(progress_q16: u16) -> u16 {
    if progress_q16 <= 42_000 {
        spring_ease_q16(window_q16(progress_q16, 34_000, 42_000))
    } else if progress_q16 <= 48_000 {
        PROGRESS_MAX
    } else {
        PROGRESS_MAX.saturating_sub(spring_ease_q16(window_q16(progress_q16, 48_000, 58_000)))
    }
}

#[cfg(test)]
fn reverse_preview_timeline(progress_q16: u16) -> u16 {
    if progress_q16 >= 25_000 {
        return 34_000;
    }
    62_000u32
        .saturating_sub(progress_q16 as u32 * 28_000 / 25_000)
        .min(PROGRESS_MAX as u32) as u16
}

#[allow(clippy::too_many_arguments)]
fn draw_preview_transfer_beam(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    selected: NavigationTransitionRect,
    preview: NavigationTransitionRect,
    progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if progress_q16 <= 24_000 || progress_q16 >= 49_000 {
        return;
    }
    let reveal = spring_ease_q16(window_q16(progress_q16, 24_000, 34_000));
    let retract = spring_ease_q16(window_q16(progress_q16, 39_000, 49_000));
    let start_x = selected.right() as usize;
    let end_x = preview.x as usize;
    if end_x <= start_x {
        return;
    }
    let start_y = selected.y as usize + selected.height as usize / 2;
    let end_y = preview.y as usize + preview.height as usize / 2;
    const SEGMENTS: usize = 12;
    let revealed_segments = SEGMENTS * reveal as usize / PROGRESS_MAX as usize;
    let retracted_segments = SEGMENTS * retract as usize / PROGRESS_MAX as usize;
    for segment in retracted_segments..revealed_segments {
        let x = start_x + (end_x - start_x) * segment / SEGMENTS;
        let y = if end_y >= start_y {
            start_y + (end_y - start_y) * segment / SEGMENTS
        } else {
            start_y.saturating_sub((start_y - end_y) * segment / SEGMENTS)
        };
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: x as u16,
                y: y.min(height.saturating_sub(1)) as u16,
                width: ((end_x - start_x) / SEGMENTS).saturating_add(2).max(3) as u16,
                height: if segment + 1 == revealed_segments {
                    5
                } else {
                    3
                },
            },
            if segment & 1 == 0 {
                Rgb565Pixel(0x07ff)
            } else {
                Rgb565Pixel(0x79b8)
            },
            stats,
        );
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn draw_reverse_preview_transfer_beam(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    selected: NavigationTransitionRect,
    preview: NavigationTransitionRect,
    progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if progress_q16 >= 22_000 {
        return;
    }
    let reverse = spring_ease_q16(window_q16(progress_q16, 0, 22_000)) as u32;
    let forward_progress =
        46_000u32.saturating_sub(reverse.saturating_mul(22_000) / PROGRESS_MAX as u32);
    draw_preview_transfer_beam(
        working,
        width,
        height,
        selected,
        preview,
        forward_progress as u16,
        stats,
    );
}

fn clip_rect_to_frame(
    rect: NavigationTransitionRect,
    width: usize,
    height: usize,
) -> Option<NavigationTransitionRect> {
    let x0 = (rect.x as usize).min(width);
    let y0 = (rect.y as usize).min(height);
    let x1 = (rect.right() as usize).min(width);
    let y1 = (rect.bottom() as usize).min(height);
    (x1 > x0 && y1 > y0).then_some(NavigationTransitionRect {
        x: x0 as u16,
        y: y0 as u16,
        width: x1.saturating_sub(x0) as u16,
        height: y1.saturating_sub(y0) as u16,
    })
}

fn copy_rect_565(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    stats: &mut NavigationTransitionRenderStats,
) {
    if working.len() != source.len() || working.len() != width.saturating_mul(height) {
        return;
    }
    let x0 = (rect.x as usize).min(width);
    let x1 = (rect.right() as usize).min(width);
    let y0 = (rect.y as usize).min(height);
    let y1 = (rect.bottom() as usize).min(height);
    for y in y0..y1 {
        let start = y * width + x0;
        let end = y * width + x1;
        working[start..end].copy_from_slice(&source[start..end]);
        stats.copied_pixels = stats
            .copied_pixels
            .saturating_add(end.saturating_sub(start) as u64);
    }
}

#[allow(clippy::too_many_arguments)]
fn compose_system_background_horizon(
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    progress_q16: u16,
    anchor_y: usize,
    shell: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    if working.len() != destination.len()
        || working.len() != width.saturating_mul(height)
        || width == 0
        || height == 0
    {
        return;
    }
    let anchor_y = anchor_y.min(height.saturating_sub(1));
    let maximum_distance = anchor_y.max(height.saturating_sub(1).saturating_sub(anchor_y));
    let revealed_distance =
        maximum_distance.saturating_mul(progress_q16 as usize) / PROGRESS_MAX as usize;
    let split_x = width / 2;
    let sample_inset = (width / 240).clamp(1, 4).min(width.saturating_sub(1));
    for y in 0..height {
        let row_start = y * width;
        if progress_q16 == 0 || y.abs_diff(anchor_y) > revealed_distance {
            working[row_start..row_start + width]
                .fill(super_scaler_shell_row_color(y, height, shell));
            stats.filled_pixels = stats.filled_pixels.saturating_add(width as u64);
            continue;
        }
        let left_color = destination[y * width + sample_inset];
        let right_color = destination[y * width + width - 1 - sample_inset];
        working[row_start..row_start + split_x].fill(left_color);
        working[row_start + split_x..row_start + width].fill(right_color);
        stats.filled_pixels = stats.filled_pixels.saturating_add(width as u64);
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn conceal_system_background_horizon(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    progress_q16: u16,
    anchor_y: usize,
    shell: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    if working.len() != width.saturating_mul(height)
        || width == 0
        || height == 0
        || progress_q16 == 0
    {
        return;
    }
    let anchor_y = anchor_y.min(height.saturating_sub(1));
    let maximum_distance = anchor_y.max(height.saturating_sub(1).saturating_sub(anchor_y));
    let destination_distance = maximum_distance
        .saturating_mul(PROGRESS_MAX.saturating_sub(progress_q16) as usize)
        / PROGRESS_MAX as usize;
    for y in 0..height {
        if y.abs_diff(anchor_y) <= destination_distance {
            continue;
        }
        let row_start = y * width;
        working[row_start..row_start + width].fill(super_scaler_shell_row_color(y, height, shell));
        stats.filled_pixels = stats.filled_pixels.saturating_add(width as u64);
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn slide_rect_out_left(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    shell: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    if working.len() != source.len() || working.len() != width.saturating_mul(height) {
        return;
    }
    if progress_q16 == 0 {
        return;
    }
    fill_rect_565(working, width, height, rect, shell, stats);
    let exit_distance = rect.right() as usize;
    let displacement = exit_distance * progress_q16 as usize / PROGRESS_MAX as usize;
    if displacement >= exit_distance {
        return;
    }
    let unclipped_destination_x = rect.x as isize - displacement as isize;
    let destination_x = unclipped_destination_x.max(0) as usize;
    let clipped_left = destination_x as isize - unclipped_destination_x;
    let source_x = rect.x as usize + clipped_left as usize;
    let remaining_width =
        (rect.width as usize - clipped_left as usize).min(width.saturating_sub(destination_x));
    if remaining_width == 0 {
        return;
    }
    for y in rect.y as usize..rect.bottom() as usize {
        let destination_start = y * width + destination_x;
        let source_start = y * width + source_x;
        working[destination_start..destination_start + remaining_width]
            .copy_from_slice(&source[source_start..source_start + remaining_width]);
        stats.copied_pixels = stats.copied_pixels.saturating_add(remaining_width as u64);
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn close_preview_aperture(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    shell: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    if working.len() != source.len() || working.len() != width.saturating_mul(height) {
        return;
    }
    if progress_q16 == 0 {
        return;
    }
    fill_rect_565(working, width, height, rect, shell, stats);
    let remaining = PROGRESS_MAX.saturating_sub(progress_q16);
    copy_rect_preview_aperture(working, source, width, height, rect, remaining, stats);
    draw_preview_aperture_glow(working, width, height, rect, remaining, stats);
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn fill_rect_edge_close(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    color: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    let covered_width = rect.width as usize * progress_q16 as usize / PROGRESS_MAX as usize;
    let left_width = covered_width / 2;
    let right_width = covered_width.saturating_sub(left_width);
    if left_width > 0 {
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: rect.x,
                y: rect.y,
                width: left_width as u16,
                height: rect.height,
            },
            color,
            stats,
        );
    }
    if right_width > 0 {
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: rect.right().saturating_sub(right_width as u16),
                y: rect.y,
                width: right_width as u16,
                height: rect.height,
            },
            color,
            stats,
        );
    }
}

fn reverse_destination_reveal_progress(reverse_progress_q16: u16) -> u16 {
    62_000u16.saturating_sub(reverse_progress_q16)
}

#[allow(clippy::too_many_arguments)]
fn conceal_source_regions_inverse(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    progress_q16: u16,
    request: NavigationTransitionRequest,
    shell: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    if width == 0
        || height == 0
        || working.len() != source.len()
        || working.len() != width.saturating_mul(height)
    {
        return;
    }
    let forward_progress = reverse_destination_reveal_progress(progress_q16);
    let full = NavigationTransitionRect {
        x: 0,
        y: 0,
        width: width as u16,
        height: height as u16,
    };
    if request.edge.enters_system_browser() {
        compose_system_background_horizon(
            working,
            source,
            width,
            height,
            spring_ease_q16(window_q16(forward_progress, 0, 18_000)),
            request.geometry.destination_title.bottom() as usize,
            shell,
            stats,
        );
    } else {
        fill_super_scaler_covered_surface(working, width, height, full, shell, stats);
    }
    reveal_destination_regions(
        working,
        source,
        width,
        height,
        forward_progress,
        request,
        stats,
    );
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn conceal_source_regions(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    progress_q16: u16,
    request: NavigationTransitionRequest,
    shell: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    if width == 0
        || height == 0
        || working.len() != source.len()
        || working.len() != width.saturating_mul(height)
    {
        return;
    }
    if progress_q16 == PROGRESS_MAX {
        fill_super_scaler_covered_surface(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: 0,
                y: 0,
                width: width as u16,
                height: height as u16,
            },
            shell,
            stats,
        );
        return;
    }
    if request.edge.enters_system_browser() {
        let list = request.geometry.destination_list;
        let selected = request.geometry.destination_selected_row;
        let row_height = selected.height as usize;
        conceal_system_background_horizon(
            working,
            width,
            height,
            spring_ease_q16(window_q16(progress_q16, 38_000, PROGRESS_MAX)),
            request.geometry.destination_title.bottom() as usize,
            shell,
            stats,
        );
        if request.geometry.destination_preview.width > 0
            && request.geometry.destination_preview.height > 0
        {
            close_preview_aperture(
                working,
                source,
                width,
                height,
                request.geometry.destination_preview,
                spring_ease_q16(window_q16(progress_q16, 0, 25_000)),
                shell,
                stats,
            );
            draw_progressive_preview_frame(
                working,
                width,
                height,
                request.geometry.destination_preview,
                preview_rail_envelope(reverse_preview_timeline(progress_q16)),
                stats,
            );
            draw_reverse_preview_transfer_beam(
                working,
                width,
                height,
                selected,
                request.geometry.destination_preview,
                progress_q16,
                stats,
            );
        }
        slide_rect_out_left(
            working,
            source,
            width,
            height,
            request.geometry.destination_footer,
            spring_ease_q16(window_q16(progress_q16, 20_000, 34_000)),
            shell,
            stats,
        );
        for distance in (1usize..9).rev() {
            let start = 9_000u16.saturating_add(((8usize.saturating_sub(distance)) * 2_600) as u16);
            for below in [true, false] {
                let Some(y) = (if below {
                    (selected.y as usize).checked_add(distance.saturating_mul(row_height))
                } else {
                    (selected.y as usize).checked_sub(distance.saturating_mul(row_height))
                }) else {
                    continue;
                };
                if y < list.y as usize || y >= list.bottom() as usize {
                    continue;
                }
                slide_rect_out_left(
                    working,
                    source,
                    width,
                    height,
                    NavigationTransitionRect {
                        x: list.x,
                        y: y as u16,
                        width: list.width,
                        height: row_height
                            .min(list.bottom() as usize - y)
                            .min(height.saturating_sub(y)) as u16,
                    },
                    spring_ease_q16(window_q16(
                        progress_q16,
                        start,
                        start.saturating_add(13_000),
                    )),
                    shell,
                    stats,
                );
            }
        }
        slide_rect_out_left(
            working,
            source,
            width,
            height,
            selected,
            spring_ease_q16(window_q16(progress_q16, 34_000, 51_000)),
            shell,
            stats,
        );
        draw_runway_selected_row_bridge(
            working,
            width,
            height,
            selected,
            spring_ease_q16(window_q16(progress_q16, 34_000, PROGRESS_MAX)),
            true,
            stats,
        );
        fill_rect_edge_close(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: 0,
                y: 0,
                width: width as u16,
                height: if request.edge.enters_system_browser() {
                    request.geometry.destination_list.y
                } else {
                    (height.saturating_mul(15) / 100) as u16
                },
            },
            spring_ease_q16(window_q16(progress_q16, 42_000, PROGRESS_MAX)),
            shell,
            stats,
        );
    } else {
        let source_center = request.geometry.source_card.x as usize
            + request.geometry.source_card.width as usize / 2;
        let selected_column = (source_center.saturating_mul(4) / width).min(3);
        for column in 0usize..4 {
            let start = (column.abs_diff(selected_column) * 4_000) as u16;
            let local = spring_ease_q16(window_q16(
                progress_q16,
                start,
                48_000u16.saturating_add(start),
            ));
            let x0 = column * width / 4;
            let x1 = (column + 1) * width / 4;
            let covered_height = height * local as usize / PROGRESS_MAX as usize;
            fill_super_scaler_covered_surface(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: x0 as u16,
                    y: height.saturating_sub(covered_height) as u16,
                    width: x1.saturating_sub(x0) as u16,
                    height: covered_height as u16,
                },
                shell,
                stats,
            );
        }
    }
}

fn erase_rect_from_snapshot_background(
    working: &mut [Rgb565Pixel],
    snapshot: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    stats: &mut NavigationTransitionRenderStats,
) {
    if working.len() != width.saturating_mul(height)
        || snapshot.len() != working.len()
        || width == 0
        || height == 0
    {
        return;
    }
    let background = opaque_content_bounds(snapshot, width, height, rect)
        .map(|(_, background)| background)
        .unwrap_or_else(|| {
            let sample_x = (rect.right() as usize + 2).min(width - 1);
            let sample_y = (rect.y as usize + rect.height as usize / 2).min(height - 1);
            snapshot[sample_y * width + sample_x]
        });
    fill_rect_565(working, width, height, rect, background, stats);
}

#[allow(clippy::too_many_arguments)]
fn draw_destination_detail_wake(
    working: &mut [Rgb565Pixel],
    snapshot: &[Rgb565Pixel],
    width: usize,
    height: usize,
    title_rect: NavigationTransitionRect,
    detail_rect: NavigationTransitionRect,
    reveal_progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if detail_rect.width == 0 || detail_rect.height == 0 {
        return;
    }
    erase_rect_from_snapshot_background(working, snapshot, width, height, detail_rect, stats);
    let motion = spring_ease_q16(window_q16(reveal_progress_q16, 18_000, 31_000));
    if motion == 0 {
        return;
    }
    let Some((title_content, _)) = opaque_content_bounds(snapshot, width, height, title_rect)
    else {
        return;
    };
    let Some((detail_content, background)) =
        opaque_content_bounds(snapshot, width, height, detail_rect)
    else {
        return;
    };
    let maximum_x = width.saturating_sub(detail_content.width as usize);
    let maximum_y = height.saturating_sub(detail_content.height as usize);
    let wake = NavigationTransitionRect {
        x: (title_content.right() as usize + 6).min(maximum_x) as u16,
        y: (title_content.y as usize + title_content.height as usize / 2)
            .saturating_sub(detail_content.height as usize / 2)
            .min(maximum_y) as u16,
        width: detail_content.width,
        height: detail_content.height,
    };
    blit_scaled_masked_dithered_565(
        working,
        snapshot,
        width,
        height,
        detail_content,
        lerp_rect(wake, detail_content, motion),
        background,
        motion,
        stats,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_detail_pixels_with_opacity(
    working: &mut [Rgb565Pixel],
    snapshot: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    opacity_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if opacity_q16 == 0 || rect.width == 0 || rect.height == 0 {
        return;
    }
    let Some((content, background)) = opaque_content_bounds(snapshot, width, height, rect) else {
        return;
    };
    blit_scaled_masked_dithered_565(
        working,
        snapshot,
        width,
        height,
        content,
        content,
        background,
        opacity_q16,
        stats,
    );
}

#[allow(clippy::too_many_arguments)]
fn move_label_between_rects(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    from: NavigationTransitionRect,
    to: NavigationTransitionRect,
    progress_q16: u16,
    center_target: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some((content, background)) = opaque_content_bounds(source, width, height, from) else {
        return;
    };
    let target_content = label_target_rect(content, from, to, center_target);
    blit_scaled_masked_565(
        working,
        source,
        width,
        height,
        content,
        lerp_rect(content, target_content, progress_q16),
        background,
        stats,
    );
}

#[allow(clippy::too_many_arguments)]
fn crossfade_labels(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    source_rect: NavigationTransitionRect,
    destination_rect: NavigationTransitionRect,
    motion_q16: u16,
    crossfade_q16: u16,
    center_target: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some((source_content, source_background)) =
        opaque_content_bounds(source, width, height, source_rect)
    else {
        return;
    };
    let target_content =
        label_target_rect(source_content, source_rect, destination_rect, center_target);
    let Some((destination_content, destination_background)) =
        opaque_content_bounds(destination, width, height, destination_rect)
    else {
        return;
    };
    let moving_content = lerp_rect(
        lerp_rect(source_content, target_content, motion_q16),
        destination_content,
        crossfade_q16,
    );
    blit_crossfaded_masks_565(
        working,
        source,
        destination,
        width,
        height,
        source_content,
        destination_content,
        moving_content,
        source_background,
        destination_background,
        crossfade_q16,
        stats,
    );
}

fn label_target_rect(
    content: NavigationTransitionRect,
    from: NavigationTransitionRect,
    to: NavigationTransitionRect,
    center_target: bool,
) -> NavigationTransitionRect {
    let target_height = ((content.height as u32 * to.height.max(1) as u32)
        / from.height.max(1) as u32)
        .min(to.height.max(1) as u32)
        .max(1) as u16;
    let target_width = ((content.width as u32 * target_height as u32)
        / content.height.max(1) as u32)
        .min(to.width.max(1) as u32)
        .max(1) as u16;
    NavigationTransitionRect {
        x: if center_target {
            to.x.saturating_add(to.width.saturating_sub(target_width) / 2)
        } else {
            to.x
        },
        y: to.y.saturating_add(
            ((content.y.saturating_sub(from.y) as u32 * to.height as u32)
                / from.height.max(1) as u32) as u16,
        ),
        width: target_width,
        height: target_height,
    }
}

fn opaque_content_bounds(
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
) -> Option<(NavigationTransitionRect, Rgb565Pixel)> {
    if source.len() != width.saturating_mul(height) || !rect.fits(width, height) {
        return None;
    }
    let x0 = rect.x as usize;
    let y0 = rect.y as usize;
    let x1 = rect.right() as usize;
    let y1 = rect.bottom() as usize;
    let corners = [
        source[y0 * width + x0],
        source[y0 * width + x1.saturating_sub(1)],
        source[y1.saturating_sub(1) * width + x0],
        source[y1.saturating_sub(1) * width + x1.saturating_sub(1)],
    ];
    let background = corners
        .iter()
        .copied()
        .max_by_key(|candidate| corners.iter().filter(|pixel| *pixel == candidate).count())
        .unwrap_or(Rgb565Pixel(0));
    let mut min_x = x1;
    let mut min_y = y1;
    let mut max_x = x0;
    let mut max_y = y0;
    let mut found = false;
    for y in y0..y1 {
        for x in x0..x1 {
            if source[y * width + x] != background {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
                found = true;
            }
        }
    }
    found.then_some((
        NavigationTransitionRect {
            x: min_x as u16,
            y: min_y as u16,
            width: max_x.saturating_sub(min_x).saturating_add(1) as u16,
            height: max_y.saturating_sub(min_y).saturating_add(1) as u16,
        },
        background,
    ))
}

#[allow(clippy::too_many_arguments)]
fn blit_scaled_masked_565(
    destination: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    source_rect: NavigationTransitionRect,
    target_rect: NavigationTransitionRect,
    transparent: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    blit_scaled_masked_dithered_565(
        destination,
        source,
        width,
        height,
        source_rect,
        target_rect,
        transparent,
        PROGRESS_MAX,
        stats,
    );
}

#[allow(clippy::too_many_arguments)]
fn blit_scaled_masked_dithered_565(
    destination: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    source_rect: NavigationTransitionRect,
    target_rect: NavigationTransitionRect,
    transparent: Rgb565Pixel,
    opacity_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let source_w = source_rect.width as usize;
    let source_h = source_rect.height as usize;
    let target_w = target_rect.width as usize;
    let target_h = target_rect.height as usize;
    if source_w == 0
        || source_h == 0
        || target_w == 0
        || target_h == 0
        || opacity_q16 == 0
        || source.len() != width.saturating_mul(height)
        || destination.len() != source.len()
    {
        return;
    }
    const DITHER: [[u16; 4]; 4] = [
        [0, 32_768, 8_192, 40_960],
        [49_152, 16_384, 57_344, 24_576],
        [12_288, 45_056, 4_096, 36_864],
        [61_440, 28_672, 53_248, 20_480],
    ];
    for ty in 0..target_h {
        let dy = target_rect.y as usize + ty;
        if dy >= height {
            break;
        }
        let sy = source_rect.y as usize + ty * source_h / target_h;
        if sy >= height {
            continue;
        }
        for tx in 0..target_w {
            let dx = target_rect.x as usize + tx;
            if dx >= width {
                break;
            }
            let sx = source_rect.x as usize + tx * source_w / target_w;
            if sx < width {
                let pixel = source[sy * width + sx];
                if pixel != transparent && DITHER[dy & 3][dx & 3] < opacity_q16 {
                    destination[dy * width + dx] = pixel;
                    stats.copied_pixels = stats.copied_pixels.saturating_add(1);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_crossfaded_masks_565(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    source_rect: NavigationTransitionRect,
    destination_rect: NavigationTransitionRect,
    target_rect: NavigationTransitionRect,
    source_transparent: Rgb565Pixel,
    destination_transparent: Rgb565Pixel,
    crossfade_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if working.len() != source.len()
        || working.len() != destination.len()
        || working.len() != width.saturating_mul(height)
        || source_rect.width == 0
        || source_rect.height == 0
        || destination_rect.width == 0
        || destination_rect.height == 0
        || target_rect.width == 0
        || target_rect.height == 0
    {
        return;
    }
    const DITHER: [[u16; 4]; 4] = [
        [0, 32_768, 8_192, 40_960],
        [49_152, 16_384, 57_344, 24_576],
        [12_288, 45_056, 4_096, 36_864],
        [61_440, 28_672, 53_248, 20_480],
    ];
    for target_y in 0..target_rect.height as usize {
        let y = target_rect.y as usize + target_y;
        if y >= height {
            break;
        }
        let source_y = source_rect.y as usize
            + target_y * source_rect.height as usize / target_rect.height as usize;
        let destination_y = destination_rect.y as usize
            + target_y * destination_rect.height as usize / target_rect.height as usize;
        for target_x in 0..target_rect.width as usize {
            let x = target_rect.x as usize + target_x;
            if x >= width {
                break;
            }
            let source_x = source_rect.x as usize
                + target_x * source_rect.width as usize / target_rect.width as usize;
            let destination_x = destination_rect.x as usize
                + target_x * destination_rect.width as usize / target_rect.width as usize;
            let choose_destination = DITHER[y & 3][x & 3] < crossfade_q16;
            let source_pixel = source[source_y * width + source_x];
            let destination_pixel = destination[destination_y * width + destination_x];
            let source_opaque = source_pixel != source_transparent;
            let destination_opaque = destination_pixel != destination_transparent;
            let pixel = match (source_opaque, destination_opaque, choose_destination) {
                (_, true, true) => Some(destination_pixel),
                (true, _, false) => Some(source_pixel),
                _ => None,
            };
            if let Some(pixel) = pixel {
                working[y * width + x] = pixel;
                stats.copied_pixels = stats.copied_pixels.saturating_add(1);
            }
        }
    }
}

fn fill_rect_565(
    destination: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    color: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    let x0 = rect.x as usize;
    let y0 = rect.y as usize;
    let x1 = rect.right() as usize;
    let y1 = rect.bottom() as usize;
    for y in y0.min(height)..y1.min(height) {
        let start = y * width + x0.min(width);
        let end = y * width + x1.min(width);
        destination[start..end].fill(color);
        stats.filled_pixels = stats
            .filled_pixels
            .saturating_add(end.saturating_sub(start) as u64);
    }
}

fn draw_outline_565(
    destination: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    color: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    if width == 0 || height == 0 || destination.is_empty() || rect.width == 0 || rect.height == 0 {
        return;
    }
    let x0 = (rect.x as usize).min(width.saturating_sub(1));
    let y0 = (rect.y as usize).min(height.saturating_sub(1));
    let x1 = (rect.right() as usize)
        .saturating_sub(1)
        .min(width.saturating_sub(1));
    let y1 = (rect.bottom() as usize)
        .saturating_sub(1)
        .min(height.saturating_sub(1));
    for x in x0..=x1 {
        destination[y0 * width + x] = color;
        destination[y1 * width + x] = color;
        stats.outline_pixels = stats.outline_pixels.saturating_add(2);
    }
    for y in y0..=y1 {
        destination[y * width + x0] = color;
        destination[y * width + x1] = color;
        stats.outline_pixels = stats.outline_pixels.saturating_add(2);
    }
}

fn draw_velocity_echo_565(
    destination: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    color: Rgb565Pixel,
    far_to_right: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    let vertical_height = (rect.height as usize * 17 / 20).max(1);
    let vertical_y = rect.y as usize + (rect.height as usize - vertical_height) / 2;
    fill_rect_565(
        destination,
        width,
        height,
        NavigationTransitionRect {
            x: if far_to_right {
                rect.right().saturating_sub(1)
            } else {
                rect.x
            },
            y: vertical_y as u16,
            width: 3.min(rect.width),
            height: vertical_height as u16,
        },
        color,
        stats,
    );
    let horizontal_width = (rect.width as usize * 11 / 20).max(1);
    let horizontal_x = if far_to_right {
        rect.right() as usize - horizontal_width
    } else {
        rect.x as usize
    };
    fill_rect_565(
        destination,
        width,
        height,
        NavigationTransitionRect {
            x: horizontal_x as u16,
            y: rect.bottom().saturating_sub(1),
            width: horizontal_width as u16,
            height: 3.min(rect.height),
        },
        color,
        stats,
    );
}

fn lerp_rect(
    from: NavigationTransitionRect,
    to: NavigationTransitionRect,
    progress_q16: u16,
) -> NavigationTransitionRect {
    NavigationTransitionRect {
        x: lerp_u16(from.x, to.x, progress_q16),
        y: lerp_u16(from.y, to.y, progress_q16),
        width: lerp_u16(from.width, to.width, progress_q16),
        height: lerp_u16(from.height, to.height, progress_q16),
    }
}

fn lerp_u16(from: u16, to: u16, progress_q16: u16) -> u16 {
    let from = from as i64;
    let delta = to as i64 - from;
    (from + delta * progress_q16 as i64 / PROGRESS_MAX as i64).clamp(0, u16::MAX as i64) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    include!("navigation_crt_tests.rs");
    include!("navigation_tests.rs");
}
