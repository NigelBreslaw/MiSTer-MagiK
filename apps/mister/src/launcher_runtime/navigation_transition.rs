// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral navigation-transition state and RGB565 frame ownership.

use crate::spring_animation::{smooth_spring_q16, warm_smooth_spring_curve};
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::Instant;

const PROGRESS_MAX: u16 = u16::MAX;
const SUPER_SCALER_COVER_PROGRESS: u16 = 31_457;
const CRT_SWEEP_END_Q16: u16 = 13_107;
const CRT_CLEAR_START_Q16: u16 = 52_428;
const DEFAULT_PREPARATION_TIMEOUT_US: u64 = 5_000_000;
const HUD_WIDTH: usize = 286;
const HUD_HEIGHT: usize = 28;
const HUD_MARGIN: usize = 8;

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
        if self.enters_system_browser() {
            1_440_000
        } else {
            1_260_000
        }
    }

    const fn history_index(self) -> usize {
        match self {
            Self::HomeToConsoles => 0,
            Self::HomeToArcade => 1,
            Self::ConsolesToSystem => 2,
        }
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
    const TILE_GAP: f32 = 16.0;
    const NARROW_TILE_WIDTH: f32 = 191.0;
    let viewport_width = frame_width.saturating_sub(36) as f32;
    let tile_width = if root_menu {
        (viewport_width - 3.0 * TILE_GAP) / 4.0
    } else {
        NARROW_TILE_WIDTH
    };
    let tile_pitch = tile_width + TILE_GAP;
    let card_height =
        (frame_height as f32 - OUTER_PADDING * 2.0 - HEADER_HEIGHT - HEADER_GAP).max(1.0);
    let unclamped_x = OUTER_PADDING + selected_index as f32 * tile_pitch - scroll_x.max(0) as f32;
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
        }
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
pub enum NavigationTransitionInput {
    Activate,
    Back,
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
    pub queued_input: Option<NavigationTransitionInput>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NavigationTransitionTelemetry {
    pub capture_us: u64,
    pub destination_prepare_us: u64,
    pub render_us: u64,
    pub covered_hold_us: u64,
    pub frames: u64,
    pub reused_frames: u64,
    pub overlay_us: u64,
    pub phosphor_pixels: u64,
    pub scanline_pixels: u64,
}

impl NavigationTransitionTelemetry {
    pub fn note_render(&mut self, render_us: u64, reused: bool) {
        self.render_us = self.render_us.saturating_add(render_us);
        self.frames = self.frames.saturating_add(1);
        self.reused_frames = self.reused_frames.saturating_add(u64::from(reused));
    }
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

#[derive(Debug, Default)]
pub struct NavigationTransitionController {
    phase: NavigationTransitionPhase,
    request: Option<NavigationTransitionRequest>,
    phase_started_us: u64,
    covered_started_us: u64,
    covered_observed_us: u64,
    progress_q16: u16,
    reverse_origin_q16: u16,
    queued_input: Option<NavigationTransitionInput>,
    failure: Option<NavigationTransitionFailure>,
    telemetry: NavigationTransitionTelemetry,
}

impl NavigationTransitionController {
    pub fn begin(&mut self, request: NavigationTransitionRequest, now_us: u64) -> bool {
        if self.is_active() {
            return false;
        }
        self.phase = NavigationTransitionPhase::Capture;
        self.request = Some(request);
        self.phase_started_us = now_us;
        self.covered_started_us = 0;
        self.covered_observed_us = 0;
        self.progress_q16 = 0;
        self.reverse_origin_q16 = 0;
        self.queued_input = None;
        self.failure = None;
        self.telemetry = NavigationTransitionTelemetry::default();
        true
    }

    pub fn captured(&mut self, now_us: u64, capture_us: u64) -> bool {
        if self.phase != NavigationTransitionPhase::Capture {
            return false;
        }
        self.telemetry.capture_us = capture_us;
        self.phase = NavigationTransitionPhase::Expand;
        self.phase_started_us = now_us;
        true
    }

    pub fn note_destination_prepared(&mut self, prepare_us: u64) {
        self.telemetry.destination_prepare_us = prepare_us;
    }

    pub fn tick(&mut self, now_us: u64, destination_ready: bool) -> NavigationTransitionFrame {
        let Some(request) = self.request else {
            return NavigationTransitionFrame::default();
        };
        let total_us = request.duration_us.max(1);
        let cover_progress = request_cover_progress_q16(request);
        let forward_cover_us =
            total_us.saturating_mul(SUPER_SCALER_COVER_PROGRESS as u64) / PROGRESS_MAX as u64;
        let cover_us = match request.direction {
            NavigationTransitionDirection::Forward => forward_cover_us,
            NavigationTransitionDirection::Reverse => total_us.saturating_sub(forward_cover_us),
        };

        if self.phase == NavigationTransitionPhase::Expand {
            let elapsed = now_us.saturating_sub(self.phase_started_us);
            self.progress_q16 = match request.direction {
                NavigationTransitionDirection::Forward => {
                    scale_progress(elapsed, cover_us, cover_progress)
                }
                NavigationTransitionDirection::Reverse => {
                    PROGRESS_MAX.saturating_sub(forward_progress_q16_at_elapsed(
                        total_us,
                        total_us.saturating_sub(elapsed.min(cover_us)),
                    ))
                }
            };
            if elapsed >= cover_us {
                self.progress_q16 = cover_progress;
                self.phase = NavigationTransitionPhase::Covered;
                self.covered_started_us = self.phase_started_us.saturating_add(cover_us);
                self.covered_observed_us = now_us;
                self.phase_started_us = self.covered_started_us;
                if destination_ready {
                    self.phase = NavigationTransitionPhase::Reveal;
                }
            }
        }
        if self.phase == NavigationTransitionPhase::Covered {
            if destination_ready {
                let destination_prepared_on_cover_tick = now_us == self.covered_observed_us;
                self.telemetry.covered_hold_us = if destination_prepared_on_cover_tick {
                    0
                } else {
                    now_us.saturating_sub(self.covered_observed_us)
                };
                self.phase = NavigationTransitionPhase::Reveal;
                self.phase_started_us = if destination_prepared_on_cover_tick {
                    self.covered_started_us
                } else {
                    now_us
                };
            } else if now_us.saturating_sub(self.covered_started_us)
                >= request.preparation_timeout_us
            {
                self.failure = Some(NavigationTransitionFailure::DestinationTimeout);
                self.queued_input = None;
                self.start_reversing(now_us);
            }
        }
        if self.phase == NavigationTransitionPhase::Reveal {
            let reveal_us = total_us.saturating_sub(cover_us).max(1);
            let elapsed = now_us.saturating_sub(self.phase_started_us);
            self.progress_q16 = match request.direction {
                NavigationTransitionDirection::Forward => {
                    let reveal_progress =
                        scale_progress(elapsed, reveal_us, PROGRESS_MAX - cover_progress);
                    cover_progress.saturating_add(reveal_progress)
                }
                NavigationTransitionDirection::Reverse => {
                    let reverse_elapsed = cover_us.saturating_add(elapsed.min(reveal_us));
                    PROGRESS_MAX.saturating_sub(forward_progress_q16_at_elapsed(
                        total_us,
                        total_us.saturating_sub(reverse_elapsed),
                    ))
                }
            };
            if elapsed >= reveal_us {
                self.progress_q16 = PROGRESS_MAX;
                self.phase = NavigationTransitionPhase::Settled;
            }
        } else if self.phase == NavigationTransitionPhase::Reversing {
            let reverse_us = total_us
                .saturating_mul(self.reverse_origin_q16 as u64)
                .saturating_div(PROGRESS_MAX as u64)
                .max(1);
            let elapsed = now_us.saturating_sub(self.phase_started_us);
            let reversed = scale_progress(elapsed, reverse_us, self.reverse_origin_q16);
            self.progress_q16 = self.reverse_origin_q16.saturating_sub(reversed);
            if elapsed >= reverse_us {
                self.progress_q16 = 0;
                self.phase = NavigationTransitionPhase::Settled;
            }
        }
        self.frame()
    }

    pub fn request_reverse(&mut self, now_us: u64) -> bool {
        if !matches!(
            self.phase,
            NavigationTransitionPhase::Expand
                | NavigationTransitionPhase::Covered
                | NavigationTransitionPhase::Reveal
        ) {
            return false;
        }
        self.start_reversing(now_us);
        true
    }

    pub fn fail(&mut self, failure: NavigationTransitionFailure, now_us: u64) {
        if self.is_active() {
            self.failure = Some(failure);
            self.queued_input = None;
            self.start_reversing(now_us);
        }
    }

    pub fn cancel_for_exclusive_view(
        &mut self,
        destination_ready: bool,
    ) -> Option<NavigationTransitionEndpoint> {
        if !self.is_active() {
            return None;
        }
        let cover_progress = self
            .request
            .map(request_cover_progress_q16)
            .unwrap_or(SUPER_SCALER_COVER_PROGRESS);
        let endpoint = if self.progress_q16 >= cover_progress && destination_ready {
            NavigationTransitionEndpoint::Destination
        } else {
            NavigationTransitionEndpoint::Source
        };
        self.progress_q16 = match endpoint {
            NavigationTransitionEndpoint::Source => 0,
            NavigationTransitionEndpoint::Destination => PROGRESS_MAX,
        };
        self.phase = NavigationTransitionPhase::Settled;
        self.failure = None;
        self.queued_input = None;
        Some(endpoint)
    }

    pub fn settle_at_destination(&mut self) -> bool {
        if !self.is_active() {
            return false;
        }
        self.progress_q16 = PROGRESS_MAX;
        self.phase = NavigationTransitionPhase::Settled;
        self.failure = None;
        self.queued_input = None;
        true
    }

    pub fn queue_input(&mut self, input: NavigationTransitionInput) {
        if self.is_active() && self.queued_input.is_none() {
            self.queued_input = Some(input);
        }
    }

    pub fn take_queued_input(&mut self) -> Option<NavigationTransitionInput> {
        self.queued_input.take()
    }

    pub fn complete(&mut self) -> Option<NavigationTransitionCompletion> {
        if self.phase != NavigationTransitionPhase::Settled {
            return None;
        }
        let endpoint = self.endpoint()?;
        let completion = NavigationTransitionCompletion {
            endpoint,
            failure: self.failure,
            queued_input: if endpoint == NavigationTransitionEndpoint::Source
                && self.failure.is_some()
            {
                None
            } else {
                self.queued_input.take()
            },
        };
        self.phase = NavigationTransitionPhase::Idle;
        self.request = None;
        self.progress_q16 = 0;
        self.reverse_origin_q16 = 0;
        self.failure = None;
        self.queued_input = None;
        Some(completion)
    }

    pub fn frame(&self) -> NavigationTransitionFrame {
        let cover_progress = self
            .request
            .map(request_cover_progress_q16)
            .unwrap_or(SUPER_SCALER_COVER_PROGRESS);
        let cover_progress_q16 = if self.progress_q16 >= cover_progress {
            PROGRESS_MAX
        } else {
            ((self.progress_q16 as u32 * PROGRESS_MAX as u32) / cover_progress as u32) as u16
        };
        let reveal_progress_q16 = if self.progress_q16 <= cover_progress {
            0
        } else {
            (((self.progress_q16 - cover_progress) as u32 * PROGRESS_MAX as u32)
                / (PROGRESS_MAX - cover_progress) as u32) as u16
        };
        NavigationTransitionFrame {
            phase: self.phase,
            progress_q16: self.progress_q16,
            cover_progress_q16,
            reveal_progress_q16,
            owns_full_frame: self.is_active(),
            endpoint: (self.phase == NavigationTransitionPhase::Settled)
                .then(|| self.endpoint())
                .flatten(),
            failure: self.failure,
            reverse_origin_q16: self.reverse_origin_q16,
            reverse_leg_progress_q16: if self.phase == NavigationTransitionPhase::Reversing
                && self.reverse_origin_q16 > 0
            {
                (((self.reverse_origin_q16 - self.progress_q16) as u32 * PROGRESS_MAX as u32)
                    / self.reverse_origin_q16 as u32) as u16
            } else {
                0
            },
        }
    }

    pub const fn phase(&self) -> NavigationTransitionPhase {
        self.phase
    }

    pub fn request(&self) -> Option<NavigationTransitionRequest> {
        self.request
    }

    pub const fn telemetry(&self) -> NavigationTransitionTelemetry {
        self.telemetry
    }

    pub fn telemetry_mut(&mut self) -> &mut NavigationTransitionTelemetry {
        &mut self.telemetry
    }

    pub const fn is_active(&self) -> bool {
        !matches!(self.phase, NavigationTransitionPhase::Idle)
    }

    fn start_reversing(&mut self, now_us: u64) {
        self.reverse_origin_q16 = self.progress_q16;
        self.phase = if self.reverse_origin_q16 == 0 {
            NavigationTransitionPhase::Settled
        } else {
            NavigationTransitionPhase::Reversing
        };
        self.phase_started_us = now_us;
    }

    fn endpoint(&self) -> Option<NavigationTransitionEndpoint> {
        if self.phase != NavigationTransitionPhase::Settled {
            return None;
        }
        Some(if self.progress_q16 == 0 {
            NavigationTransitionEndpoint::Source
        } else {
            NavigationTransitionEndpoint::Destination
        })
    }
}

const fn request_cover_progress_q16(request: NavigationTransitionRequest) -> u16 {
    let forward_cover = SUPER_SCALER_COVER_PROGRESS;
    match request.direction {
        NavigationTransitionDirection::Forward => forward_cover,
        NavigationTransitionDirection::Reverse => PROGRESS_MAX - forward_cover,
    }
}

fn forward_progress_q16_at_elapsed(total_us: u64, elapsed_us: u64) -> u16 {
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

fn scale_progress(elapsed_us: u64, duration_us: u64, maximum: u16) -> u16 {
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
    pub copied_pixels: u64,
    pub filled_pixels: u64,
    pub outline_pixels: u64,
    pub overlay_us: u64,
    pub phosphor_pixels: u64,
    pub scanline_pixels: u64,
}

#[derive(Debug)]
pub struct NavigationTransitionPoc {
    enabled: bool,
    duration_override_us: Option<u64>,
    scanline_kernel: ScanlineKernel,
    controller: NavigationTransitionController,
    buffers: NavigationTransitionBuffers,
    geometry_history: [Option<NavigationTransitionGeometry>; 3],
    last_render_stats: NavigationTransitionRenderStats,
    last_frame_work_us: u64,
    hud_scratch: Vec<Rgb565Pixel>,
}

impl NavigationTransitionPoc {
    pub fn from_env(width: usize, height: usize) -> Self {
        let mut poc = Self::new(width, height, env_flag("MISTER_NAV_TRANSITION_POC"));
        poc.scanline_kernel = ScanlineKernel::from_env();
        poc.duration_override_us = std::env::var("MISTER_NAV_TRANSITION_DEBUG_DURATION_MS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(|milliseconds| milliseconds.clamp(100, 10_000).saturating_mul(1_000));
        poc
    }

    pub fn new(width: usize, height: usize, enabled: bool) -> Self {
        if enabled {
            warm_smooth_spring_curve();
        }
        let (buffer_width, buffer_height) = if enabled { (width, height) } else { (0, 0) };
        Self {
            enabled,
            duration_override_us: None,
            scanline_kernel: ScanlineKernel::Scalar,
            controller: NavigationTransitionController::default(),
            buffers: NavigationTransitionBuffers::new(buffer_width, buffer_height),
            geometry_history: [None; 3],
            last_render_stats: NavigationTransitionRenderStats::default(),
            last_frame_work_us: 0,
            hud_scratch: vec![
                Rgb565Pixel(0);
                if enabled {
                    HUD_WIDTH.saturating_mul(HUD_HEIGHT)
                } else {
                    0
                }
            ],
        }
    }

    pub fn configure_preview(&mut self, duration_ms: Option<u64>) {
        self.duration_override_us =
            duration_ms.map(|milliseconds| milliseconds.clamp(100, 10_000).saturating_mul(1_000));
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn begin(
        &mut self,
        edge: NavigationTransitionEdge,
        direction: NavigationTransitionDirection,
        geometry: NavigationTransitionGeometry,
        source: &[Rgb565Pixel],
        now_us: u64,
    ) -> Result<bool, NavigationTransitionFailure> {
        if !self.enabled || self.controller.is_active() {
            return Ok(false);
        }
        if direction == NavigationTransitionDirection::Forward {
            self.geometry_history[edge.history_index()] = Some(geometry);
        }
        self.buffers.begin_capture();
        let capture_started = Instant::now();
        self.buffers.capture_source(source)?;
        let capture_us = capture_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        let mut request = NavigationTransitionRequest::new(edge, direction, geometry);
        if let Some(duration_us) = self.duration_override_us {
            request.duration_us = duration_us;
        }
        if !self.controller.begin(request, now_us) {
            return Ok(false);
        }
        self.controller.captured(now_us, capture_us);
        Ok(true)
    }

    pub fn geometry_for_reverse(
        &self,
        edge: NavigationTransitionEdge,
    ) -> Option<NavigationTransitionGeometry> {
        self.geometry_history[edge.history_index()]
    }

    pub fn capture_destination(
        &mut self,
        destination: &[Rgb565Pixel],
    ) -> Result<(), NavigationTransitionFailure> {
        let prepare_started = Instant::now();
        self.buffers.capture_destination(destination)?;
        self.controller.note_destination_prepared(
            prepare_started.elapsed().as_micros().min(u64::MAX as u128) as u64,
        );
        Ok(())
    }

    pub fn tick(&mut self, now_us: u64) -> NavigationTransitionFrame {
        self.controller
            .tick(now_us, self.buffers.destination_ready())
    }

    pub fn render(&mut self) -> Result<&[Rgb565Pixel], NavigationTransitionFailure> {
        let request = self
            .controller
            .request()
            .ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
        let frame = self.controller.frame();
        let started = Instant::now();
        let mut stats = render_super_scaler_shell(&mut self.buffers, request, frame)?;
        render_hero_label_last(&mut self.buffers, request, frame, &mut stats)?;
        let overlay_started = Instant::now();
        apply_crt_scanline_overlay(
            self.buffers.working.as_mut_slice(),
            self.buffers.width,
            self.buffers.height,
            frame,
            self.scanline_kernel,
            &mut stats,
        );
        stats.overlay_us = overlay_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        stats.render_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        self.controller.telemetry.overlay_us = self
            .controller
            .telemetry
            .overlay_us
            .saturating_add(stats.overlay_us);
        self.controller.telemetry.phosphor_pixels = self
            .controller
            .telemetry
            .phosphor_pixels
            .saturating_add(stats.phosphor_pixels);
        self.controller.telemetry.scanline_pixels = self
            .controller
            .telemetry
            .scanline_pixels
            .saturating_add(stats.scanline_pixels);
        self.controller
            .telemetry_mut()
            .note_render(stats.render_us, false);
        self.last_render_stats = stats;
        restore_hud_pixels(
            &self.hud_scratch,
            self.buffers.width,
            self.buffers.height,
            self.buffers.working.as_mut_slice(),
        );
        Ok(self.buffers.working())
    }

    pub fn request_reverse(&mut self, now_us: u64) -> bool {
        self.controller.request_reverse(now_us)
    }

    pub fn settle_at_destination(&mut self) -> bool {
        self.controller.settle_at_destination()
    }

    pub fn queue_input(&mut self, input: NavigationTransitionInput) {
        self.controller.queue_input(input);
    }

    pub fn cancel_for_exclusive_view(&mut self) -> Option<NavigationTransitionEndpoint> {
        self.controller
            .cancel_for_exclusive_view(self.buffers.destination_ready())
    }

    pub fn complete(&mut self) -> Option<NavigationTransitionCompletion> {
        self.controller.complete()
    }

    pub fn frame(&self) -> NavigationTransitionFrame {
        self.controller.frame()
    }

    pub fn request(&self) -> Option<NavigationTransitionRequest> {
        self.controller.request()
    }

    pub const fn is_active(&self) -> bool {
        self.controller.is_active()
    }

    pub const fn destination_ready(&self) -> bool {
        self.buffers.destination_ready()
    }

    pub const fn last_render_stats(&self) -> NavigationTransitionRenderStats {
        self.last_render_stats
    }

    pub const fn last_frame_work_us(&self) -> u64 {
        self.last_frame_work_us
    }

    pub fn note_frame_work_us(&mut self, frame_work_us: u64) {
        self.last_frame_work_us = frame_work_us;
    }

    pub fn capture_hud(&mut self, frame: &[Rgb565Pixel]) {
        let width = self.buffers.width;
        let height = self.buffers.height;
        let hud_width = HUD_WIDTH.min(width.saturating_sub(HUD_MARGIN));
        if !self.enabled
            || hud_width == 0
            || height < HUD_MARGIN.saturating_add(HUD_HEIGHT)
            || frame.len() != width.saturating_mul(height)
            || self.hud_scratch.len() < hud_width.saturating_mul(HUD_HEIGHT)
        {
            return;
        }
        let x = width.saturating_sub(HUD_MARGIN).saturating_sub(hud_width);
        for row in 0..HUD_HEIGHT {
            let source = (HUD_MARGIN + row) * width + x;
            let target = row * hud_width;
            self.hud_scratch[target..target + hud_width]
                .copy_from_slice(&frame[source..source + hud_width]);
        }
    }

    pub fn restore_hud(&self, frame: &mut [Rgb565Pixel]) {
        restore_hud_pixels(
            &self.hud_scratch,
            self.buffers.width,
            self.buffers.height,
            frame,
        );
    }

    pub const fn telemetry(&self) -> NavigationTransitionTelemetry {
        self.controller.telemetry()
    }
}

fn restore_hud_pixels(
    hud_scratch: &[Rgb565Pixel],
    width: usize,
    height: usize,
    frame: &mut [Rgb565Pixel],
) {
    let hud_width = HUD_WIDTH.min(width.saturating_sub(HUD_MARGIN));
    if hud_width == 0
        || height < HUD_MARGIN.saturating_add(HUD_HEIGHT)
        || frame.len() != width.saturating_mul(height)
        || hud_scratch.len() < hud_width.saturating_mul(HUD_HEIGHT)
    {
        return;
    }
    let x = width.saturating_sub(HUD_MARGIN).saturating_sub(hud_width);
    for row in 0..HUD_HEIGHT {
        let source = row * hud_width;
        let target = (HUD_MARGIN + row) * width + x;
        frame[target..target + hud_width].copy_from_slice(&hud_scratch[source..source + hud_width]);
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScanlineKernel {
    Scalar,
    #[cfg(target_arch = "arm")]
    Neon,
}

impl ScanlineKernel {
    fn from_env() -> Self {
        #[cfg(target_arch = "arm")]
        if std::env::var("MISTER_NAV_TRANSITION_SCANLINE_KERNEL")
            .ok()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("neon"))
        {
            assert!(
                scanline_neon::matches_scalar_reference(),
                "navigation transition NEON scanline kernel differs from scalar RGB565 output"
            );
            return Self::Neon;
        }
        Self::Scalar
    }
}

fn apply_crt_scanline_overlay(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    frame: NavigationTransitionFrame,
    kernel: ScanlineKernel,
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

    for y in (1..height).step_by(2) {
        let covered = if reversing {
            overlay_row_covered(frame.reverse_origin_q16, y, height)
                && clear_y.map_or(true, |line_y| y as isize >= line_y)
        } else {
            overlay_row_covered(frame.progress_q16, y, height)
        };
        if !covered {
            continue;
        }
        let start = y * width;
        darken_rgb565_row_7_8(&mut working[start..start + width], kernel);
        stats.phosphor_pixels = stats.phosphor_pixels.saturating_add(width as u64);
    }
}

fn darken_rgb565_row_7_8(row: &mut [Rgb565Pixel], kernel: ScanlineKernel) {
    #[cfg(target_arch = "arm")]
    if kernel == ScanlineKernel::Neon {
        // SAFETY: the kernel reads and writes exactly `row.len()` initialized RGB565 pixels.
        unsafe {
            scanline_neon::darken_row(row);
        }
        return;
    }
    let _ = kernel;
    darken_rgb565_row_scalar_7_8(row);
}

#[inline(always)]
fn darken_rgb565_row_scalar_7_8(row: &mut [Rgb565Pixel]) {
    for pixel in row {
        *pixel = darken_rgb565_7_8(*pixel);
    }
}

#[cfg(target_arch = "arm")]
mod scanline_neon {
    use super::{Rgb565Pixel, darken_rgb565_7_8 as scalar_darken_rgb565_7_8};

    unsafe extern "C" {
        fn mister_magik_scanline_neon_darken_7_8(pixels: *mut u16, count: usize);
    }

    pub(super) unsafe fn darken_row(row: &mut [Rgb565Pixel]) {
        unsafe {
            mister_magik_scanline_neon_darken_7_8(row.as_mut_ptr().cast(), row.len());
        }
    }

    pub(super) fn matches_scalar_reference() -> bool {
        let mut actual = (u16::MIN..=u16::MAX).map(Rgb565Pixel).collect::<Vec<_>>();
        let expected = actual
            .iter()
            .copied()
            .map(scalar_darken_rgb565_7_8)
            .collect::<Vec<_>>();
        // SAFETY: `actual` is a fully initialized, exclusively borrowed RGB565 slice.
        unsafe {
            self::darken_row(&mut actual);
        }
        actual == expected
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
                working.copy_from_slice(source);
                stats.copied_pixels = source.len() as u64;
                return Ok(stats);
            }
            Some(NavigationTransitionEndpoint::Destination) => {
                if let Some(destination) = destination {
                    working.copy_from_slice(destination);
                    stats.copied_pixels = destination.len() as u64;
                } else {
                    working.copy_from_slice(source);
                    stats.copied_pixels = source.len() as u64;
                }
                return Ok(stats);
            }
            None => {}
        }
    }
    if frame.progress_q16 == 0 {
        working.copy_from_slice(source);
        stats.copied_pixels = source.len() as u64;
        return Ok(stats);
    }
    if request.direction == NavigationTransitionDirection::Forward
        && frame.reveal_progress_q16 >= 62_000
    {
        if let Some(destination) = destination {
            working.copy_from_slice(destination);
            stats.copied_pixels = destination.len() as u64;
            return Ok(stats);
        }
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
        working.copy_from_slice(source);
        stats.copied_pixels = source.len() as u64;
    }

    match request.direction {
        NavigationTransitionDirection::Forward => {
            if frame.reveal_progress_q16 > 0 {
                if let Some(destination) = destination {
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
                } else {
                    fill_super_scaler_covered_surface(
                        working, width, height, full, shell, &mut stats,
                    );
                }
            }
            if frame.reveal_progress_q16 == 0 {
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
                    &mut stats,
                );
            }
        }
        NavigationTransitionDirection::Reverse => {
            if frame.reveal_progress_q16 == 0 {
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
            } else if let Some(destination) = destination {
                working.copy_from_slice(destination);
                stats.copied_pixels = stats.copied_pixels.saturating_add(destination.len() as u64);
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
                    &mut stats,
                );
            }
        }
    }
    Ok(stats)
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
            10_000,
            26_000,
        ))),
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
        copy_rect_shifted_x_with_overshoot(
            working,
            destination,
            width,
            height,
            selected,
            spring_ease_q16(window_q16(progress_q16, 10_000, 28_000)),
            -(selected.right() as isize),
            10,
            stats,
        );
        for distance in 1usize..9 {
            let start = 22_000u16.saturating_add(((distance - 1) * 4_500) as u16);
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
                copy_rect_shifted_x(
                    working,
                    destination,
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
                    spring_ease_q16(window_q16(progress_q16, start, start.saturating_add(7_500))),
                    -(list.width as isize + distance as isize * 12),
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
    let left_motion = if selected_left {
        spring_ease_q16(window_q16(progress_q16, PRESS_END, left_end))
    } else {
        spring_ease_q16(window_q16(progress_q16, PRESS_END, left_end))
    };
    let right_motion = if selected_left {
        spring_ease_q16(window_q16(progress_q16, PRESS_END, right_end))
    } else {
        spring_ease_q16(window_q16(progress_q16, PRESS_END, right_end))
    };
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
    let motion = if reverse {
        spring_ease_q16(window_q16(progress_q16, 0, 18_000))
    } else {
        spring_ease_q16(window_q16(progress_q16, 0, 18_000))
    };
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
    stats: &mut NavigationTransitionRenderStats,
) {
    if source.len() != width.saturating_mul(height)
        || destination.len() != source.len()
        || !source_rect.fits(width, height)
        || source_rect.width == 0
        || source_rect.height == 0
        || target_rect.width == 0
        || target_rect.height == 0
        || texture_q16 == 0
    {
        return;
    }
    let source_width = source_rect.width as usize;
    let source_height = source_rect.height as usize;
    let target_width = target_rect.width as usize;
    let target_height = target_rect.height as usize;
    let x_step_q16 = ((source_width as u64) << 16) / target_width as u64;
    let y_step_q16 = ((source_height as u64) << 16) / target_height as u64;
    const DITHER: [[u16; 4]; 4] = [
        [0, 32_768, 8_192, 40_960],
        [49_152, 16_384, 57_344, 24_576],
        [12_288, 45_056, 4_096, 36_864],
        [61_440, 28_672, 53_248, 20_480],
    ];
    for target_y in 0..target_height {
        let y = target_rect.y as usize + target_y;
        if y >= height {
            break;
        }
        let source_y = source_rect.y as usize + ((target_y as u64 * y_step_q16) >> 16) as usize;
        let mut source_x_q16 = 0u64;
        for target_x in 0..target_width {
            let x = target_rect.x as usize + target_x;
            if x >= width {
                break;
            }
            let source_x = source_rect.x as usize + (source_x_q16 >> 16) as usize;
            source_x_q16 = source_x_q16.saturating_add(x_step_q16);
            let excluded = source_x >= excluded_source_rect.x as usize
                && source_x < excluded_source_rect.right() as usize
                && source_y >= excluded_source_rect.y as usize
                && source_y < excluded_source_rect.bottom() as usize;
            if !excluded && DITHER[y & 3][x & 3] < texture_q16 {
                destination[y * width + x] = source[source_y * width + source_x];
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
fn copy_rect_shifted_x_with_overshoot(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    initial_offset: isize,
    overshoot: isize,
    stats: &mut NavigationTransitionRenderStats,
) {
    if progress_q16 == 0 {
        return;
    }
    let offset = if progress_q16 <= 34_000 {
        let motion = spring_ease_q16(window_q16(progress_q16, 0, 34_000)) as i64;
        initial_offset as i64
            + (overshoot as i64 - initial_offset as i64) * motion / PROGRESS_MAX as i64
    } else if progress_q16 <= 50_000 {
        overshoot as i64
    } else {
        let settle = spring_ease_q16(window_q16(progress_q16, 50_000, PROGRESS_MAX)) as i64;
        overshoot as i64 * (PROGRESS_MAX as i64 - settle) / PROGRESS_MAX as i64
    };
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
fn slide_rect_out_left_with_recoil(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    recoil: isize,
    shell: Rgb565Pixel,
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
    fill_rect_565(working, width, height, rect, shell, stats);
    let offset = if progress_q16 <= 10_000 {
        recoil as i64 * spring_ease_q16(window_q16(progress_q16, 0, 10_000)) as i64
            / PROGRESS_MAX as i64
    } else if progress_q16 <= 18_000 {
        recoil as i64
    } else {
        let launch = spring_ease_q16(window_q16(progress_q16, 18_000, PROGRESS_MAX)) as i64;
        recoil as i64 + (-(rect.right() as i64) - recoil as i64) * launch / PROGRESS_MAX as i64
    };
    copy_rect_at_offset(working, source, width, height, rect, offset as isize, stats);
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
        slide_rect_out_left_with_recoil(
            working,
            source,
            width,
            height,
            selected,
            spring_ease_q16(window_q16(progress_q16, 34_000, 51_000)),
            6,
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

    fn geometry() -> NavigationTransitionGeometry {
        NavigationTransitionGeometry {
            source_card: NavigationTransitionRect {
                x: 18,
                y: 74,
                width: 219,
                height: 448,
            },
            source_label: NavigationTransitionRect {
                x: 60,
                y: 260,
                width: 135,
                height: 16,
            },
            destination_title: NavigationTransitionRect {
                x: 18,
                y: 18,
                width: 200,
                height: 24,
            },
            ..NavigationTransitionGeometry::default()
        }
    }

    fn request() -> NavigationTransitionRequest {
        NavigationTransitionRequest::new(
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionDirection::Forward,
            geometry(),
        )
    }

    fn system_request(direction: NavigationTransitionDirection) -> NavigationTransitionRequest {
        NavigationTransitionRequest::new(
            NavigationTransitionEdge::ConsolesToSystem,
            direction,
            NavigationTransitionGeometry {
                source_card: NavigationTransitionRect {
                    x: 2,
                    y: 15,
                    width: 20,
                    height: 80,
                },
                source_label: NavigationTransitionRect {
                    x: 4,
                    y: 46,
                    width: 16,
                    height: 6,
                },
                source_detail: NavigationTransitionRect {
                    x: 4,
                    y: 53,
                    width: 16,
                    height: 4,
                },
                destination_title: NavigationTransitionRect {
                    x: 2,
                    y: 2,
                    width: 30,
                    height: 8,
                },
                destination_detail: NavigationTransitionRect {
                    x: 2,
                    y: 11,
                    width: 30,
                    height: 4,
                },
                destination_list: NavigationTransitionRect {
                    x: 8,
                    y: 18,
                    width: 47,
                    height: 76,
                },
                destination_selected_row: NavigationTransitionRect {
                    x: 8,
                    y: 46,
                    width: 47,
                    height: 6,
                },
                destination_preview: NavigationTransitionRect {
                    x: 58,
                    y: 18,
                    width: 38,
                    height: 76,
                },
                destination_footer: NavigationTransitionRect {
                    x: 8,
                    y: 95,
                    width: 47,
                    height: 5,
                },
            },
        )
    }

    #[test]
    fn super_scaler_edges_keep_intended_durations() {
        assert_eq!(
            NavigationTransitionEdge::HomeToConsoles.duration_us(),
            1_260_000
        );
        assert_eq!(
            NavigationTransitionEdge::HomeToArcade.duration_us(),
            1_440_000
        );
        assert_eq!(
            NavigationTransitionEdge::ConsolesToSystem.duration_us(),
            1_440_000
        );
    }

    #[test]
    fn super_scaler_spatial_windows_use_only_the_smooth_spring() {
        let source = include_str!("navigation_transition.rs");
        let production = source
            .rsplit_once("\n#[cfg(test)]\nmod tests {")
            .expect("test module delimiter")
            .0;
        assert!(!production.contains("smoothstep_q16"));
        assert!(!production.contains("ease_out_cubic_q16"));
        for (line_number, line) in production.lines().enumerate() {
            if line.contains("window_q16(") && !line.contains("fn window_q16(") {
                assert!(
                    line.contains("spring_ease_q16(window_q16("),
                    "raw-linear movement window at source line {}: {line}",
                    line_number + 1
                );
            }
        }
    }

    #[test]
    fn request_duration_override_stretches_the_same_transition_phases() {
        let mut stretched = request();
        stretched.duration_us = 4_000_000;
        let mut controller = NavigationTransitionController::default();
        assert!(controller.begin(stretched, 0));
        assert!(controller.captured(0, 0));

        let expanding = controller.tick(500_000, true);
        assert_eq!(expanding.phase, NavigationTransitionPhase::Expand);
        assert!(expanding.progress_q16 < SUPER_SCALER_COVER_PROGRESS);

        let settled = controller.tick(4_000_000, true);
        assert_eq!(settled.phase, NavigationTransitionPhase::Settled);
        assert_eq!(
            settled.endpoint,
            Some(NavigationTransitionEndpoint::Destination)
        );
    }

    #[test]
    fn super_scaler_card_press_and_expansion_keep_exact_endpoints() {
        let source = geometry().source_card;
        let full = NavigationTransitionRect {
            x: 0,
            y: 0,
            width: 960,
            height: 540,
        };

        assert_eq!(super_scaler_card_rect(source, full, 0), source);
        let pressed = super_scaler_card_rect(source, full, 7_000);
        assert_eq!(pressed.x, source.x + 7);
        assert_eq!(pressed.y, source.y + 24);
        assert_eq!(pressed.width, source.width - 14);
        assert_eq!(pressed.height, source.height - 48);
        let launched = super_scaler_card_rect(source, full, 40_000);
        assert!(launched.x > 0);
        assert!(launched.right() > 900);
        assert!(launched.bottom() > 500);
        assert_eq!(super_scaler_card_rect(source, full, PROGRESS_MAX), full);
    }

    #[test]
    fn super_scaler_keeps_exact_source_and_has_no_cover_reveal_surface_cut() {
        let width = 32;
        let height = 24;
        let source = (0..width * height)
            .map(|pixel| Rgb565Pixel((pixel as u16).wrapping_mul(17)))
            .collect::<Vec<_>>();
        let destination = (0..width * height)
            .map(|pixel| Rgb565Pixel((pixel as u16).wrapping_mul(29)))
            .collect::<Vec<_>>();
        let mut buffers = NavigationTransitionBuffers::new(width, height);
        buffers.capture_source(&source).unwrap();
        buffers.capture_destination(&destination).unwrap();
        let request = NavigationTransitionRequest::new(
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Forward,
            NavigationTransitionGeometry {
                source_card: NavigationTransitionRect {
                    x: 2,
                    y: 4,
                    width: 8,
                    height: 16,
                },
                source_label: NavigationTransitionRect {
                    x: 3,
                    y: 10,
                    width: 6,
                    height: 3,
                },
                destination_title: NavigationTransitionRect {
                    x: 1,
                    y: 1,
                    width: 10,
                    height: 3,
                },
                ..NavigationTransitionGeometry::default()
            },
        );
        let at_source = render_super_scaler_shell(
            &mut buffers,
            request,
            NavigationTransitionFrame {
                phase: NavigationTransitionPhase::Expand,
                owns_full_frame: true,
                ..NavigationTransitionFrame::default()
            },
        )
        .unwrap();
        assert_eq!(at_source.copied_pixels, source.len() as u64);
        assert_eq!(buffers.working(), source);

        let covered_stats = render_super_scaler_shell(
            &mut buffers,
            request,
            NavigationTransitionFrame {
                phase: NavigationTransitionPhase::Expand,
                progress_q16: SUPER_SCALER_COVER_PROGRESS,
                cover_progress_q16: PROGRESS_MAX,
                owns_full_frame: true,
                ..NavigationTransitionFrame::default()
            },
        )
        .unwrap();
        assert_eq!(covered_stats.copied_pixels, 0);
        assert!(covered_stats.filled_pixels >= source.len() as u64);
        let final_cover = buffers.working().to_vec();
        render_super_scaler_shell(
            &mut buffers,
            request,
            NavigationTransitionFrame {
                phase: NavigationTransitionPhase::Reveal,
                progress_q16: SUPER_SCALER_COVER_PROGRESS + 1,
                cover_progress_q16: PROGRESS_MAX,
                reveal_progress_q16: 1,
                owns_full_frame: true,
                ..NavigationTransitionFrame::default()
            },
        )
        .unwrap();
        assert_eq!(buffers.working(), final_cover);
    }

    #[test]
    fn super_scaler_category_edges_keep_speed_bands_through_both_directions() {
        let width = 32;
        let height = 24;
        let shell = Rgb565Pixel(0x1028);
        let source = vec![Rgb565Pixel(0x1111); width * height];
        let destination = vec![Rgb565Pixel(0x2222); width * height];
        let mut buffers = NavigationTransitionBuffers::new(width, height);
        buffers.capture_source(&source).unwrap();
        buffers.capture_destination(&destination).unwrap();
        let geometry = NavigationTransitionGeometry {
            source_card: NavigationTransitionRect {
                x: 2,
                y: 4,
                width: 8,
                height: 16,
            },
            ..NavigationTransitionGeometry::default()
        };
        let request = NavigationTransitionRequest::new(
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionDirection::Forward,
            geometry,
        );

        render_super_scaler_shell(
            &mut buffers,
            request,
            NavigationTransitionFrame {
                phase: NavigationTransitionPhase::Expand,
                progress_q16: SUPER_SCALER_COVER_PROGRESS,
                cover_progress_q16: PROGRESS_MAX,
                owns_full_frame: true,
                ..NavigationTransitionFrame::default()
            },
        )
        .unwrap();
        let covered = buffers.working().to_vec();
        render_super_scaler_shell(
            &mut buffers,
            request,
            NavigationTransitionFrame {
                phase: NavigationTransitionPhase::Reveal,
                progress_q16: SUPER_SCALER_COVER_PROGRESS + 1,
                cover_progress_q16: PROGRESS_MAX,
                reveal_progress_q16: 1,
                owns_full_frame: true,
                ..NavigationTransitionFrame::default()
            },
        )
        .unwrap();
        assert_eq!(buffers.working(), covered);

        let mut concealed = source.clone();
        let mut stats = NavigationTransitionRenderStats::default();
        conceal_source_regions(
            &mut concealed,
            &source,
            width,
            height,
            60_000,
            NavigationTransitionRequest {
                direction: NavigationTransitionDirection::Reverse,
                ..request
            },
            shell,
            &mut stats,
        );
        assert_eq!(
            concealed[21 * width],
            super_scaler_shell_row_color(21, height, shell)
        );
        let mut expected_reverse_cover = vec![shell; width * height];
        fill_super_scaler_covered_surface(
            &mut expected_reverse_cover,
            width,
            height,
            NavigationTransitionRect {
                x: 0,
                y: 0,
                width: width as u16,
                height: height as u16,
            },
            shell,
            &mut stats,
        );
        assert_eq!(concealed, expected_reverse_cover);
    }

    #[test]
    fn system_background_opens_as_one_horizon_instead_of_scanline_moire() {
        let width = 12;
        let height = 16;
        let shell = Rgb565Pixel(0x1111);
        let mut destination = vec![Rgb565Pixel(0x2222); width * height];
        for row in 0..height {
            destination[row * width] = Rgb565Pixel(0xf800);
            destination[row * width + width - 1] = Rgb565Pixel(0xf800);
        }
        let mut working = vec![shell; width * height];
        let mut stats = NavigationTransitionRenderStats::default();

        compose_system_background_horizon(
            &mut working,
            &destination,
            width,
            height,
            PROGRESS_MAX / 2,
            4,
            shell,
            &mut stats,
        );

        let destination_rows = (0..height)
            .filter(|row| working[row * width] == Rgb565Pixel(0x2222))
            .collect::<Vec<_>>();
        assert!(!destination_rows.is_empty());
        assert!(
            destination_rows
                .windows(2)
                .all(|rows| rows[1] == rows[0] + 1)
        );
        assert!(destination_rows.contains(&4));
        assert!(!working.iter().any(|pixel| *pixel == Rgb565Pixel(0xf800)));
    }

    #[test]
    fn scaled_card_excludes_the_duplicate_label_surface() {
        let width = 8;
        let height = 8;
        let mut source = vec![Rgb565Pixel(0x1111); width * height];
        let card = NavigationTransitionRect {
            x: 1,
            y: 1,
            width: 6,
            height: 6,
        };
        let label = NavigationTransitionRect {
            x: 3,
            y: 3,
            width: 2,
            height: 2,
        };
        for y in label.y as usize..label.bottom() as usize {
            for x in label.x as usize..label.right() as usize {
                source[y * width + x] = Rgb565Pixel(0xffff);
            }
        }
        let mut destination = vec![Rgb565Pixel(0x2222); width * height];
        let mut stats = NavigationTransitionRenderStats::default();
        blit_scaled_card_565(
            &mut destination,
            &source,
            width,
            height,
            card,
            card,
            label,
            PROGRESS_MAX,
            &mut stats,
        );

        assert_eq!(destination[3 * width + 3], Rgb565Pixel(0x2222));
        assert_eq!(destination[1 * width + 1], Rgb565Pixel(0x1111));
    }

    #[test]
    fn super_scaler_echoes_remain_visible_above_the_expanding_card() {
        let width = 64;
        let height = 48;
        let source = vec![Rgb565Pixel(0x1111); width * height];
        let mut buffers = NavigationTransitionBuffers::new(width, height);
        buffers.capture_source(&source).unwrap();
        let request = NavigationTransitionRequest::new(
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Forward,
            NavigationTransitionGeometry {
                source_card: NavigationTransitionRect {
                    x: 10,
                    y: 8,
                    width: 18,
                    height: 32,
                },
                ..NavigationTransitionGeometry::default()
            },
        );

        render_super_scaler_shell(
            &mut buffers,
            request,
            NavigationTransitionFrame {
                phase: NavigationTransitionPhase::Expand,
                progress_q16: 20_000,
                cover_progress_q16: 35_000,
                owns_full_frame: true,
                ..NavigationTransitionFrame::default()
            },
        )
        .unwrap();

        assert!(
            buffers
                .working()
                .iter()
                .any(|pixel| *pixel == Rgb565Pixel(0x79b8))
        );
        assert!(
            buffers
                .working()
                .iter()
                .any(|pixel| *pixel == Rgb565Pixel(0x30aa))
        );
        assert!(
            buffers
                .working()
                .iter()
                .any(|pixel| *pixel == Rgb565Pixel(0x07d6))
        );
    }

    #[test]
    fn zero_opacity_detail_draw_does_not_erase_the_expanding_shell() {
        let width = 12;
        let height = 8;
        let mut snapshot = vec![Rgb565Pixel(0x0000); width * height];
        snapshot[3 * width + 5] = Rgb565Pixel(0xffff);
        let mut working = vec![Rgb565Pixel(0x1028); width * height];
        let original = working.clone();
        let mut stats = NavigationTransitionRenderStats::default();

        draw_detail_pixels_with_opacity(
            &mut working,
            &snapshot,
            width,
            height,
            NavigationTransitionRect {
                x: 3,
                y: 2,
                width: 5,
                height: 3,
            },
            0,
            &mut stats,
        );

        assert_eq!(working, original);
        assert_eq!(stats, NavigationTransitionRenderStats::default());
    }

    #[test]
    fn forward_hero_title_docks_to_left_aligned_destination() {
        let from = NavigationTransitionRect {
            x: 20,
            y: 100,
            width: 180,
            height: 16,
        };
        let centered_content = NavigationTransitionRect {
            x: 80,
            y: 103,
            width: 60,
            height: 10,
        };
        let destination = NavigationTransitionRect {
            x: 16,
            y: 16,
            width: 160,
            height: 24,
        };

        let target = label_target_rect(centered_content, from, destination, false);

        assert_eq!(target.x, destination.x);
        assert_eq!(target.y, 20);
        assert_eq!(target.height, 15);
    }

    #[test]
    fn final_region_reveal_is_the_exact_destination() {
        let width = 16;
        let height = 12;
        let mut working = vec![Rgb565Pixel(0); width * height];
        let destination = (0..width * height)
            .map(|pixel| Rgb565Pixel(pixel as u16))
            .collect::<Vec<_>>();
        let mut stats = NavigationTransitionRenderStats::default();

        reveal_destination_regions(
            &mut working,
            &destination,
            width,
            height,
            62_000,
            request(),
            &mut stats,
        );

        assert_eq!(working, destination);
    }

    #[test]
    fn system_reveal_orders_title_rows_frame_and_preview_content() {
        let width = 100;
        let height = 100;
        let mut destination = vec![Rgb565Pixel(0); width * height];
        for y in 2..10 {
            destination[y * width + 2..y * width + 32].fill(Rgb565Pixel(0x1234));
        }
        for y in 18..24 {
            destination[y * width + 8..y * width + 55].fill(Rgb565Pixel(0x4567));
        }
        for y in 46..52 {
            destination[y * width + 8..y * width + 55].fill(Rgb565Pixel(0x1234));
        }
        for y in 70..76 {
            destination[y * width + 8..y * width + 55].fill(Rgb565Pixel(0x1234));
        }
        for y in 18..94 {
            destination[y * width + 58..y * width + 96].fill(Rgb565Pixel(0x1234));
        }
        let mut stats = NavigationTransitionRenderStats::default();

        let mut title_only = vec![Rgb565Pixel(0); width * height];
        reveal_destination_regions(
            &mut title_only,
            &destination,
            width,
            height,
            8_000,
            system_request(NavigationTransitionDirection::Forward),
            &mut stats,
        );
        assert_eq!(title_only[5 * width + 5], Rgb565Pixel(0x1234));
        assert_eq!(title_only[20 * width + 20], Rgb565Pixel(0));
        assert_eq!(title_only[46 * width + 20], Rgb565Pixel(0));

        let mut selected_row = vec![Rgb565Pixel(0); width * height];
        reveal_destination_regions(
            &mut selected_row,
            &destination,
            width,
            height,
            22_000,
            system_request(NavigationTransitionDirection::Forward),
            &mut stats,
        );
        assert_eq!(selected_row[46 * width + 20], Rgb565Pixel(0x1234));
        assert_eq!(selected_row[70 * width + 20], Rgb565Pixel(0));

        let mut framed = vec![Rgb565Pixel(0); width * height];
        reveal_destination_regions(
            &mut framed,
            &destination,
            width,
            height,
            46_000,
            system_request(NavigationTransitionDirection::Forward),
            &mut stats,
        );
        assert_eq!(framed[18 * width + 77], Rgb565Pixel(0x79b8));
        assert_eq!(framed[50 * width + 77], Rgb565Pixel(0x1234));
    }

    #[test]
    fn preview_rails_pulse_without_popping_at_forward_or_reverse_endpoints() {
        assert_eq!(preview_rail_envelope(34_000), 0);
        assert!(preview_rail_envelope(40_000) > 0);
        assert_eq!(preview_rail_envelope(42_000), PROGRESS_MAX);
        assert_eq!(preview_rail_envelope(44_000), PROGRESS_MAX);
        assert_eq!(preview_rail_envelope(48_000), PROGRESS_MAX);
        assert_eq!(preview_rail_envelope(58_000), 0);
        assert_eq!(preview_rail_envelope(61_999), 0);
        assert_eq!(
            preview_rail_envelope(reverse_destination_reveal_progress(0)),
            0
        );
        assert_eq!(
            preview_rail_envelope(reverse_destination_reveal_progress(14_000)),
            PROGRESS_MAX
        );
        assert_eq!(
            preview_rail_envelope(reverse_destination_reveal_progress(28_000)),
            0
        );
    }

    #[test]
    fn preview_aperture_opens_from_a_horizontal_scanline() {
        let preview = NavigationTransitionRect {
            x: 100,
            y: 80,
            width: 320,
            height: 240,
        };
        let slit = preview_aperture_rect(preview, 8_000).unwrap();
        assert!(slit.width > 64);
        assert!(slit.height <= 2);
        assert_eq!(preview_aperture_rect(preview, PROGRESS_MAX), Some(preview));
    }

    #[test]
    fn system_reverse_reconstructs_exact_forward_reveal_endpoints() {
        let width = 100;
        let height = 100;
        let source = vec![Rgb565Pixel(0x1234); width * height];
        let shell = Rgb565Pixel(0x1028);
        let mut working = source.clone();
        let mut stats = NavigationTransitionRenderStats::default();

        conceal_source_regions_inverse(
            &mut working,
            &source,
            width,
            height,
            0,
            system_request(NavigationTransitionDirection::Reverse),
            shell,
            &mut stats,
        );

        assert_eq!(working, source);

        conceal_source_regions_inverse(
            &mut working,
            &source,
            width,
            height,
            PROGRESS_MAX,
            system_request(NavigationTransitionDirection::Reverse),
            shell,
            &mut stats,
        );
        let mut expected = vec![shell; width * height];
        fill_super_scaler_covered_surface(
            &mut expected,
            width,
            height,
            NavigationTransitionRect {
                x: 0,
                y: 0,
                width: width as u16,
                height: height as u16,
            },
            shell,
            &mut stats,
        );
        assert_eq!(working, expected);
    }

    #[test]
    fn shifted_row_copy_ignores_equal_undersized_buffers() {
        let source = vec![Rgb565Pixel(0xaaaa); 8];
        let mut working = vec![Rgb565Pixel(0); 8];
        let mut stats = NavigationTransitionRenderStats::default();

        copy_rect_shifted_x(
            &mut working,
            &source,
            8,
            8,
            NavigationTransitionRect {
                x: 0,
                y: 0,
                width: 8,
                height: 8,
            },
            PROGRESS_MAX,
            -28,
            &mut stats,
        );

        assert!(working.iter().all(|pixel| *pixel == Rgb565Pixel(0)));
    }

    #[test]
    fn label_crossfade_uses_one_moving_surface_without_dither_holes() {
        let width = 16;
        let height = 8;
        let mut source = vec![Rgb565Pixel(0); width * height];
        let mut destination = vec![Rgb565Pixel(0); width * height];
        let source_rect = NavigationTransitionRect {
            x: 1,
            y: 1,
            width: 4,
            height: 4,
        };
        let destination_rect = NavigationTransitionRect {
            x: 10,
            y: 2,
            width: 4,
            height: 4,
        };
        let target = NavigationTransitionRect {
            x: 6,
            y: 2,
            width: 4,
            height: 4,
        };
        for y in source_rect.y as usize..source_rect.bottom() as usize {
            source[y * width + source_rect.x as usize..y * width + source_rect.right() as usize]
                .fill(Rgb565Pixel(0x1111));
        }
        for y in destination_rect.y as usize..destination_rect.bottom() as usize {
            destination[y * width + destination_rect.x as usize
                ..y * width + destination_rect.right() as usize]
                .fill(Rgb565Pixel(0xeeee));
        }
        let mut working = vec![Rgb565Pixel(0); width * height];
        let mut stats = NavigationTransitionRenderStats::default();

        blit_crossfaded_masks_565(
            &mut working,
            &source,
            &destination,
            width,
            height,
            source_rect,
            destination_rect,
            target,
            Rgb565Pixel(0),
            Rgb565Pixel(0),
            PROGRESS_MAX / 2,
            &mut stats,
        );

        for y in target.y as usize..target.bottom() as usize {
            for x in target.x as usize..target.right() as usize {
                assert_ne!(working[y * width + x], Rgb565Pixel(0));
            }
        }
        assert_eq!(working[2 * width + 10], Rgb565Pixel(0));
    }

    #[test]
    fn label_crossfade_deterministically_erodes_disjoint_glyph_masks() {
        let width = 12;
        let height = 6;
        let rect = NavigationTransitionRect {
            x: 2,
            y: 1,
            width: 8,
            height: 4,
        };
        let mut source = vec![Rgb565Pixel(0); width * height];
        let mut destination = vec![Rgb565Pixel(0); width * height];
        for y in rect.y as usize..rect.bottom() as usize {
            source[y * width + 2..y * width + 6].fill(Rgb565Pixel(0x1111));
            destination[y * width + 6..y * width + 10].fill(Rgb565Pixel(0xeeee));
        }
        let mut first = vec![Rgb565Pixel(0); width * height];
        let mut second = first.clone();
        let mut stats = NavigationTransitionRenderStats::default();

        for working in [&mut first, &mut second] {
            blit_crossfaded_masks_565(
                working,
                &source,
                &destination,
                width,
                height,
                rect,
                rect,
                rect,
                Rgb565Pixel(0),
                Rgb565Pixel(0),
                PROGRESS_MAX / 2,
                &mut stats,
            );
        }

        assert_eq!(first, second);
        assert!(first.iter().any(|pixel| *pixel == Rgb565Pixel(0x1111)));
        assert!(first.iter().any(|pixel| *pixel == Rgb565Pixel(0xeeee)));
        assert!(first.iter().any(|pixel| *pixel == Rgb565Pixel(0)));
    }

    #[test]
    fn reverse_row_translation_preserves_source_alignment_and_clipping() {
        let width = 20;
        let height = 8;
        let rect = NavigationTransitionRect {
            x: 8,
            y: 6,
            width: 8,
            height: 2,
        };
        let shell = Rgb565Pixel(0x2222);
        let mut source = vec![Rgb565Pixel(0); width * height];
        for y in rect.y as usize..rect.bottom() as usize {
            for x in rect.x as usize..rect.right() as usize {
                source[y * width + x] = Rgb565Pixel(x as u16);
            }
        }
        let mut stats = NavigationTransitionRenderStats::default();

        let mut at_start = source.clone();
        slide_rect_out_left(
            &mut at_start,
            &source,
            width,
            height,
            rect,
            0,
            shell,
            &mut stats,
        );
        assert_eq!(
            &at_start[6 * width + 8..6 * width + 16],
            &source[6 * width + 8..6 * width + 16]
        );

        let mut halfway = vec![Rgb565Pixel(0); width * height];
        slide_rect_out_left(
            &mut halfway,
            &source,
            width,
            height,
            rect,
            PROGRESS_MAX / 2 + 1,
            shell,
            &mut stats,
        );
        assert_eq!(halfway[6 * width], Rgb565Pixel(8));
        assert_eq!(halfway[6 * width + 7], Rgb565Pixel(15));
        assert_eq!(halfway[7 * width], Rgb565Pixel(8));

        let mut nearly_gone = source.clone();
        slide_rect_out_left(
            &mut nearly_gone,
            &source,
            width,
            height,
            rect,
            PROGRESS_MAX - 1,
            shell,
            &mut stats,
        );
        assert_eq!(nearly_gone[6 * width], Rgb565Pixel(15));
        assert_eq!(nearly_gone[6 * width + 1], Rgb565Pixel(0));

        let mut at_end = vec![Rgb565Pixel(0); width * height];
        slide_rect_out_left(
            &mut at_end,
            &source,
            width,
            height,
            rect,
            PROGRESS_MAX,
            shell,
            &mut stats,
        );
        assert!(
            at_end[6 * width + 8..6 * width + 16]
                .iter()
                .all(|pixel| *pixel == shell)
        );
    }

    #[test]
    fn forward_row_translation_enters_from_the_first_screen_pixel() {
        let width = 20;
        let height = 8;
        let rect = NavigationTransitionRect {
            x: 8,
            y: 6,
            width: 8,
            height: 2,
        };
        let mut source = vec![Rgb565Pixel(0); width * height];
        for y in rect.y as usize..rect.bottom() as usize {
            for x in rect.x as usize..rect.right() as usize {
                source[y * width + x] = Rgb565Pixel(x as u16);
            }
        }
        let mut working = vec![Rgb565Pixel(0); width * height];
        let mut stats = NavigationTransitionRenderStats::default();

        copy_rect_shifted_x(
            &mut working,
            &source,
            width,
            height,
            rect,
            1,
            -(rect.right() as isize),
            &mut stats,
        );

        assert_eq!(working[6 * width], Rgb565Pixel(15));
        assert_eq!(working[6 * width + 1], Rgb565Pixel(0));
    }

    #[test]
    fn selected_row_overshoots_then_settles_to_its_exact_destination() {
        let width = 32;
        let height = 8;
        let rect = NavigationTransitionRect {
            x: 8,
            y: 3,
            width: 8,
            height: 2,
        };
        let mut source = vec![Rgb565Pixel(0); width * height];
        source[3 * width + 8..3 * width + 16].fill(Rgb565Pixel(0xaaaa));
        let mut overshot = vec![Rgb565Pixel(0); width * height];
        let mut stats = NavigationTransitionRenderStats::default();

        copy_rect_shifted_x_with_overshoot(
            &mut overshot,
            &source,
            width,
            height,
            rect,
            40_000,
            -(rect.right() as isize),
            6,
            &mut stats,
        );
        assert_eq!(overshot[3 * width + 14], Rgb565Pixel(0xaaaa));
        assert_eq!(overshot[3 * width + 8], Rgb565Pixel(0));

        let mut settled = vec![Rgb565Pixel(0); width * height];
        copy_rect_shifted_x_with_overshoot(
            &mut settled,
            &source,
            width,
            height,
            rect,
            PROGRESS_MAX,
            -(rect.right() as isize),
            6,
            &mut stats,
        );
        assert_eq!(
            &settled[3 * width + 8..3 * width + 16],
            &source[3 * width + 8..3 * width + 16]
        );
    }

    #[test]
    fn reverse_selected_row_recoils_for_a_frame_then_exits_with_clipping() {
        let width = 32;
        let height = 8;
        let shell = Rgb565Pixel(0x2222);
        let rect = NavigationTransitionRect {
            x: 24,
            y: 3,
            width: 8,
            height: 2,
        };
        let mut source = vec![Rgb565Pixel(0); width * height];
        for x in rect.x as usize..rect.right() as usize {
            source[3 * width + x] = Rgb565Pixel(x as u16);
        }
        let mut stats = NavigationTransitionRenderStats::default();
        let mut unchanged = source.clone();
        slide_rect_out_left_with_recoil(
            &mut unchanged,
            &source,
            width,
            height,
            rect,
            0,
            6,
            shell,
            &mut stats,
        );
        assert_eq!(unchanged, source);

        let mut recoiled = source.clone();
        slide_rect_out_left_with_recoil(
            &mut recoiled,
            &source,
            width,
            height,
            rect,
            14_000,
            6,
            shell,
            &mut stats,
        );
        assert_eq!(recoiled[3 * width + 30], Rgb565Pixel(24));
        assert_eq!(recoiled[3 * width + 31], Rgb565Pixel(25));

        let mut gone = source.clone();
        slide_rect_out_left_with_recoil(
            &mut gone,
            &source,
            width,
            height,
            rect,
            PROGRESS_MAX,
            6,
            shell,
            &mut stats,
        );
        assert!(
            gone[3 * width + 24..3 * width + 32]
                .iter()
                .all(|pixel| *pixel == shell)
        );
    }

    #[test]
    fn reverse_preview_aperture_keeps_identity_and_closes_exactly() {
        let width = 20;
        let height = 12;
        let shell = Rgb565Pixel(0x2222);
        let preview = NavigationTransitionRect {
            x: 4,
            y: 3,
            width: 12,
            height: 6,
        };
        let source = vec![Rgb565Pixel(0xaaaa); width * height];
        let mut stats = NavigationTransitionRenderStats::default();
        let mut unchanged = source.clone();
        close_preview_aperture(
            &mut unchanged,
            &source,
            width,
            height,
            preview,
            0,
            shell,
            &mut stats,
        );
        assert_eq!(unchanged, source);

        let mut closed = source.clone();
        close_preview_aperture(
            &mut closed,
            &source,
            width,
            height,
            preview,
            PROGRESS_MAX,
            shell,
            &mut stats,
        );
        for y in preview.y as usize..preview.bottom() as usize {
            assert!(
                closed[y * width + preview.x as usize..y * width + preview.right() as usize]
                    .iter()
                    .all(|pixel| *pixel == shell)
            );
        }
    }

    #[test]
    fn wipe_helpers_clip_saturated_rectangles_without_panicking() {
        let width = 8;
        let height = 6;
        let source = vec![Rgb565Pixel(0xaaaa); width * height];
        let mut working = vec![Rgb565Pixel(0); width * height];
        let mut stats = NavigationTransitionRenderStats::default();
        let saturated = NavigationTransitionRect {
            x: 6,
            y: 4,
            width: u16::MAX,
            height: u16::MAX,
        };

        copy_rect_horizontal_wipe(
            &mut working,
            &source,
            width,
            height,
            saturated,
            PROGRESS_MAX,
            usize::MAX,
            &mut stats,
        );
        copy_rect_vertical_wipe(
            &mut working,
            &source,
            width,
            height,
            saturated,
            PROGRESS_MAX,
            false,
            &mut stats,
        );

        assert_eq!(working[4 * width + 6], Rgb565Pixel(0xaaaa));
        assert_eq!(working[5 * width + 7], Rgb565Pixel(0xaaaa));
    }

    #[test]
    fn transition_waits_covered_until_destination_is_ready() {
        let mut controller = NavigationTransitionController::default();
        let request = request();
        let cover_us =
            request.duration_us * SUPER_SCALER_COVER_PROGRESS as u64 / PROGRESS_MAX as u64;
        assert!(controller.begin(request, 1_000));
        assert!(controller.captured(2_000, 300));
        let covered_at = 2_000 + cover_us;

        let covered = controller.tick(covered_at, false);
        assert_eq!(covered.phase, NavigationTransitionPhase::Covered);
        assert_eq!(covered.progress_q16, SUPER_SCALER_COVER_PROGRESS);

        let still_covered = controller.tick(covered_at + 200_000, false);
        assert_eq!(still_covered.phase, NavigationTransitionPhase::Covered);

        let reveal_at = covered_at + 210_000;
        let reveal = controller.tick(reveal_at, true);
        assert_eq!(reveal.phase, NavigationTransitionPhase::Reveal);
        assert_eq!(
            controller.telemetry().covered_hold_us,
            reveal_at - covered_at
        );
    }

    #[test]
    fn standalone_reverse_uses_the_complementary_covered_boundary() {
        let forward = NavigationTransitionRequest::new(
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionDirection::Forward,
            geometry(),
        );
        let reverse = NavigationTransitionRequest {
            direction: NavigationTransitionDirection::Reverse,
            ..forward
        };
        assert_eq!(
            request_cover_progress_q16(forward),
            SUPER_SCALER_COVER_PROGRESS
        );
        assert_eq!(
            request_cover_progress_q16(reverse),
            PROGRESS_MAX - SUPER_SCALER_COVER_PROGRESS
        );

        let mut controller = NavigationTransitionController::default();
        assert!(controller.begin(reverse, 0));
        assert!(controller.captured(0, 0));
        let forward_covered_us =
            reverse.duration_us * SUPER_SCALER_COVER_PROGRESS as u64 / PROGRESS_MAX as u64;
        let covered_us = reverse.duration_us - forward_covered_us;
        let covered = controller.tick(covered_us, true);
        assert_eq!(
            covered.progress_q16,
            PROGRESS_MAX - SUPER_SCALER_COVER_PROGRESS
        );
        assert_eq!(covered.cover_progress_q16, PROGRESS_MAX);
        assert_eq!(covered.reveal_progress_q16, 0);
    }

    #[test]
    fn standalone_reverse_progress_is_the_exact_forward_complement() {
        let frame_at = |request, elapsed_us| {
            let mut controller = NavigationTransitionController::default();
            assert!(controller.begin(request, 0));
            assert!(controller.captured(0, 0));
            controller.tick(elapsed_us, true)
        };
        for duration_us in [500_000, 4_000_000] {
            let forward = NavigationTransitionRequest {
                duration_us,
                ..NavigationTransitionRequest::new(
                    NavigationTransitionEdge::HomeToArcade,
                    NavigationTransitionDirection::Forward,
                    geometry(),
                )
            };
            let reverse = NavigationTransitionRequest {
                direction: NavigationTransitionDirection::Reverse,
                ..forward
            };
            let covered_us = duration_us * SUPER_SCALER_COVER_PROGRESS as u64 / PROGRESS_MAX as u64;
            for forward_us in [0, covered_us, duration_us / 2, duration_us] {
                let forward_frame = frame_at(forward, forward_us);
                let reverse_frame = frame_at(reverse, duration_us - forward_us);
                assert_eq!(
                    forward_frame.progress_q16,
                    PROGRESS_MAX - reverse_frame.progress_q16,
                    "duration={duration_us} forward_us={forward_us}"
                );
                assert_eq!(
                    spring_ease_q16(forward_frame.progress_q16),
                    spring_ease_q16(PROGRESS_MAX - reverse_frame.progress_q16),
                    "spring playback diverged at duration={duration_us} forward_us={forward_us}"
                );
            }
        }
    }

    #[test]
    fn destination_prepared_on_the_cover_tick_does_not_add_a_quantization_hold() {
        let duration_us = 500_000;
        let forward = NavigationTransitionRequest {
            duration_us,
            ..NavigationTransitionRequest::new(
                NavigationTransitionEdge::HomeToArcade,
                NavigationTransitionDirection::Forward,
                geometry(),
            )
        };
        let reverse = NavigationTransitionRequest {
            direction: NavigationTransitionDirection::Reverse,
            ..forward
        };
        let mut forward_controller = NavigationTransitionController::default();
        assert!(forward_controller.begin(forward, 0));
        assert!(forward_controller.captured(0, 0));
        let forward_midpoint = forward_controller.tick(duration_us / 2, true);

        let mut reverse_controller = NavigationTransitionController::default();
        assert!(reverse_controller.begin(reverse, 0));
        assert!(reverse_controller.captured(0, 0));
        let observed_cover = reverse_controller.tick(233_333, false);
        assert_eq!(observed_cover.phase, NavigationTransitionPhase::Covered);
        let reveal = reverse_controller.tick(233_333, true);
        assert_eq!(reveal.phase, NavigationTransitionPhase::Reveal);
        assert_eq!(reverse_controller.telemetry().covered_hold_us, 0);
        let reverse_midpoint = reverse_controller.tick(duration_us / 2, true);
        assert_eq!(
            forward_midpoint.progress_q16,
            PROGRESS_MAX - reverse_midpoint.progress_q16
        );
    }

    #[test]
    fn completed_transition_settles_at_destination() {
        let mut controller = NavigationTransitionController::default();
        let request = request();
        let duration_us = request.duration_us;
        controller.begin(request, 0);
        controller.captured(0, 0);
        controller.tick(300_000, true);
        controller.tick(300_001, true);
        let settled = controller.tick(duration_us, true);

        assert_eq!(settled.phase, NavigationTransitionPhase::Settled);
        assert_eq!(
            settled.endpoint,
            Some(NavigationTransitionEndpoint::Destination)
        );
        assert_eq!(
            controller.complete(),
            Some(NavigationTransitionCompletion {
                endpoint: NavigationTransitionEndpoint::Destination,
                failure: None,
                queued_input: None,
            })
        );
        assert!(!controller.is_active());
    }

    #[test]
    fn ready_destination_preserves_elapsed_overrun_across_boundaries() {
        let mut controller = NavigationTransitionController::default();
        let request = request();
        let duration_us = request.duration_us;
        controller.begin(request, 0);
        controller.captured(0, 0);

        let settled = controller.tick(duration_us, true);

        assert_eq!(settled.phase, NavigationTransitionPhase::Settled);
        assert_eq!(
            settled.endpoint,
            Some(NavigationTransitionEndpoint::Destination)
        );
    }

    #[test]
    fn back_during_motion_reverses_without_progress_jump() {
        let mut controller = NavigationTransitionController::default();
        controller.begin(request(), 0);
        controller.captured(0, 0);
        let forward = controller.tick(100_000, false);
        assert!(forward.progress_q16 > 0);

        assert!(controller.request_reverse(100_000));
        assert_eq!(controller.frame().progress_q16, forward.progress_q16);
        let settled = controller.tick(600_000, false);
        assert_eq!(settled.phase, NavigationTransitionPhase::Settled);
        assert_eq!(settled.endpoint, Some(NavigationTransitionEndpoint::Source));
    }

    #[test]
    fn only_one_input_is_queued() {
        let mut controller = NavigationTransitionController::default();
        controller.begin(request(), 0);
        controller.queue_input(NavigationTransitionInput::Activate);
        controller.queue_input(NavigationTransitionInput::Back);

        assert_eq!(
            controller.take_queued_input(),
            Some(NavigationTransitionInput::Activate)
        );
        assert_eq!(controller.take_queued_input(), None);
    }

    #[test]
    fn preparation_timeout_reverses_to_source() {
        let mut timed = request();
        timed.preparation_timeout_us = 50_000;
        let cover_us = timed.duration_us * SUPER_SCALER_COVER_PROGRESS as u64 / PROGRESS_MAX as u64;
        let mut controller = NavigationTransitionController::default();
        controller.begin(timed, 0);
        controller.captured(0, 0);
        controller.tick(cover_us, false);
        let reverse_at = cover_us + timed.preparation_timeout_us;
        let reversing = controller.tick(reverse_at, false);

        assert_eq!(reversing.phase, NavigationTransitionPhase::Reversing);
        assert_eq!(
            reversing.failure,
            Some(NavigationTransitionFailure::DestinationTimeout)
        );
        let settled = controller.tick(reverse_at + cover_us, false);
        assert_eq!(settled.endpoint, Some(NavigationTransitionEndpoint::Source));
    }

    #[test]
    fn failed_recap_does_not_expose_stale_snapshot() {
        let mut buffers = NavigationTransitionBuffers::new(4, 3);
        let pixels = vec![Rgb565Pixel(0x1234); 12];
        buffers.capture_source(&pixels).unwrap();
        buffers.capture_destination(&pixels).unwrap();

        assert_eq!(
            buffers.capture_source(&pixels[..11]),
            Err(NavigationTransitionFailure::SnapshotSizeMismatch)
        );
        assert!(!buffers.source_ready());
        assert_eq!(buffers.source(), None);
        assert!(buffers.destination_ready());

        assert_eq!(
            buffers.capture_destination(&pixels[..11]),
            Err(NavigationTransitionFailure::SnapshotSizeMismatch)
        );
        assert!(!buffers.destination_ready());
        assert_eq!(buffers.destination(), None);
    }

    #[test]
    fn buffers_reuse_storage_without_clearing_live_snapshots() {
        let mut buffers = NavigationTransitionBuffers::new(4, 3);
        let pixels = (0..12)
            .map(|value| Rgb565Pixel(value as u16))
            .collect::<Vec<_>>();
        buffers.capture_source(&pixels).unwrap();
        buffers.capture_destination(&pixels).unwrap();

        assert_eq!(buffers.source(), Some(pixels.as_slice()));
        assert_eq!(buffers.destination(), Some(pixels.as_slice()));

        let working_ptr = buffers.working().as_ptr();
        buffers.resize(4, 3);
        assert_eq!(buffers.working().as_ptr(), working_ptr);
        assert!(buffers.source_ready());
        assert!(buffers.destination_ready());

        buffers.begin_capture();
        assert!(!buffers.source_ready());
        assert!(!buffers.destination_ready());
    }

    #[test]
    fn timeout_discards_queued_input_atomically() {
        let mut timed = request();
        timed.preparation_timeout_us = 50_000;
        let cover_us = timed.duration_us * SUPER_SCALER_COVER_PROGRESS as u64 / PROGRESS_MAX as u64;
        let mut controller = NavigationTransitionController::default();
        controller.begin(timed, 0);
        controller.captured(0, 0);
        controller.queue_input(NavigationTransitionInput::Activate);
        controller.tick(cover_us, false);
        let reverse_at = cover_us + timed.preparation_timeout_us;
        controller.tick(reverse_at, false);
        controller.tick(reverse_at + cover_us, false);

        assert_eq!(
            controller.complete(),
            Some(NavigationTransitionCompletion {
                endpoint: NavigationTransitionEndpoint::Source,
                failure: Some(NavigationTransitionFailure::DestinationTimeout),
                queued_input: None,
            })
        );
    }

    #[test]
    fn exclusive_view_cancels_only_to_a_ready_destination() {
        let cover_us = NavigationTransitionEdge::HomeToConsoles.duration_us()
            * SUPER_SCALER_COVER_PROGRESS as u64
            / PROGRESS_MAX as u64;
        let mut before_cover = NavigationTransitionController::default();
        before_cover.begin(request(), 0);
        before_cover.captured(0, 0);
        before_cover.tick(100_000, false);
        assert_eq!(
            before_cover.cancel_for_exclusive_view(false),
            Some(NavigationTransitionEndpoint::Source)
        );

        let mut covered_unready = NavigationTransitionController::default();
        covered_unready.begin(request(), 0);
        covered_unready.captured(0, 0);
        covered_unready.tick(cover_us, false);
        assert_eq!(
            covered_unready.cancel_for_exclusive_view(false),
            Some(NavigationTransitionEndpoint::Source)
        );
        assert_eq!(
            covered_unready
                .complete()
                .map(|completion| completion.endpoint),
            Some(NavigationTransitionEndpoint::Source)
        );

        let mut covered_ready = NavigationTransitionController::default();
        covered_ready.begin(request(), 0);
        covered_ready.captured(0, 0);
        covered_ready.tick(cover_us, true);
        assert_eq!(
            covered_ready.cancel_for_exclusive_view(true),
            Some(NavigationTransitionEndpoint::Destination)
        );
        assert_eq!(
            covered_ready
                .complete()
                .map(|completion| completion.endpoint),
            Some(NavigationTransitionEndpoint::Destination)
        );
    }

    #[test]
    fn geometry_bounds_are_explicit() {
        assert!(geometry().source_card.fits(960, 540));
        assert!(!NavigationTransitionRect::default().fits(960, 540));
    }

    #[test]
    fn hdmi_geometry_matches_root_and_nested_card_layouts() {
        let root = hdmi_navigation_geometry(
            960,
            540,
            1,
            0,
            true,
            NavigationTransitionEdge::HomeToConsoles,
            "Consoles",
        );
        assert_eq!(
            root.source_card,
            NavigationTransitionRect {
                x: 253,
                y: 74,
                width: 219,
                height: 448,
            }
        );

        let nested = hdmi_navigation_geometry(
            960,
            540,
            0,
            0,
            false,
            NavigationTransitionEdge::ConsolesToSystem,
            "Atari",
        );
        assert_eq!(
            nested.source_card,
            NavigationTransitionRect {
                x: 18,
                y: 74,
                width: 191,
                height: 448,
            }
        );
        assert_eq!(nested.destination_title.x, 16);
        assert_eq!(nested.destination_title.y, 16);
        assert_eq!(nested.destination_title.width, 120);
        assert_ne!(root.label_signature, nested.label_signature);
        assert_eq!(nested.label_signature, navigation_label_signature("ATARI"));
        let live_list_height =
            crate::arcade_list_renderer::ArcadeListGeometry::NORMAL.visible_height(540);
        let live_row_height = crate::arcade_catalog::ARCADE_ROW_HEIGHT as usize;
        let live_selected_y = 56 + (live_list_height / live_row_height / 2) * live_row_height;
        let live_footer_y = 56 + live_list_height + 4;
        let live_footer_height = 540 - live_footer_y - 8;
        assert_eq!(live_list_height, 452);
        assert_eq!(live_selected_y, 248);
        assert_eq!(
            nested.destination_list,
            NavigationTransitionRect {
                x: 8,
                y: 56,
                width: 510,
                height: 452,
            }
        );
        assert_eq!(
            nested.destination_selected_row,
            NavigationTransitionRect {
                x: 8,
                y: 248,
                width: 510,
                height: 48,
            }
        );
        assert_eq!(
            nested.destination_preview,
            NavigationTransitionRect {
                x: 560,
                y: 102,
                width: 320,
                height: 320,
            }
        );
        assert_eq!(
            nested.destination_footer,
            NavigationTransitionRect {
                x: 8,
                y: 512,
                width: 510,
                height: 20,
            }
        );
        assert_eq!(nested.destination_list.height as usize, live_list_height);
        assert_eq!(nested.destination_selected_row.y as usize, live_selected_y);
        assert_eq!(nested.destination_footer.y as usize, live_footer_y);
        assert_eq!(
            nested.destination_footer.height as usize,
            live_footer_height
        );
        assert_eq!(
            nested.destination_list.height % nested.destination_selected_row.height,
            20
        );
    }

    #[test]
    fn mac_preview_configuration_selects_debug_duration() {
        let mut poc = NavigationTransitionPoc::new(960, 540, true);
        poc.configure_preview(Some(4_000));
        assert_eq!(poc.duration_override_us, Some(4_000_000));
    }

    #[test]
    fn super_scaler_endpoints_are_exact_snapshots() {
        let mut poc = NavigationTransitionPoc::new(16, 12, true);
        let source = vec![Rgb565Pixel(0x1111); 16 * 12];
        let destination = vec![Rgb565Pixel(0x2222); 16 * 12];
        let geometry = NavigationTransitionGeometry {
            source_card: NavigationTransitionRect {
                x: 2,
                y: 2,
                width: 4,
                height: 8,
            },
            source_label: NavigationTransitionRect {
                x: 2,
                y: 4,
                width: 4,
                height: 2,
            },
            destination_title: NavigationTransitionRect {
                x: 1,
                y: 1,
                width: 8,
                height: 2,
            },
            ..NavigationTransitionGeometry::default()
        };
        poc.begin(
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionDirection::Forward,
            geometry,
            &source,
            0,
        )
        .unwrap();
        assert_eq!(poc.render().unwrap(), source);
        poc.capture_destination(&destination).unwrap();
        poc.tick(NavigationTransitionEdge::HomeToConsoles.duration_us());
        assert_eq!(poc.render().unwrap(), destination);

        let mut reverse = NavigationTransitionPoc::new(16, 12, true);
        reverse
            .begin(
                NavigationTransitionEdge::HomeToConsoles,
                NavigationTransitionDirection::Reverse,
                geometry,
                &destination,
                0,
            )
            .unwrap();
        assert_eq!(reverse.render().unwrap(), destination);
        reverse.capture_destination(&source).unwrap();
        reverse.tick(NavigationTransitionEdge::HomeToConsoles.duration_us());
        assert_eq!(reverse.render().unwrap(), source);

        let mut cancelled = NavigationTransitionPoc::new(16, 12, true);
        cancelled
            .begin(
                NavigationTransitionEdge::HomeToConsoles,
                NavigationTransitionDirection::Forward,
                geometry,
                &source,
                0,
            )
            .unwrap();
        cancelled.tick(100_000);
        assert!(cancelled.request_reverse(100_000));
        cancelled.tick(300_000);
        assert_eq!(
            cancelled.frame().endpoint,
            Some(NavigationTransitionEndpoint::Source)
        );
        assert_eq!(cancelled.render().unwrap(), source);
    }

    #[test]
    fn crt_overlay_sweeps_holds_clears_and_preserves_endpoints() {
        let width = 12;
        let height = 10;
        let original = vec![Rgb565Pixel(0xffff); width * height];
        let full_phosphor_pixels = (height / 2 * width) as u64;
        for (progress, expected_full) in [
            (0, false),
            (CRT_SWEEP_END_Q16 / 2, false),
            (CRT_SWEEP_END_Q16, true),
            (PROGRESS_MAX / 2, true),
            (CRT_CLEAR_START_Q16, true),
            (
                ((CRT_CLEAR_START_Q16 as u32 + PROGRESS_MAX as u32) / 2) as u16,
                false,
            ),
            (PROGRESS_MAX, false),
        ] {
            let mut pixels = original.clone();
            let mut stats = NavigationTransitionRenderStats::default();
            apply_crt_scanline_overlay(
                &mut pixels,
                width,
                height,
                NavigationTransitionFrame {
                    phase: NavigationTransitionPhase::Reveal,
                    progress_q16: progress,
                    ..NavigationTransitionFrame::default()
                },
                ScanlineKernel::Scalar,
                &mut stats,
            );
            if progress == 0 || progress == PROGRESS_MAX {
                assert_eq!(pixels, original);
                assert_eq!(stats.phosphor_pixels, 0);
                assert_eq!(stats.scanline_pixels, 0);
            } else if expected_full {
                assert_eq!(stats.phosphor_pixels, full_phosphor_pixels);
                assert_eq!(stats.scanline_pixels, 0);
            } else {
                assert!(stats.phosphor_pixels < full_phosphor_pixels);
                assert!(stats.scanline_pixels <= width as u64 * 5);
            }
        }

        let mut reversing = original.clone();
        let mut stats = NavigationTransitionRenderStats::default();
        apply_crt_scanline_overlay(
            &mut reversing,
            width,
            height,
            NavigationTransitionFrame {
                phase: NavigationTransitionPhase::Reversing,
                progress_q16: 20_000,
                reverse_origin_q16: PROGRESS_MAX / 2,
                reverse_leg_progress_q16: PROGRESS_MAX / 2,
                ..NavigationTransitionFrame::default()
            },
            ScanlineKernel::Scalar,
            &mut stats,
        );
        assert_eq!(stats.phosphor_pixels, full_phosphor_pixels);
        assert_eq!(stats.scanline_pixels, 0);
    }

    #[test]
    fn disabled_poc_does_not_allocate_frame_buffers() {
        let poc = NavigationTransitionPoc::new(960, 540, false);
        assert!(!poc.enabled());
        assert!(poc.buffers.source.is_empty());
        assert!(poc.buffers.destination.is_empty());
        assert!(poc.buffers.working.is_empty());
        assert!(poc.hud_scratch.is_empty());
    }
}
