// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral navigation-transition state and RGB565 frame ownership.

use slint::platform::software_renderer::Rgb565Pixel;

const PROGRESS_MAX: u16 = u16::MAX;
const COVER_PROGRESS: u16 = 36_044;
const DEFAULT_PREPARATION_TIMEOUT_US: u64 = 5_000_000;

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
}
