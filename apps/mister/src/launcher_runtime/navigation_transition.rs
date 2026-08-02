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
        if started && direction == NavigationTransitionDirection::Forward {
            self.geometry_history.push((edge, geometry));
        }
        Ok(started)
    }

    pub fn begin_settings_page(
        &mut self,
        direction: NavigationTransitionDirection,
        source: &[Rgb565Pixel],
        now_us: u64,
    ) -> Result<bool, NavigationTransitionFailure> {
        let mut request = NavigationTransitionRequest::settings_page(direction);
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
    fn settings_page_push_settles_to_exact_snapshots_in_both_directions() {
        let width = 16;
        let height = 3;
        let source = vec![Rgb565Pixel(0xf800); width * height];
        let destination = vec![Rgb565Pixel(0x07e0); width * height];
        let mut buffers = NavigationTransitionBuffers::new(width, height);
        buffers.begin_capture();
        buffers.capture_source(&source).unwrap();
        buffers.capture_destination(&destination).unwrap();

        for direction in [
            NavigationTransitionDirection::Forward,
            NavigationTransitionDirection::Reverse,
        ] {
            let request = NavigationTransitionRequest::settings_page(direction);
            render_settings_page_push(
                &mut buffers,
                request,
                NavigationTransitionFrame {
                    progress_q16: 0,
                    ..NavigationTransitionFrame::default()
                },
            )
            .unwrap();
            assert_eq!(buffers.working(), source);

            render_settings_page_push(
                &mut buffers,
                request,
                NavigationTransitionFrame {
                    progress_q16: PROGRESS_MAX,
                    ..NavigationTransitionFrame::default()
                },
            )
            .unwrap();
            assert_eq!(buffers.working(), destination);
        }
    }

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
    fn settings_page_push_moves_only_horizontally_with_clipped_row_copies() {
        let width = 8;
        let height = 2;
        let source: Vec<_> = (0..width * height)
            .map(|index| Rgb565Pixel(index as u16))
            .collect();
        let mut output = vec![Rgb565Pixel(0xffff); width * height];

        assert_eq!(
            blit_snapshot_x(&mut output, &source, width, height, -3),
            ((width - 3) * height) as u64
        );
        assert_eq!(&output[..width - 3], &source[3..width]);
        assert_eq!(
            &output[width..width + width - 3],
            &source[width + 3..width * 2]
        );

        output.fill(Rgb565Pixel(0xffff));
        assert_eq!(
            blit_snapshot_x(&mut output, &source, width, height, 3),
            ((width - 3) * height) as u64
        );
        assert_eq!(&output[3..width], &source[..width - 3]);
        assert_eq!(&output[width + 3..width * 2], &source[width..width * 2 - 3]);
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
                .begin_settings_page(NavigationTransitionDirection::Forward, &snapshot, 1)
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
                ..NavigationTransitionGeometry::default()
            },
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
    fn super_scaler_visual_windows_use_only_the_smooth_spring() {
        let source = include_str!("navigation_transition.rs");
        let production = source
            .rsplit_once("\n#[cfg(test)]\nmod tests {")
            .expect("test module delimiter")
            .0;
        assert!(!production.contains("smoothstep_q16"));
        assert!(!production.contains("ease_out_cubic_q16"));
        assert!(!production.contains("with_overshoot"));
        assert!(!production.contains("recoil"));
        for (line_number, line) in production.lines().enumerate() {
            if line.contains("window_q16(") && !line.contains("fn window_q16(") {
                assert!(
                    line.contains("spring_ease_q16(window_q16("),
                    "raw-linear visual window at source line {}: {line}",
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
        let mut scale_source_x = vec![0; width];
        let mut scale_source_y = vec![0; height];
        let mut scale_excluded_x = vec![false; width];
        let mut scale_dither_x = vec![false; width * 4];
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
            &mut scale_source_x,
            &mut scale_source_y,
            &mut scale_excluded_x,
            &mut scale_dither_x,
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
                .any(|pixel| *pixel == Rgb565Pixel(0x40ed))
        );
        assert!(
            buffers
                .working()
                .iter()
                .any(|pixel| *pixel == Rgb565Pixel(0x28aa))
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
        assert_eq!(selected_row[48 * width + 20], Rgb565Pixel(0x1234));
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

        let mut content = vec![Rgb565Pixel(0); width * height];
        reveal_destination_regions(
            &mut content,
            &destination,
            width,
            height,
            60_000,
            system_request(NavigationTransitionDirection::Forward),
            &mut stats,
        );
        assert_eq!(content[50 * width + 77], Rgb565Pixel(0x1234));
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
    fn selected_row_enters_monotonically_and_settles_exactly() {
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
        let mut stats = NavigationTransitionRenderStats::default();
        let initial_offset = -(rect.right() as isize + SYSTEM_ROW_OFFSCREEN_MARGIN);
        let mut previous_right = 0;
        let mut settled = Vec::new();
        for phase in [0, 8_000, 16_000, 24_000, 32_000, 48_000, PROGRESS_MAX] {
            let mut frame = vec![Rgb565Pixel(0); width * height];
            copy_rect_shifted_x(
                &mut frame,
                &source,
                width,
                height,
                rect,
                spring_ease_q16(phase),
                initial_offset,
                &mut stats,
            );
            let right = frame[3 * width..4 * width]
                .iter()
                .rposition(|pixel| *pixel == Rgb565Pixel(0xaaaa))
                .map_or(0, |x| x + 1);
            assert!(
                right >= previous_right,
                "row moved backwards at phase {phase}"
            );
            assert!(
                right <= rect.right() as usize,
                "row overshot its destination"
            );
            previous_right = right;
            settled = frame;
        }
        assert_eq!(
            &settled[3 * width + 8..3 * width + 16],
            &source[3 * width + 8..3 * width + 16]
        );
    }

    #[test]
    fn reverse_selected_row_exits_monotonically_without_recoil() {
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
        let mut previous_left = rect.x as usize;
        let mut gone = source.clone();
        for phase in [0, 8_000, 16_000, 24_000, 32_000, 48_000, PROGRESS_MAX] {
            let mut frame = source.clone();
            slide_rect_out_left(
                &mut frame,
                &source,
                width,
                height,
                rect,
                spring_ease_q16(phase),
                shell,
                &mut stats,
            );
            let left = frame[3 * width..4 * width]
                .iter()
                .position(|pixel| *pixel != shell && *pixel != Rgb565Pixel(0))
                .unwrap_or(0);
            assert!(left <= previous_left, "row recoiled at phase {phase}");
            previous_left = left;
            gone = frame;
        }
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
            .begin_settings_page(NavigationTransitionDirection::Forward, &source, 0)
            .unwrap();
        runtime.queue_input(activate);
        runtime.queue_input(back);
        runtime.queue_input(home);
        assert_eq!(runtime.take_queued_input(), None);
        runtime.capture_destination(&destination, 1).unwrap();
        runtime.tick(SETTINGS_PAGE_DURATION_US + 1);
        assert_eq!(
            runtime.complete().map(|completion| completion.endpoint),
            Some(NavigationTransitionEndpoint::Destination)
        );

        assert_eq!(runtime.take_queued_input(), Some(activate));
        runtime
            .begin_settings_page(
                NavigationTransitionDirection::Reverse,
                &destination,
                SETTINGS_PAGE_DURATION_US + 2,
            )
            .unwrap();
        assert_eq!(runtime.take_queued_input(), None);
        runtime
            .capture_destination(&source, SETTINGS_PAGE_DURATION_US + 3)
            .unwrap();
        runtime.tick(SETTINGS_PAGE_DURATION_US * 2 + 3);
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
            .begin_settings_page(NavigationTransitionDirection::Reverse, &source, 0)
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
    fn crt_overlay_sweeps_holds_clears_and_preserves_endpoints() {
        let width = 12;
        let height = 10;
        let original = vec![Rgb565Pixel(0xffff); width * height];
        let full_phosphor_pixels = ((height + 1) / CRT_SCANLINE_PERIOD_ROWS * width) as u64;
        for (progress, expected_full) in [
            (0, false),
            (1, false),
            (CRT_SWEEP_END_Q16, true),
            (PROGRESS_MAX / 2, true),
            (CRT_CLEAR_START_Q16, true),
            (PROGRESS_MAX - 1, false),
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
                &mut stats,
            );
            if progress == 0 || progress == PROGRESS_MAX {
                assert_eq!(pixels, original);
                assert_eq!(stats.phosphor_pixels, 0);
                assert_eq!(stats.scanline_pixels, 0);
            } else if expected_full {
                assert_eq!(stats.phosphor_pixels, full_phosphor_pixels);
                assert_eq!(stats.scanline_pixels, 0);
                let darkened = darken_rgb565_7_8(Rgb565Pixel(0xffff));
                for y in 0..height {
                    let expected = if y >= 1 && (y - 1) % CRT_SCANLINE_PERIOD_ROWS == 0 {
                        darkened
                    } else {
                        Rgb565Pixel(0xffff)
                    };
                    assert!(
                        pixels[y * width..(y + 1) * width]
                            .iter()
                            .all(|pixel| *pixel == expected)
                    );
                }
            } else {
                assert!(
                    stats.phosphor_pixels < full_phosphor_pixels,
                    "progress={progress} phosphor_pixels={}",
                    stats.phosphor_pixels
                );
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
            &mut stats,
        );
        assert_eq!(stats.phosphor_pixels, full_phosphor_pixels);
        assert_eq!(stats.scanline_pixels, 0);
    }

    #[test]
    fn disabled_runtime_does_not_allocate_frame_buffers() {
        let poc = NavigationTransitionRuntime::new(960, 540, false);
        assert!(!poc.enabled());
        assert!(poc.buffers.source.is_empty());
        assert!(poc.buffers.destination.is_empty());
        assert!(poc.buffers.working.is_empty());
        assert!(poc.buffers.scale_source_x.is_empty());
        assert!(poc.buffers.scale_source_y.is_empty());
        assert!(poc.buffers.scale_excluded_x.is_empty());
        assert!(poc.buffers.scale_dither_x.is_empty());
    }
}
