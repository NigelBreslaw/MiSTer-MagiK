// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral navigation-transition state and RGB565 frame ownership.

use crate::input_state::PadState;
use mister_magik_framebuffer_scenes::Rgb565Pixel as SharedRgb565Pixel;
pub use mister_magik_framebuffer_scenes::navigation::{
    CrtNavigationLayout, NavigationTransitionBuffers, NavigationTransitionCompletion,
    NavigationTransitionDirection, NavigationTransitionEdge, NavigationTransitionEndpoint,
    NavigationTransitionFailure, NavigationTransitionFrame, NavigationTransitionGeometry,
    NavigationTransitionPhase, NavigationTransitionRect, NavigationTransitionRenderStats,
    NavigationTransitionRequest, crt_navigation_geometry, hdmi_navigation_geometry,
};
use mister_magik_framebuffer_scenes::navigation::{
    PROGRESS_MAX, SUPER_SCALER_COVER_PROGRESS, forward_progress_q16_at_elapsed,
    render_navigation_transition, request_cover_progress_q16, scale_progress,
    warm_navigation_transition_rasterizer,
};
use slint::platform::software_renderer::Rgb565Pixel;
use std::collections::VecDeque;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationTransitionRoute {
    HomeToConsoles,
    HomeToArcade,
    ConsolesToSystem,
    HomeToSettings,
    SettingsToScreensaver,
    SettingsToAbout,
    AboutToInfo,
    AboutToLicenses,
    NestedToHome,
}

impl NavigationTransitionRoute {
    pub const fn from_super_scaler_edge(edge: NavigationTransitionEdge) -> Self {
        match edge {
            NavigationTransitionEdge::HomeToConsoles => Self::HomeToConsoles,
            NavigationTransitionEdge::HomeToArcade => Self::HomeToArcade,
            NavigationTransitionEdge::ConsolesToSystem => Self::ConsolesToSystem,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::HomeToConsoles => "home-consoles",
            Self::HomeToArcade => "home-arcade",
            Self::ConsolesToSystem => "consoles-system",
            Self::HomeToSettings => "home-settings",
            Self::SettingsToScreensaver => "settings-screensaver",
            Self::SettingsToAbout => "settings-about",
            Self::AboutToInfo => "about-info",
            Self::AboutToLicenses => "about-licenses",
            Self::NestedToHome => "nested-home",
        }
    }

    pub const fn renderer(self) -> &'static str {
        match self {
            Self::HomeToSettings
            | Self::SettingsToScreensaver
            | Self::SettingsToAbout
            | Self::AboutToInfo
            | Self::AboutToLicenses
            | Self::NestedToHome => "settings-page",
            Self::HomeToConsoles | Self::HomeToArcade | Self::ConsolesToSystem => "super-scaler",
        }
    }

    pub const fn is_settings_page(self) -> bool {
        matches!(
            self,
            Self::HomeToSettings
                | Self::SettingsToScreensaver
                | Self::SettingsToAbout
                | Self::AboutToInfo
                | Self::AboutToLicenses
                | Self::NestedToHome
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NavigationTransitionInput {
    pub activate: bool,
    pub back: bool,
    pub home: bool,
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
}

impl NavigationTransitionInput {
    pub fn rising_edges(now: &PadState, previous: &PadState) -> Option<Self> {
        let input = Self {
            activate: now.btn_a && !previous.btn_a,
            back: now.btn_b && !previous.btn_b,
            home: now.btn_home && !previous.btn_home,
            up: now.dpad_up && !previous.dpad_up,
            down: now.dpad_down && !previous.dpad_down,
            left: now.dpad_left && !previous.dpad_left,
            right: now.dpad_right && !previous.dpad_right,
        };
        (!input.is_empty()).then_some(input)
    }

    pub const fn is_empty(self) -> bool {
        !self.activate
            && !self.back
            && !self.home
            && !self.up
            && !self.down
            && !self.left
            && !self.right
    }

    pub fn without_back_or_home(mut self) -> Option<Self> {
        self.back = false;
        self.home = false;
        (!self.is_empty()).then_some(self)
    }

    pub fn replay(self, physical: &PadState) -> (PadState, PadState) {
        let mut previous = physical.clone();
        let mut now = physical.clone();
        for (queued, previous_field, now_field) in [
            (self.activate, &mut previous.btn_a, &mut now.btn_a),
            (self.back, &mut previous.btn_b, &mut now.btn_b),
            (self.home, &mut previous.btn_home, &mut now.btn_home),
            (self.up, &mut previous.dpad_up, &mut now.dpad_up),
            (self.down, &mut previous.dpad_down, &mut now.dpad_down),
            (self.left, &mut previous.dpad_left, &mut now.dpad_left),
            (self.right, &mut previous.dpad_right, &mut now.dpad_right),
        ] {
            if queued {
                *previous_field = false;
                *now_field = true;
            }
        }
        previous.rebuild_pressed_now();
        now.rebuild_pressed_now();
        (previous, now)
    }
}

#[derive(Clone, Debug)]
pub struct NavigationTransitionInputDelivery {
    pub previous: PadState,
    pub now: PadState,
    pub replayed: bool,
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
    pub status_quiesce_wait_us: u64,
    pub status_quiesce_timeout: bool,
}

impl NavigationTransitionTelemetry {
    pub fn note_render(&mut self, render_us: u64, reused: bool) {
        self.render_us = self.render_us.saturating_add(render_us);
        self.frames = self.frames.saturating_add(1);
        self.reused_frames = self.reused_frames.saturating_add(u64::from(reused));
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
        Some(endpoint)
    }

    pub fn settle_at_destination(&mut self) -> bool {
        if !self.is_active() {
            return false;
        }
        self.progress_q16 = PROGRESS_MAX;
        self.phase = NavigationTransitionPhase::Settled;
        self.failure = None;
        true
    }

    pub fn complete(&mut self) -> Option<NavigationTransitionCompletion> {
        if self.phase != NavigationTransitionPhase::Settled {
            return None;
        }
        let endpoint = self.endpoint()?;
        let completion = NavigationTransitionCompletion {
            endpoint,
            failure: self.failure,
        };
        self.phase = NavigationTransitionPhase::Idle;
        self.request = None;
        self.progress_q16 = 0;
        self.reverse_origin_q16 = 0;
        self.failure = None;
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

#[derive(Debug)]
pub struct NavigationTransitionRuntime {
    enabled: bool,
    duration_override_us: Option<u64>,
    controller: NavigationTransitionController,
    pending_request: Option<NavigationTransitionRequest>,
    pending_capture_us: u64,
    pending_started_us: u64,
    pending_prepare_started: Option<Instant>,
    pending_status_quiesce_us: u64,
    pending_status_quiesce_timeout: bool,
    queued_inputs: VecDeque<NavigationTransitionInput>,
    buffers: NavigationTransitionBuffers,
    geometry_history: Vec<(NavigationTransitionEdge, NavigationTransitionGeometry)>,
    last_render_stats: NavigationTransitionRenderStats,
    last_frame_work_us: u64,
    route: Option<NavigationTransitionRoute>,
}

impl NavigationTransitionRuntime {
    pub fn new(width: usize, height: usize, enabled: bool) -> Self {
        if enabled {
            warm_navigation_transition_rasterizer();
        }
        let (buffer_width, buffer_height) = if enabled { (width, height) } else { (0, 0) };
        Self {
            enabled,
            duration_override_us: None,
            controller: NavigationTransitionController::default(),
            pending_request: None,
            pending_capture_us: 0,
            pending_started_us: 0,
            pending_prepare_started: None,
            pending_status_quiesce_us: 0,
            pending_status_quiesce_timeout: false,
            queued_inputs: VecDeque::new(),
            buffers: NavigationTransitionBuffers::new(buffer_width, buffer_height),
            geometry_history: Vec::new(),
            last_render_stats: NavigationTransitionRenderStats::default(),
            last_frame_work_us: 0,
            route: None,
        }
    }

    pub fn configure_preview(&mut self, duration_ms: Option<u64>) {
        self.duration_override_us =
            duration_ms.map(|milliseconds| milliseconds.clamp(100, 10_000).saturating_mul(1_000));
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, width: usize, height: usize, enabled: bool) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        self.pending_request = None;
        self.controller = NavigationTransitionController::default();
        self.queued_inputs.clear();
        self.geometry_history.clear();
        self.route = None;
        if enabled {
            warm_navigation_transition_rasterizer();
            self.buffers.resize(width, height);
        } else {
            self.buffers.resize(0, 0);
        }
    }

    pub fn begin(
        &mut self,
        edge: NavigationTransitionEdge,
        direction: NavigationTransitionDirection,
        geometry: NavigationTransitionGeometry,
        source: &[Rgb565Pixel],
        now_us: u64,
    ) -> Result<bool, NavigationTransitionFailure> {
        let mut request = NavigationTransitionRequest::new(edge, direction, geometry);
        if let Some(duration_us) = self.duration_override_us {
            request.duration_us = duration_us;
        }
        let started = self.begin_request(request, source, now_us)?;
        if started {
            self.route = Some(NavigationTransitionRoute::from_super_scaler_edge(edge));
        }
        if started && direction == NavigationTransitionDirection::Forward {
            self.geometry_history.push((edge, geometry));
        }
        Ok(started)
    }

    pub fn begin_settings_page(
        &mut self,
        route: NavigationTransitionRoute,
        direction: NavigationTransitionDirection,
        source: &[Rgb565Pixel],
        now_us: u64,
    ) -> Result<bool, NavigationTransitionFailure> {
        if !route.is_settings_page() {
            return Ok(false);
        }
        let started = self.begin_settings_page_request(
            NavigationTransitionRequest::settings_page(direction),
            source,
            now_us,
        )?;
        if started {
            self.route = Some(route);
        }
        Ok(started)
    }

    fn begin_settings_page_request(
        &mut self,
        mut request: NavigationTransitionRequest,
        source: &[Rgb565Pixel],
        now_us: u64,
    ) -> Result<bool, NavigationTransitionFailure> {
        if let Some(duration_us) = self.duration_override_us {
            request.duration_us = duration_us;
        }
        self.begin_request(request, source, now_us)
    }

    fn begin_request(
        &mut self,
        request: NavigationTransitionRequest,
        source: &[Rgb565Pixel],
        now_us: u64,
    ) -> Result<bool, NavigationTransitionFailure> {
        if !self.enabled || self.is_active() {
            return Ok(false);
        }
        self.buffers.begin_capture();
        let capture_started = Instant::now();
        self.buffers
            .capture_source(slint_rgb565_as_shared(source))?;
        let capture_us = capture_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        self.pending_request = Some(request);
        self.pending_capture_us = capture_us;
        self.pending_started_us = now_us;
        self.pending_prepare_started = Some(Instant::now());
        self.pending_status_quiesce_us = 0;
        self.pending_status_quiesce_timeout = false;
        Ok(true)
    }

    pub fn geometry_for_reverse(
        &self,
        edge: NavigationTransitionEdge,
    ) -> Option<NavigationTransitionGeometry> {
        self.geometry_history
            .last()
            .filter(|(history_edge, _)| *history_edge == edge)
            .map(|(_, geometry)| *geometry)
    }

    pub fn capture_destination(
        &mut self,
        destination: &[Rgb565Pixel],
        now_us: u64,
    ) -> Result<(), NavigationTransitionFailure> {
        self.buffers
            .capture_destination(slint_rgb565_as_shared(destination))?;
        let prepare_us = self
            .pending_prepare_started
            .take()
            .map(|started| started.elapsed().as_micros().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        if self.activate_pending(now_us) {
            self.controller.note_destination_prepared(prepare_us);
        }
        Ok(())
    }

    pub fn tick(&mut self, now_us: u64) -> NavigationTransitionFrame {
        if let Some(request) = self.pending_request {
            if now_us.saturating_sub(self.pending_started_us) >= request.preparation_timeout_us {
                self.activate_pending(now_us);
                self.controller
                    .fail(NavigationTransitionFailure::DestinationTimeout, now_us);
                return self.controller.frame();
            }
            return self.frame();
        }
        self.controller
            .tick(now_us, self.buffers.destination_ready())
    }

    pub fn render(&mut self) -> Result<&[Rgb565Pixel], NavigationTransitionFailure> {
        let started = Instant::now();
        let Some(request) = self.request() else {
            return Err(NavigationTransitionFailure::SnapshotSizeMismatch);
        };
        let frame = self.frame();
        let pending = self.pending_request.is_some();
        let mut stats = if pending {
            let copied_pixels = self.buffers.copy_source_to_working()?;
            NavigationTransitionRenderStats {
                copied_pixels: copied_pixels as u64,
                ..NavigationTransitionRenderStats::default()
            }
        } else {
            render_navigation_transition(&mut self.buffers, request, frame)?
        };
        if pending {
            stats.render_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        }
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
        Ok(shared_rgb565_as_slint(self.buffers.working()))
    }

    pub fn request_reverse(&mut self, now_us: u64) -> bool {
        if self.pending_request.is_some() {
            self.activate_pending(now_us);
        }
        self.controller.request_reverse(now_us)
    }

    pub fn settle_at_destination(&mut self) -> bool {
        if self.pending_request.is_some() {
            self.activate_pending(self.pending_started_us);
        }
        self.controller.settle_at_destination()
    }

    pub fn queue_input(&mut self, input: NavigationTransitionInput) {
        if !input.is_empty() {
            self.queued_inputs.push_back(input);
        }
    }

    pub fn take_queued_input(&mut self) -> Option<NavigationTransitionInput> {
        (!self.is_active())
            .then(|| self.queued_inputs.pop_front())
            .flatten()
    }

    pub fn clear_queued_inputs(&mut self) {
        self.queued_inputs.clear();
    }

    pub fn route_input(
        &mut self,
        physical_now: &PadState,
        physical_previous: &PadState,
        consumed_back_or_home: bool,
    ) -> Option<NavigationTransitionInputDelivery> {
        let physical_input =
            NavigationTransitionInput::rising_edges(physical_now, physical_previous).and_then(
                |input| {
                    if consumed_back_or_home {
                        input.without_back_or_home()
                    } else {
                        Some(input)
                    }
                },
            );
        if self.is_active() {
            if let Some(input) = physical_input {
                self.queue_input(input);
            }
            return None;
        }
        if let Some(queued_input) = self.take_queued_input() {
            if let Some(input) = physical_input {
                self.queue_input(input);
            }
            let (previous, now) = queued_input.replay(physical_now);
            return Some(NavigationTransitionInputDelivery {
                previous,
                now,
                replayed: true,
            });
        }
        if consumed_back_or_home {
            let (previous, now) = physical_input.map_or_else(
                || (physical_now.clone(), physical_now.clone()),
                |input| input.replay(physical_now),
            );
            return Some(NavigationTransitionInputDelivery {
                previous,
                now,
                replayed: true,
            });
        }
        Some(NavigationTransitionInputDelivery {
            previous: physical_previous.clone(),
            now: physical_now.clone(),
            replayed: false,
        })
    }

    pub fn cancel_for_exclusive_view(&mut self) -> Option<NavigationTransitionEndpoint> {
        self.queued_inputs.clear();
        if self.pending_request.is_some() {
            self.activate_pending(self.pending_started_us);
        }
        self.controller
            .cancel_for_exclusive_view(self.buffers.destination_ready())
    }

    pub fn complete(&mut self) -> Option<NavigationTransitionCompletion> {
        let request = self.request();
        let completion = self.controller.complete()?;
        if completion.failure.is_some() {
            self.queued_inputs.clear();
        }
        if request.is_some_and(|request| {
            request.is_super_scaler()
                && matches!(
                    (request.direction, completion.endpoint),
                    (
                        NavigationTransitionDirection::Forward,
                        NavigationTransitionEndpoint::Source
                    ) | (
                        NavigationTransitionDirection::Reverse,
                        NavigationTransitionEndpoint::Destination
                    )
                )
        }) {
            self.geometry_history.pop();
        }
        Some(completion)
    }

    pub fn clear_geometry_history(&mut self) {
        self.geometry_history.clear();
    }

    pub fn frame(&self) -> NavigationTransitionFrame {
        if self.pending_request.is_some() {
            return NavigationTransitionFrame {
                phase: NavigationTransitionPhase::Capture,
                owns_full_frame: true,
                ..NavigationTransitionFrame::default()
            };
        }
        self.controller.frame()
    }

    pub fn request(&self) -> Option<NavigationTransitionRequest> {
        self.pending_request.or_else(|| self.controller.request())
    }

    pub const fn route(&self) -> Option<NavigationTransitionRoute> {
        self.route
    }

    pub const fn is_active(&self) -> bool {
        self.pending_request.is_some() || self.controller.is_active()
    }

    pub const fn destination_ready(&self) -> bool {
        self.buffers.destination_ready()
    }

    /// The cached source and destination now own every visible transition pixel.
    /// Slint must retain its pending redraw without advancing or rasterizing until
    /// this playback settles.
    pub const fn snapshot_locked(&self) -> bool {
        self.is_active() && self.pending_request.is_none() && self.buffers.destination_ready()
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

    pub fn note_pending_status_quiesce(&mut self, wait_us: u64, timed_out: bool) {
        self.pending_status_quiesce_us = wait_us;
        self.pending_status_quiesce_timeout = timed_out;
    }

    pub const fn telemetry(&self) -> NavigationTransitionTelemetry {
        self.controller.telemetry()
    }

    fn activate_pending(&mut self, now_us: u64) -> bool {
        let Some(request) = self.pending_request.take() else {
            return false;
        };
        self.pending_prepare_started = None;
        if !self.controller.begin(request, now_us) {
            return false;
        }
        let captured = self.controller.captured(now_us, self.pending_capture_us);
        if captured {
            let telemetry = self.controller.telemetry_mut();
            telemetry.status_quiesce_wait_us = self.pending_status_quiesce_us;
            telemetry.status_quiesce_timeout = self.pending_status_quiesce_timeout;
        }
        captured
    }
}

fn slint_rgb565_as_shared(pixels: &[Rgb565Pixel]) -> &[SharedRgb565Pixel] {
    assert_eq!(
        std::mem::size_of::<Rgb565Pixel>(),
        std::mem::size_of::<SharedRgb565Pixel>()
    );
    assert_eq!(
        std::mem::align_of::<Rgb565Pixel>(),
        std::mem::align_of::<SharedRgb565Pixel>()
    );
    // SAFETY: both RGB565 pixel types are transparent `u16` wrappers with equal
    // size/alignment, and the returned slice retains the input lifetime.
    unsafe { std::slice::from_raw_parts(pixels.as_ptr().cast::<SharedRgb565Pixel>(), pixels.len()) }
}

fn shared_rgb565_as_slint(pixels: &[SharedRgb565Pixel]) -> &[Rgb565Pixel] {
    assert_eq!(
        std::mem::size_of::<Rgb565Pixel>(),
        std::mem::size_of::<SharedRgb565Pixel>()
    );
    assert_eq!(
        std::mem::align_of::<Rgb565Pixel>(),
        std::mem::align_of::<SharedRgb565Pixel>()
    );
    // SAFETY: both RGB565 pixel types are transparent `u16` wrappers with equal
    // size/alignment, and the returned slice retains the input lifetime.
    unsafe { std::slice::from_raw_parts(pixels.as_ptr().cast::<Rgb565Pixel>(), pixels.len()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crt_navigation_geometry_stays_inside_every_supported_frame_shape() {
        for (frame_width, frame_height, content_x, content_y, content_width, content_height) in [
            (640, 480, 0, 0, 640, 480),
            (640, 480, 0, 20, 640, 255),
            (576, 576, 0, 0, 576, 576),
        ] {
            let layout = CrtNavigationLayout {
                content_x,
                content_y,
                content_width,
                content_height,
                grid_x: 4,
                grid_y: 4,
                header_height: 48,
                footer_height: 24,
                heading_font_height: 16,
                title_font_height: 16,
                detail_font_height: 8,
                game_row_height: 24,
            };
            for (selected, item_count, root_menu) in
                [(0, 4, true), (3, 4, true), (4, 9, false), (8, 9, false)]
            {
                let geometry = crt_navigation_geometry(
                    frame_width,
                    frame_height,
                    layout,
                    selected,
                    item_count,
                    root_menu,
                    NavigationTransitionEdge::ConsolesToSystem,
                    "Arcade",
                );
                for rect in [
                    geometry.source_card,
                    geometry.source_label,
                    geometry.source_detail,
                    geometry.destination_title,
                    geometry.destination_detail,
                    geometry.destination_list,
                    geometry.destination_selected_row,
                    geometry.destination_footer,
                ] {
                    assert!(rect.fits(frame_width, frame_height), "{rect:?}");
                }
                assert_eq!(
                    geometry.destination_preview,
                    NavigationTransitionRect::default()
                );
            }
        }
    }

    #[test]
    fn cancelled_settings_push_does_not_consume_super_scaler_geometry_history() {
        let width = 16;
        let height = 8;
        let snapshot = vec![Rgb565Pixel(0); width * height];
        let geometry = NavigationTransitionGeometry {
            source_card: NavigationTransitionRect {
                x: 1,
                y: 1,
                width: 4,
                height: 4,
            },
            ..NavigationTransitionGeometry::default()
        };
        let mut transition = NavigationTransitionRuntime::new(width, height, true);
        assert!(
            transition
                .begin(
                    NavigationTransitionEdge::HomeToConsoles,
                    NavigationTransitionDirection::Forward,
                    geometry,
                    &snapshot,
                    0,
                )
                .unwrap()
        );
        transition.capture_destination(&snapshot, 0).unwrap();
        transition.settle_at_destination();
        transition.complete();

        assert!(
            transition
                .begin_settings_page(
                    NavigationTransitionRoute::HomeToSettings,
                    NavigationTransitionDirection::Forward,
                    &snapshot,
                    1,
                )
                .unwrap()
        );
        assert_eq!(
            transition.cancel_for_exclusive_view(),
            Some(NavigationTransitionEndpoint::Source)
        );
        transition.complete();
        assert_eq!(
            transition.geometry_for_reverse(NavigationTransitionEdge::HomeToConsoles),
            Some(geometry)
        );

        assert!(
            transition
                .begin(
                    NavigationTransitionEdge::HomeToConsoles,
                    NavigationTransitionDirection::Reverse,
                    geometry,
                    &snapshot,
                    2,
                )
                .unwrap()
        );
        transition.capture_destination(&snapshot, 2).unwrap();
        transition.settle_at_destination();
        transition.complete();
        assert_eq!(
            transition.geometry_for_reverse(NavigationTransitionEdge::HomeToConsoles),
            None
        );
    }

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

    #[test]
    fn super_scaler_edges_keep_intended_durations() {
        assert_eq!(
            NavigationTransitionEdge::HomeToConsoles.duration_us(),
            600_000
        );
        assert_eq!(
            NavigationTransitionEdge::HomeToArcade.duration_us(),
            720_000
        );
        assert_eq!(
            NavigationTransitionEdge::ConsolesToSystem.duration_us(),
            720_000
        );
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
        let mut reverse = forward;
        reverse.direction = NavigationTransitionDirection::Reverse;
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
            let mut forward = NavigationTransitionRequest::new(
                NavigationTransitionEdge::HomeToArcade,
                NavigationTransitionDirection::Forward,
                geometry(),
            );
            forward.duration_us = duration_us;
            let mut reverse = forward;
            reverse.direction = NavigationTransitionDirection::Reverse;
            let covered_us = duration_us * SUPER_SCALER_COVER_PROGRESS as u64 / PROGRESS_MAX as u64;
            for forward_us in [0, covered_us, duration_us / 2, duration_us] {
                let forward_frame = frame_at(forward, forward_us);
                let reverse_frame = frame_at(reverse, duration_us - forward_us);
                assert_eq!(
                    forward_frame.progress_q16,
                    PROGRESS_MAX - reverse_frame.progress_q16,
                    "duration={duration_us} forward_us={forward_us}"
                );
            }
        }
    }

    #[test]
    fn destination_prepared_on_the_cover_tick_does_not_add_a_quantization_hold() {
        let duration_us = 500_000;
        let mut forward = NavigationTransitionRequest::new(
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Forward,
            geometry(),
        );
        forward.duration_us = duration_us;
        let mut reverse = forward;
        reverse.direction = NavigationTransitionDirection::Reverse;
        let forward_cover_us =
            duration_us * SUPER_SCALER_COVER_PROGRESS as u64 / PROGRESS_MAX as u64;
        let mut forward_controller = NavigationTransitionController::default();
        assert!(forward_controller.begin(forward, 0));
        assert!(forward_controller.captured(0, 0));
        let forward_reveal = forward_controller.tick(forward_cover_us, true);
        assert_eq!(forward_reveal.phase, NavigationTransitionPhase::Reveal);
        assert_eq!(forward_controller.telemetry().covered_hold_us, 0);

        let mut reverse_controller = NavigationTransitionController::default();
        assert!(reverse_controller.begin(reverse, 0));
        assert!(reverse_controller.captured(0, 0));
        let reverse_cover_us = duration_us.saturating_sub(forward_cover_us);
        let observed_cover = reverse_controller.tick(reverse_cover_us, false);
        assert_eq!(observed_cover.phase, NavigationTransitionPhase::Covered);
        let reveal = reverse_controller.tick(reverse_cover_us, true);
        assert_eq!(reveal.phase, NavigationTransitionPhase::Reveal);
        assert_eq!(reverse_controller.telemetry().covered_hold_us, 0);
        let progressing = reverse_controller.tick(reverse_cover_us + 1, true);
        assert_ne!(progressing.progress_q16, reveal.progress_q16);
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
    fn navigation_input_captures_and_replays_simultaneous_edges() {
        let previous = PadState::default();
        let mut physical = PadState {
            btn_a: true,
            btn_b: true,
            dpad_down: true,
            btn_x: true,
            ..PadState::default()
        };
        physical.rebuild_pressed_now();
        let input = NavigationTransitionInput::rising_edges(&physical, &previous).unwrap();

        assert!(input.activate);
        assert!(input.back);
        assert!(input.down);
        assert!(!input.home);
        let (replay_previous, replay_now) = input.replay(&physical);
        assert!(!replay_previous.btn_a);
        assert!(!replay_previous.btn_b);
        assert!(!replay_previous.dpad_down);
        assert!(replay_previous.btn_x);
        assert!(replay_now.btn_a);
        assert!(replay_now.btn_b);
        assert!(replay_now.dpad_down);
        assert!(replay_now.btn_x);
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
    fn timeout_completion_reports_the_failure_atomically() {
        let mut timed = request();
        timed.preparation_timeout_us = 50_000;
        let cover_us = timed.duration_us * SUPER_SCALER_COVER_PROGRESS as u64 / PROGRESS_MAX as u64;
        let mut controller = NavigationTransitionController::default();
        controller.begin(timed, 0);
        controller.captured(0, 0);
        controller.tick(cover_us, false);
        let reverse_at = cover_us + timed.preparation_timeout_us;
        controller.tick(reverse_at, false);
        controller.tick(reverse_at + cover_us, false);

        assert_eq!(
            controller.complete(),
            Some(NavigationTransitionCompletion {
                endpoint: NavigationTransitionEndpoint::Source,
                failure: Some(NavigationTransitionFailure::DestinationTimeout),
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
        let mut poc = NavigationTransitionRuntime::new(960, 540, true);
        poc.configure_preview(Some(4_000));
        assert_eq!(poc.duration_override_us, Some(4_000_000));
    }

    #[test]
    fn super_scaler_endpoints_are_exact_snapshots() {
        let mut poc = NavigationTransitionRuntime::new(16, 12, true);
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
        poc.capture_destination(&destination, 20_000).unwrap();
        poc.tick(20_000 + NavigationTransitionEdge::HomeToConsoles.duration_us());
        assert_eq!(poc.render().unwrap(), destination);

        let mut reverse = NavigationTransitionRuntime::new(16, 12, true);
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
        reverse.capture_destination(&source, 20_000).unwrap();
        reverse.tick(20_000 + NavigationTransitionEdge::HomeToConsoles.duration_us());
        assert_eq!(reverse.render().unwrap(), source);

        let mut cancelled = NavigationTransitionRuntime::new(16, 12, true);
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
    fn destination_preparation_does_not_advance_the_animation_clock() {
        let mut poc = NavigationTransitionRuntime::new(16, 12, true);
        let source = vec![Rgb565Pixel(0x1111); 16 * 12];
        let destination = vec![Rgb565Pixel(0x2222); 16 * 12];
        let geometry = NavigationTransitionGeometry {
            source_card: NavigationTransitionRect {
                x: 2,
                y: 2,
                width: 4,
                height: 8,
            },
            ..NavigationTransitionGeometry::default()
        };
        poc.begin(
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionDirection::Forward,
            geometry,
            &source,
            10_000,
        )
        .unwrap();

        let preparing = poc.tick(900_000);
        assert_eq!(preparing.phase, NavigationTransitionPhase::Capture);
        assert_eq!(preparing.progress_q16, 0);
        assert!(!poc.snapshot_locked());
        assert_eq!(poc.render().unwrap(), source);

        poc.capture_destination(&destination, 900_000).unwrap();
        assert!(poc.snapshot_locked());
        let first_animation_frame = poc.tick(900_000);
        assert_eq!(
            first_animation_frame.phase,
            NavigationTransitionPhase::Expand
        );
        assert_eq!(first_animation_frame.progress_q16, 0);
        assert_eq!(poc.render().unwrap(), source);
        poc.tick(900_000 + NavigationTransitionEdge::HomeToConsoles.duration_us());
        assert!(poc.snapshot_locked());
        assert!(poc.complete().is_some());
        assert!(!poc.snapshot_locked());
    }

    #[test]
    fn runtime_queue_preserves_every_input_across_chained_transitions() {
        let mut runtime = NavigationTransitionRuntime::new(16, 12, true);
        let duration_us =
            NavigationTransitionRequest::settings_page(NavigationTransitionDirection::Forward)
                .duration_us;
        let source = vec![Rgb565Pixel(0x1111); 16 * 12];
        let destination = vec![Rgb565Pixel(0x2222); 16 * 12];
        let activate = NavigationTransitionInput {
            activate: true,
            ..NavigationTransitionInput::default()
        };
        let back = NavigationTransitionInput {
            back: true,
            ..NavigationTransitionInput::default()
        };
        let home = NavigationTransitionInput {
            home: true,
            ..NavigationTransitionInput::default()
        };

        runtime
            .begin_settings_page(
                NavigationTransitionRoute::HomeToSettings,
                NavigationTransitionDirection::Forward,
                &source,
                0,
            )
            .unwrap();
        runtime.queue_input(activate);
        runtime.queue_input(back);
        runtime.queue_input(home);
        assert_eq!(runtime.take_queued_input(), None);
        runtime.capture_destination(&destination, 1).unwrap();
        runtime.tick(duration_us + 1);
        assert_eq!(
            runtime.complete().map(|completion| completion.endpoint),
            Some(NavigationTransitionEndpoint::Destination)
        );

        assert_eq!(runtime.take_queued_input(), Some(activate));
        runtime
            .begin_settings_page(
                NavigationTransitionRoute::HomeToSettings,
                NavigationTransitionDirection::Reverse,
                &destination,
                duration_us + 2,
            )
            .unwrap();
        assert_eq!(runtime.take_queued_input(), None);
        runtime
            .capture_destination(&source, duration_us + 3)
            .unwrap();
        runtime.tick(duration_us * 2 + 3);
        assert!(runtime.complete().is_some());

        assert_eq!(runtime.take_queued_input(), Some(back));
        assert_eq!(runtime.take_queued_input(), Some(home));
        assert_eq!(runtime.take_queued_input(), None);
    }

    #[test]
    fn runtime_queue_clears_when_transition_is_disabled() {
        let mut runtime = NavigationTransitionRuntime::new(16, 12, true);
        runtime.queue_input(NavigationTransitionInput {
            back: true,
            ..NavigationTransitionInput::default()
        });

        runtime.set_enabled(16, 12, false);

        assert_eq!(runtime.take_queued_input(), None);
    }

    #[test]
    fn routed_input_replays_every_rapid_back_edge_in_order() {
        let mut runtime = NavigationTransitionRuntime::new(16, 12, true);
        let source = vec![Rgb565Pixel(0x1111); 16 * 12];
        runtime
            .begin_settings_page(
                NavigationTransitionRoute::HomeToSettings,
                NavigationTransitionDirection::Reverse,
                &source,
                0,
            )
            .unwrap();
        let released = PadState::default();
        let pressed = PadState {
            btn_b: true,
            ..PadState::default()
        };

        assert!(runtime.route_input(&pressed, &released, false).is_none());
        assert!(runtime.route_input(&released, &pressed, false).is_none());
        assert!(runtime.route_input(&pressed, &released, false).is_none());
        runtime.settle_at_destination();
        assert!(runtime.complete().is_some());

        let first = runtime.route_input(&released, &released, false).unwrap();
        assert!(first.replayed);
        assert!(!first.previous.btn_b);
        assert!(first.now.btn_b);
        let second = runtime.route_input(&released, &released, false).unwrap();
        assert!(second.replayed);
        assert!(!second.previous.btn_b);
        assert!(second.now.btn_b);
        let live = runtime.route_input(&released, &released, false).unwrap();
        assert!(!live.replayed);
    }

    #[test]
    fn routed_input_preserves_new_physical_edges_behind_the_backlog() {
        let mut runtime = NavigationTransitionRuntime::new(16, 12, true);
        runtime.queue_input(NavigationTransitionInput {
            activate: true,
            ..NavigationTransitionInput::default()
        });
        let released = PadState::default();
        let physical_back = PadState {
            btn_b: true,
            ..PadState::default()
        };

        let activate = runtime
            .route_input(&physical_back, &released, false)
            .unwrap();
        assert!(activate.replayed);
        assert!(activate.now.btn_a);
        assert!(activate.previous.btn_b);
        assert!(activate.now.btn_b);
        let back = runtime
            .route_input(&released, &physical_back, false)
            .unwrap();
        assert!(back.replayed);
        assert!(back.now.btn_b);
    }

    #[test]
    fn routed_input_removes_consumed_cancel_edges_only() {
        let mut runtime = NavigationTransitionRuntime::new(16, 12, true);
        let released = PadState::default();
        let physical = PadState {
            btn_b: true,
            btn_home: true,
            dpad_down: true,
            ..PadState::default()
        };

        let remaining = runtime.route_input(&physical, &released, true).unwrap();

        assert!(remaining.replayed);
        assert!(remaining.previous.btn_b);
        assert!(remaining.previous.btn_home);
        assert!(!remaining.previous.dpad_down);
        assert!(remaining.now.btn_b);
        assert!(remaining.now.btn_home);
        assert!(remaining.now.dpad_down);
    }

    #[test]
    fn disabled_runtime_uses_empty_frame_buffers() {
        let poc = NavigationTransitionRuntime::new(960, 540, false);
        assert!(!poc.enabled());
        assert_eq!(poc.buffers.width(), 0);
        assert_eq!(poc.buffers.height(), 0);
        assert!(poc.buffers.working().is_empty());
        assert!(!poc.buffers.source_ready());
        assert!(!poc.buffers.destination_ready());
    }
}
