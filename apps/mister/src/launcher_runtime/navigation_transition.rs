// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral navigation-transition state and RGB565 frame ownership.

use slint::platform::software_renderer::Rgb565Pixel;
use std::time::Instant;

const PROGRESS_MAX: u16 = u16::MAX;
const COVER_PROGRESS: u16 = 36_044;
const DEFAULT_PREPARATION_TIMEOUT_US: u64 = 5_000_000;
const HUD_WIDTH: usize = 286;
const HUD_HEIGHT: usize = 28;
const HUD_MARGIN: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationTransitionStyle {
    SuperScalerShell,
    SpriteFoundry,
    NeonCabinetDive,
    CrtCabinetBoot,
    CharacterRomRecompile,
}

impl NavigationTransitionStyle {
    pub const ALL: [Self; 5] = [
        Self::SuperScalerShell,
        Self::SpriteFoundry,
        Self::NeonCabinetDive,
        Self::CrtCabinetBoot,
        Self::CharacterRomRecompile,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::SuperScalerShell => "super-scaler-shell",
            Self::SpriteFoundry => "sprite-foundry",
            Self::NeonCabinetDive => "neon-cabinet-dive",
            Self::CrtCabinetBoot => "crt-cabinet-boot",
            Self::CharacterRomRecompile => "character-rom-recompile",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.to_ascii_lowercase().replace('_', "-");
        Self::ALL
            .iter()
            .copied()
            .find(|style| style.label() == normalized)
    }

    pub const fn duration_us(self, edge: NavigationTransitionEdge) -> u64 {
        match self {
            Self::SuperScalerShell if edge.enters_system_browser() => 480_000,
            Self::SuperScalerShell => 420_000,
            Self::SpriteFoundry => 500_000,
            Self::NeonCabinetDive => 417_000,
            Self::CrtCabinetBoot => 480_000,
            Self::CharacterRomRecompile => 433_000,
        }
    }
}

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
    NavigationTransitionGeometry {
        source_card,
        source_label,
        destination_title,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NavigationTransitionGeometry {
    pub source_card: NavigationTransitionRect,
    pub source_label: NavigationTransitionRect,
    pub destination_title: NavigationTransitionRect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationTransitionRequest {
    pub style: NavigationTransitionStyle,
    pub edge: NavigationTransitionEdge,
    pub direction: NavigationTransitionDirection,
    pub geometry: NavigationTransitionGeometry,
    pub preparation_timeout_us: u64,
}

impl NavigationTransitionRequest {
    pub const fn new(
        style: NavigationTransitionStyle,
        edge: NavigationTransitionEdge,
        direction: NavigationTransitionDirection,
        geometry: NavigationTransitionGeometry,
    ) -> Self {
        Self {
            style,
            edge,
            direction,
            geometry,
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
        }
    }
}

#[derive(Debug, Default)]
pub struct NavigationTransitionController {
    phase: NavigationTransitionPhase,
    request: Option<NavigationTransitionRequest>,
    phase_started_us: u64,
    covered_started_us: u64,
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
        let total_us = request.style.duration_us(request.edge).max(1);
        let cover_us = total_us.saturating_mul(COVER_PROGRESS as u64) / PROGRESS_MAX as u64;

        if self.phase == NavigationTransitionPhase::Expand {
            let elapsed = now_us.saturating_sub(self.phase_started_us);
            self.progress_q16 = scale_progress(elapsed, cover_us, COVER_PROGRESS);
            if elapsed >= cover_us {
                self.progress_q16 = COVER_PROGRESS;
                self.phase = NavigationTransitionPhase::Covered;
                self.covered_started_us = self.phase_started_us.saturating_add(cover_us);
                self.phase_started_us = self.covered_started_us;
                if destination_ready {
                    self.phase = NavigationTransitionPhase::Reveal;
                }
            }
        }
        if self.phase == NavigationTransitionPhase::Covered {
            if destination_ready {
                self.telemetry.covered_hold_us = now_us.saturating_sub(self.covered_started_us);
                self.phase = NavigationTransitionPhase::Reveal;
                self.phase_started_us = now_us;
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
            let reveal_progress = scale_progress(elapsed, reveal_us, PROGRESS_MAX - COVER_PROGRESS);
            self.progress_q16 = COVER_PROGRESS.saturating_add(reveal_progress);
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
        let endpoint = if self.progress_q16 >= COVER_PROGRESS && destination_ready {
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
        let cover_progress_q16 = if self.progress_q16 >= COVER_PROGRESS {
            PROGRESS_MAX
        } else {
            ((self.progress_q16 as u32 * PROGRESS_MAX as u32) / COVER_PROGRESS as u32) as u16
        };
        let reveal_progress_q16 = if self.progress_q16 <= COVER_PROGRESS {
            0
        } else {
            (((self.progress_q16 - COVER_PROGRESS) as u32 * PROGRESS_MAX as u32)
                / (PROGRESS_MAX - COVER_PROGRESS) as u32) as u16
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
}

#[derive(Debug)]
pub struct NavigationTransitionPoc {
    enabled: bool,
    style_index: usize,
    controller: NavigationTransitionController,
    buffers: NavigationTransitionBuffers,
    geometry_history: [Option<NavigationTransitionGeometry>; 3],
    last_render_stats: NavigationTransitionRenderStats,
    last_frame_work_us: u64,
    hud_scratch: Vec<Rgb565Pixel>,
}

impl NavigationTransitionPoc {
    pub fn from_env(width: usize, height: usize) -> Self {
        let style_index = std::env::var("MISTER_NAV_TRANSITION_STYLE")
            .ok()
            .and_then(|value| NavigationTransitionStyle::parse(&value))
            .and_then(|style| {
                NavigationTransitionStyle::ALL
                    .iter()
                    .position(|item| *item == style)
            })
            .unwrap_or(0)
            .min(Self::implemented_style_count().saturating_sub(1));
        Self::new_with_style(
            width,
            height,
            env_flag("MISTER_NAV_TRANSITION_POC"),
            style_index,
        )
    }

    pub fn new(width: usize, height: usize, enabled: bool) -> Self {
        Self::new_with_style(width, height, enabled, 0)
    }

    fn new_with_style(width: usize, height: usize, enabled: bool, style_index: usize) -> Self {
        let (buffer_width, buffer_height) = if enabled { (width, height) } else { (0, 0) };
        Self {
            enabled,
            style_index,
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

    pub const fn implemented_style_count() -> usize {
        1
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn style(&self) -> NavigationTransitionStyle {
        NavigationTransitionStyle::ALL[self.style_index]
    }

    pub const fn style_index(&self) -> usize {
        self.style_index
    }

    pub fn cycle_style(&mut self, delta: i32) -> bool {
        if !self.enabled || self.controller.is_active() {
            return false;
        }
        let count = Self::implemented_style_count();
        if count <= 1 {
            return false;
        }
        self.style_index = (self.style_index as i32 + delta).rem_euclid(count as i32) as usize;
        true
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
        let request = NavigationTransitionRequest::new(self.style(), edge, direction, geometry);
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
        let mut stats = match request.style {
            NavigationTransitionStyle::SuperScalerShell => {
                render_super_scaler_shell(&mut self.buffers, request, frame)?
            }
            _ => return Err(NavigationTransitionFailure::SnapshotSizeMismatch),
        };
        stats.render_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
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
    if working.len() != source.len() {
        return Err(NavigationTransitionFailure::SnapshotSizeMismatch);
    }
    working.copy_from_slice(source);
    let mut stats = NavigationTransitionRenderStats {
        copied_pixels: source.len() as u64,
        ..NavigationTransitionRenderStats::default()
    };
    let width = buffers.width;
    let height = buffers.height;
    let full = NavigationTransitionRect {
        x: 0,
        y: 0,
        width: width.min(u16::MAX as usize) as u16,
        height: height.min(u16::MAX as usize) as u16,
    };
    let shell = Rgb565Pixel(0x18c8);
    let mint = Rgb565Pixel(0x06b4);
    let purple = Rgb565Pixel(0x72ae);

    if frame.phase == NavigationTransitionPhase::Settled {
        match frame.endpoint {
            Some(NavigationTransitionEndpoint::Source) => return Ok(stats),
            Some(NavigationTransitionEndpoint::Destination) => {
                if let Some(destination) = destination {
                    working.copy_from_slice(destination);
                    stats.copied_pixels =
                        stats.copied_pixels.saturating_add(destination.len() as u64);
                }
                return Ok(stats);
            }
            None => {}
        }
    }

    match request.direction {
        NavigationTransitionDirection::Forward => {
            if frame.phase == NavigationTransitionPhase::Reveal {
                fill_rect_565(working, width, height, full, shell, &mut stats);
                if let Some(destination) = destination {
                    reveal_destination_bands(
                        working,
                        destination,
                        width,
                        height,
                        frame.reveal_progress_q16,
                        request.edge.enters_system_browser(),
                        &mut stats,
                    );
                }
            }
            let cover = ease_out_cubic_q16(frame.cover_progress_q16);
            let rect = lerp_rect(request.geometry.source_card, full, cover);
            if frame.phase != NavigationTransitionPhase::Reveal {
                fill_rect_565(working, width, height, rect, shell, &mut stats);
                for echo in 1..=3 {
                    let delayed = cover.saturating_sub((echo * 4_000) as u16);
                    let echo_rect = lerp_rect(request.geometry.source_card, full, delayed);
                    draw_outline_565(
                        working,
                        width,
                        height,
                        echo_rect,
                        if echo == 1 { mint } else { purple },
                        &mut stats,
                    );
                }
                move_label_pixels(
                    working,
                    source,
                    width,
                    height,
                    request.geometry.source_label,
                    request.geometry.destination_title,
                    cover,
                    false,
                    &mut stats,
                );
            } else if frame.reveal_progress_q16 < PROGRESS_MAX {
                move_label_pixels(
                    working,
                    source,
                    width,
                    height,
                    request.geometry.source_label,
                    request.geometry.destination_title,
                    PROGRESS_MAX,
                    false,
                    &mut stats,
                );
            }
        }
        NavigationTransitionDirection::Reverse => {
            let cover = ease_out_cubic_q16(frame.cover_progress_q16);
            if frame.phase != NavigationTransitionPhase::Reveal {
                let covered_rows = height.saturating_mul(cover as usize) / PROGRESS_MAX as usize;
                let y0 = height.saturating_sub(covered_rows) / 2;
                fill_rect_565(
                    working,
                    width,
                    height,
                    NavigationTransitionRect {
                        x: 0,
                        y: y0 as u16,
                        width: width as u16,
                        height: covered_rows as u16,
                    },
                    shell,
                    &mut stats,
                );
                draw_outline_565(working, width, height, full, mint, &mut stats);
            } else if let Some(destination) = destination {
                working.copy_from_slice(destination);
                stats.copied_pixels = stats.copied_pixels.saturating_add(destination.len() as u64);
                let shrink = ease_out_cubic_q16(frame.reveal_progress_q16);
                let rect = lerp_rect(full, request.geometry.source_card, shrink);
                fill_rect_565(working, width, height, rect, shell, &mut stats);
                draw_outline_565(working, width, height, rect, mint, &mut stats);
                for echo in 1..=3 {
                    let delayed = shrink.saturating_sub((echo * 3_000) as u16);
                    let echo_rect = lerp_rect(full, request.geometry.source_card, delayed);
                    draw_outline_565(working, width, height, echo_rect, purple, &mut stats);
                }
                move_label_pixels(
                    working,
                    source,
                    width,
                    height,
                    request.geometry.destination_title,
                    request.geometry.source_label,
                    shrink,
                    true,
                    &mut stats,
                );
            }
        }
    }
    Ok(stats)
}

fn reveal_destination_bands(
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    progress_q16: u16,
    system_browser: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    if working.len() != destination.len() || width == 0 {
        return;
    }
    let bands = if system_browser { 12 } else { 8 };
    for band in 0..bands {
        let y0 = band * height / bands;
        let y1 = (band + 1) * height / bands;
        let delay = band * 2_200;
        let local = progress_q16.saturating_sub(delay as u16);
        let local = ((local as u32 * PROGRESS_MAX as u32)
            / (PROGRESS_MAX as u32).saturating_sub(delay as u32).max(1))
        .min(PROGRESS_MAX as u32) as u16;
        let rise = 24usize.saturating_mul((PROGRESS_MAX - local) as usize) / PROGRESS_MAX as usize;
        let visible_rows = (y1 - y0).saturating_mul(local as usize) / PROGRESS_MAX as usize;
        for row in 0..visible_rows {
            let dst_y = y1.saturating_sub(visible_rows).saturating_add(row);
            let src_y = dst_y.saturating_add(rise).min(height.saturating_sub(1));
            let dst_start = dst_y * width;
            let src_start = src_y * width;
            working[dst_start..dst_start + width]
                .copy_from_slice(&destination[src_start..src_start + width]);
            stats.copied_pixels = stats.copied_pixels.saturating_add(width as u64);
        }
    }
    if progress_q16 == PROGRESS_MAX {
        working.copy_from_slice(destination);
    }
}

fn move_label_pixels(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    from: NavigationTransitionRect,
    to: NavigationTransitionRect,
    progress_q16: u16,
    reverse: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some((content, background)) = opaque_content_bounds(source, width, height, from) else {
        return;
    };
    let target_height = if reverse {
        (content.height as u32 * 2 / 3).max(1) as u16
    } else {
        (content.height as u32 * 3 / 2)
            .min(to.height.max(1) as u32)
            .max(1) as u16
    };
    let target_width = ((content.width as u32 * target_height as u32)
        / content.height.max(1) as u32)
        .min(to.width.max(1) as u32)
        .max(1) as u16;
    let target = NavigationTransitionRect {
        x: if reverse {
            to.x.saturating_add(to.width.saturating_sub(target_width) / 2)
        } else {
            to.x
        },
        y: if reverse {
            to.y.saturating_add(to.height.saturating_sub(target_height) / 2)
        } else {
            to.y
        },
        width: target_width,
        height: target_height,
    };
    let moving = lerp_rect(content, target, progress_q16);
    blit_scaled_masked_565(
        working, source, width, height, content, moving, background, stats,
    );
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
    let source_w = source_rect.width as usize;
    let source_h = source_rect.height as usize;
    let target_w = target_rect.width as usize;
    let target_h = target_rect.height as usize;
    if source_w == 0 || source_h == 0 || target_w == 0 || target_h == 0 {
        return;
    }
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
                if pixel != transparent {
                    destination[dy * width + dx] = pixel;
                    stats.copied_pixels = stats.copied_pixels.saturating_add(1);
                }
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

fn ease_out_cubic_q16(progress_q16: u16) -> u16 {
    let inverse = PROGRESS_MAX as u64 - progress_q16 as u64;
    let cubic = inverse.saturating_mul(inverse).saturating_mul(inverse)
        / (PROGRESS_MAX as u64).saturating_mul(PROGRESS_MAX as u64);
    (PROGRESS_MAX as u64)
        .saturating_sub(cubic)
        .min(PROGRESS_MAX as u64) as u16
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
        }
    }

    fn request() -> NavigationTransitionRequest {
        NavigationTransitionRequest::new(
            NavigationTransitionStyle::SuperScalerShell,
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionDirection::Forward,
            geometry(),
        )
    }

    #[test]
    fn style_labels_parse_and_keep_intended_durations() {
        for style in NavigationTransitionStyle::ALL {
            assert_eq!(NavigationTransitionStyle::parse(style.label()), Some(style));
            assert_eq!(
                NavigationTransitionStyle::parse(&style.label().replace('-', "_")),
                Some(style)
            );
        }
        assert_eq!(
            NavigationTransitionStyle::SuperScalerShell
                .duration_us(NavigationTransitionEdge::HomeToConsoles),
            420_000
        );
        assert_eq!(
            NavigationTransitionStyle::SuperScalerShell
                .duration_us(NavigationTransitionEdge::HomeToArcade),
            480_000
        );
    }

    #[test]
    fn transition_waits_covered_until_destination_is_ready() {
        let mut controller = NavigationTransitionController::default();
        assert!(controller.begin(request(), 1_000));
        assert!(controller.captured(2_000, 300));

        let covered = controller.tick(300_000, false);
        assert_eq!(covered.phase, NavigationTransitionPhase::Covered);
        assert_eq!(covered.progress_q16, COVER_PROGRESS);

        let still_covered = controller.tick(500_000, false);
        assert_eq!(still_covered.phase, NavigationTransitionPhase::Covered);

        let reveal = controller.tick(510_000, true);
        assert_eq!(reveal.phase, NavigationTransitionPhase::Reveal);
        assert_eq!(controller.telemetry().covered_hold_us, 277_002);
    }

    #[test]
    fn completed_transition_settles_at_destination() {
        let mut controller = NavigationTransitionController::default();
        controller.begin(request(), 0);
        controller.captured(0, 0);
        controller.tick(300_000, true);
        controller.tick(300_001, true);
        let settled = controller.tick(700_000, true);

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
        controller.begin(request(), 0);
        controller.captured(0, 0);

        let settled = controller.tick(420_000, true);

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
        let mut controller = NavigationTransitionController::default();
        controller.begin(timed, 0);
        controller.captured(0, 0);
        controller.tick(300_000, false);
        let reversing = controller.tick(360_000, false);

        assert_eq!(reversing.phase, NavigationTransitionPhase::Reversing);
        assert_eq!(
            reversing.failure,
            Some(NavigationTransitionFailure::DestinationTimeout)
        );
        let settled = controller.tick(800_000, false);
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
        let mut controller = NavigationTransitionController::default();
        controller.begin(timed, 0);
        controller.captured(0, 0);
        controller.queue_input(NavigationTransitionInput::Activate);
        controller.tick(300_000, false);
        controller.tick(360_000, false);
        controller.tick(800_000, false);

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
        covered_unready.tick(300_000, false);
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
        covered_ready.tick(300_000, true);
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
        poc.tick(420_000);
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
        reverse.tick(420_000);
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
    fn disabled_poc_does_not_allocate_frame_buffers() {
        let poc = NavigationTransitionPoc::new(960, 540, false);
        assert!(!poc.enabled());
        assert!(poc.buffers.source.is_empty());
        assert!(poc.buffers.destination.is_empty());
        assert!(poc.buffers.working.is_empty());
        assert!(poc.hud_scratch.is_empty());
    }
}
