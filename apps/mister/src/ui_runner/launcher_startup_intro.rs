// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! First-run startup intro presentation over the production hidden-slot latch.

use super::*;
use crate::ui_runner::launcher_readiness::{SourceEvidenceRequest, SourceFrameEvidence};
use mister_magik_fb::launcher_runtime::startup_intro::StartupIntroPlayback;
use mister_magik_latch_contract::{PresentationTelemetry, validate_presentation_telemetry_window};

pub(super) struct PreparedStartupIntro {
    playback: StartupIntroPlayback,
}

impl PreparedStartupIntro {
    pub(super) fn new(ui: &UiDisplay) -> Result<Self, String> {
        #[cfg(feature = "ui-device-tests")]
        if std::env::var("MISTER_UI_TEST_STARTUP_MODE").ok().as_deref()
            == Some("cold-intro-failure")
        {
            return Err("UI-test injected startup intro preparation failure".to_string());
        }
        Ok(Self {
            playback: StartupIntroPlayback::new(ui)?,
        })
    }

    pub(super) fn start(self) -> StartupIntroSession {
        let refresh_period_us = self.playback.refresh_period_us();
        StartupIntroSession {
            playback: self.playback,
            refresh_period_us,
            confirmed_frames: 0,
            expected_refresh_intervals: 0,
            software_estimated_dropped_frames: 0,
            pacing_failures: 0,
            max_confirmation_gap_us: 0,
            last_confirmed_at: None,
            presentation_start: None,
        }
    }
}

pub(super) struct StartupIntroSession {
    playback: StartupIntroPlayback,
    refresh_period_us: u64,
    confirmed_frames: u64,
    expected_refresh_intervals: u64,
    software_estimated_dropped_frames: u64,
    pacing_failures: u64,
    max_confirmation_gap_us: u64,
    last_confirmed_at: Option<Instant>,
    presentation_start: Option<Result<(PresentationTelemetry, Instant), String>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct StartupIntroCadence {
    pub(super) confirmed_frames: u64,
    pub(super) cabinet_wait_frames: u64,
    pub(super) expected_refresh_intervals: u64,
    pub(super) software_estimated_dropped_frames: u64,
    pub(super) pacing_failures: u64,
    pub(super) max_confirmation_gap_us: u64,
}

impl StartupIntroSession {
    pub(super) fn presentation_start_capture_needed(&self) -> bool {
        self.presentation_start.is_none()
    }

    pub(super) fn snapshot_capture_needed(&self) -> bool {
        self.playback.snapshot_capture_needed()
    }

    pub(super) const fn waiting_frames(&self) -> u64 {
        self.playback.waiting_frames()
    }

    pub(super) fn begin_launcher_snapshot_preparation(
        &mut self,
        launcher_pixels: &[Rgb565Pixel],
    ) -> Result<(), String> {
        self.playback
            .begin_launcher_snapshot_preparation(launcher_pixels)
    }

    pub(super) fn poll_launcher_snapshot_preparation(&mut self) -> Result<bool, String> {
        self.playback.poll_launcher_snapshot_preparation()
    }

    pub(super) fn render_into(
        &mut self,
        grant: HiddenSlotRenderGrant,
        pixels: &mut [Rgb565Pixel],
        readiness_source_request: Option<SourceEvidenceRequest>,
    ) -> Result<Option<SourceFrameEvidence>, String> {
        let geometry = self.playback.geometry();
        if grant.width != geometry.width()
            || grant.height != geometry.height()
            || grant.stride_pixels != grant.width
        {
            return Err(format!(
                "startup intro grant geometry {}x{} stride={} does not match {}x{}",
                grant.width,
                grant.height,
                grant.stride_pixels,
                geometry.width(),
                geometry.height()
            ));
        }
        let slot = grant
            .slot_index
            .checked_sub(1)
            .ok_or("startup intro received invalid hidden slot zero")?;
        self.playback
            .render_into(pixels, slot, grant.stride_pixels)?;
        Ok(hidden_frame_source_evidence(
            pixels,
            grant,
            readiness_source_request,
        ))
    }

    /// Advances only after the latch reports this sequence active at the
    /// physical scanout boundary. Latch protocol drops and dropped frames are
    /// deliberately separate signals: a healthy latch may still miss a
    /// presentation deadline when rendering takes longer than one refresh.
    pub(super) fn note_confirmed_present(
        &mut self,
        confirmed_at: Instant,
        refresh_period_us: u64,
        vsync_confirmed: bool,
    ) -> Option<StartupIntroCadence> {
        if refresh_period_us != 0 {
            self.refresh_period_us = refresh_period_us;
        }
        self.confirmed_frames = self.confirmed_frames.saturating_add(1);
        if !vsync_confirmed {
            self.pacing_failures = self.pacing_failures.saturating_add(1);
        }
        if let Some(previous) = self.last_confirmed_at.replace(confirmed_at) {
            let gap_us = confirmed_at
                .saturating_duration_since(previous)
                .as_micros()
                .min(u128::from(u64::MAX)) as u64;
            self.max_confirmation_gap_us = self.max_confirmation_gap_us.max(gap_us);
            let expected = expected_refresh_intervals(gap_us, refresh_period_us);
            self.expected_refresh_intervals =
                self.expected_refresh_intervals.saturating_add(expected);
            self.software_estimated_dropped_frames = self
                .software_estimated_dropped_frames
                .saturating_add(expected.saturating_sub(1));
        }
        self.playback
            .note_presented(self.refresh_period_us)
            .then_some(self.cadence())
    }

    pub(super) const fn cadence(&self) -> StartupIntroCadence {
        StartupIntroCadence {
            confirmed_frames: self.confirmed_frames,
            cabinet_wait_frames: self.playback.waiting_frames(),
            expected_refresh_intervals: self.expected_refresh_intervals,
            software_estimated_dropped_frames: self.software_estimated_dropped_frames,
            pacing_failures: self.pacing_failures,
            max_confirmation_gap_us: self.max_confirmation_gap_us,
        }
    }

    pub(super) fn capture_presentation_start(
        &mut self,
        captured_at: Instant,
        telemetry: std::io::Result<PresentationTelemetry>,
    ) {
        if self.presentation_start.is_none() {
            self.presentation_start = Some(
                telemetry
                    .map(|snapshot| (snapshot, captured_at))
                    .map_err(|error| error.to_string()),
            );
        }
    }

    pub(super) fn authoritative_cadence_status(
        &self,
        captured_at: Instant,
        telemetry: std::io::Result<PresentationTelemetry>,
        software: StartupIntroCadence,
    ) -> runtime_status::StartupIntroCadenceStatus {
        let start = match self.presentation_start.as_ref() {
            Some(Ok(start)) => *start,
            Some(Err(error)) => {
                return unavailable_cadence_status(software, error.clone(), None, None);
            }
            None => {
                return unavailable_cadence_status(
                    software,
                    "startup intro presentation telemetry baseline was not captured".into(),
                    None,
                    None,
                );
            }
        };
        let end = match telemetry {
            Ok(end) => end,
            Err(error) => {
                return unavailable_cadence_status(
                    software,
                    error.to_string(),
                    Some(snapshot_status(start.0)),
                    None,
                );
            }
        };
        let elapsed_us = captured_at
            .saturating_duration_since(start.1)
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
        match validate_presentation_telemetry_window(
            start.0,
            end,
            elapsed_us,
            self.refresh_period_us,
        ) {
            Ok(delta) => runtime_status::StartupIntroCadenceStatus {
                schema: "mister-magik-startup-intro-cadence-v1",
                source: "fpga-owned-vblank-telemetry",
                available: true,
                qualified: delta.repeated_vblank_delta == 0,
                dropped_frames: Some(u64::from(delta.repeated_vblank_delta)),
                software_estimated_dropped_frames: software.software_estimated_dropped_frames,
                confirmed_frames: software.confirmed_frames,
                cabinet_wait_frames: software.cabinet_wait_frames,
                expected_refresh_intervals: software.expected_refresh_intervals,
                pacing_failures: software.pacing_failures,
                max_confirmation_gap_us: software.max_confirmation_gap_us,
                elapsed_us: Some(delta.elapsed_us),
                owned_vblank_delta: Some(delta.owned_vblank_delta),
                presented_vblank_delta: Some(delta.presented_vblank_delta),
                repeated_vblank_delta: Some(delta.repeated_vblank_delta),
                ownership_loss_delta: Some(delta.ownership_loss_delta),
                start: Some(snapshot_status(start.0)),
                end: Some(snapshot_status(end)),
                error: None,
            },
            Err(error) => unavailable_cadence_status(
                software,
                error.to_string(),
                Some(snapshot_status(start.0)),
                Some(snapshot_status(end)),
            ),
        }
    }

    pub(super) fn restore_handoff_snapshot(&self, target: &mut LayerTarget<'_>) -> bool {
        target.restore_presentation_cached(self.playback.handoff_snapshot())
    }

    #[cfg(test)]
    pub(super) fn frame(&self) -> u64 {
        self.playback.frame()
    }

    #[cfg(test)]
    pub(super) fn elapsed(&self) -> Duration {
        self.playback.elapsed()
    }
}

fn hidden_frame_source_evidence(
    pixels: &[Rgb565Pixel],
    grant: HiddenSlotRenderGrant,
    request: Option<SourceEvidenceRequest>,
) -> Option<SourceFrameEvidence> {
    request.and_then(|request| {
        SourceFrameEvidence::from_rgb565_rows(
            pixels,
            grant.width,
            grant.height,
            grant.stride_pixels,
            request,
        )
    })
}

fn snapshot_status(
    telemetry: PresentationTelemetry,
) -> runtime_status::PresentationTelemetrySnapshotStatus {
    runtime_status::PresentationTelemetrySnapshotStatus {
        owned_vblank_count: telemetry.owned_vblank_count,
        presented_vblank_count: telemetry.presented_vblank_count,
        repeated_vblank_count: telemetry.repeated_vblank_count,
        ownership_loss_count: telemetry.ownership_loss_count,
        active_sequence: telemetry.active_sequence,
        magik_ownership: telemetry.magik_ownership(),
        pending: telemetry.pending(),
        lifetime_invariant_valid: telemetry.lifetime_invariant_valid(),
    }
}

fn unavailable_cadence_status(
    software: StartupIntroCadence,
    error: String,
    start: Option<runtime_status::PresentationTelemetrySnapshotStatus>,
    end: Option<runtime_status::PresentationTelemetrySnapshotStatus>,
) -> runtime_status::StartupIntroCadenceStatus {
    runtime_status::StartupIntroCadenceStatus {
        schema: "mister-magik-startup-intro-cadence-v1",
        source: "fpga-owned-vblank-telemetry",
        available: false,
        qualified: false,
        dropped_frames: None,
        software_estimated_dropped_frames: software.software_estimated_dropped_frames,
        confirmed_frames: software.confirmed_frames,
        cabinet_wait_frames: software.cabinet_wait_frames,
        expected_refresh_intervals: software.expected_refresh_intervals,
        pacing_failures: software.pacing_failures,
        max_confirmation_gap_us: software.max_confirmation_gap_us,
        elapsed_us: None,
        owned_vblank_delta: None,
        presented_vblank_delta: None,
        repeated_vblank_delta: None,
        ownership_loss_delta: None,
        start,
        end,
        error: Some(error),
    }
}

fn expected_refresh_intervals(gap_us: u64, refresh_period_us: u64) -> u64 {
    if refresh_period_us == 0 {
        return 1;
    }
    gap_us
        .saturating_add(refresh_period_us / 2)
        .checked_div(refresh_period_us)
        .unwrap_or(1)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mister_magik_fb::launcher_runtime::startup_intro::{FINAL_ELAPSED, MORPH_START};

    fn test_session() -> StartupIntroSession {
        let ui = UiDisplay::for_framebuffer(320, 180);
        let prepared = PreparedStartupIntro::new(&ui).unwrap();
        let refresh_period_us = prepared.playback.refresh_period_us();
        StartupIntroSession {
            playback: prepared.playback,
            refresh_period_us,
            confirmed_frames: 0,
            expected_refresh_intervals: 0,
            software_estimated_dropped_frames: 0,
            pacing_failures: 0,
            max_confirmation_gap_us: 0,
            last_confirmed_at: None,
            presentation_start: None,
        }
    }

    #[test]
    fn snapshot_failure_leaves_the_original_composition_cache_intact() {
        let mut session = test_session();
        let original = session.playback.handoff_snapshot().to_vec();

        assert!(
            session
                .begin_launcher_snapshot_preparation(&[Rgb565Pixel(7)])
                .is_err()
        );
        assert_eq!(session.playback.handoff_snapshot(), original);
        assert!(session.snapshot_capture_needed());
    }

    #[test]
    fn intro_source_evidence_is_captured_only_when_readiness_requests_it() {
        let grant = HiddenSlotRenderGrant {
            slot_index: 1,
            generation: 1,
            width: 2,
            height: 2,
            stride_pixels: 2,
        };
        let pixels = [Rgb565Pixel(0x1234); 4];

        assert!(hidden_frame_source_evidence(&pixels, grant, None).is_none());
        assert!(
            hidden_frame_source_evidence(&pixels, grant, Some(SourceEvidenceRequest::Nonblank),)
                .is_some()
        );
    }

    #[test]
    fn handoff_restores_the_original_composition_cache() {
        let ui = UiDisplay::for_framebuffer(320, 180);
        let mut session = test_session();
        let expected = (0..320 * 180)
            .map(|index| Rgb565Pixel(index as u16))
            .collect::<Vec<_>>();
        session
            .begin_launcher_snapshot_preparation(&expected)
            .unwrap();
        let mut target = UiFrameTarget::cached(frame_target_geometry(&ui));
        target.cached_565_mut().fill(Rgb565Pixel(0xffff));
        let mut layer = LayerTarget::new(&mut target, &ui);

        assert!(session.restore_handoff_snapshot(&mut layer));
        assert_eq!(layer.cached_frame_view().pixels(), expected);
    }

    #[test]
    fn pal_and_ntsc_cadence_cross_the_morph_boundary_by_elapsed_time() {
        for period_us in [16_667, 20_000] {
            let mut session = test_session();
            let confirms_before_boundary = 16_000_000_u64.div_ceil(period_us) - 1;
            for frame in 0..confirms_before_boundary {
                assert!(
                    session
                        .note_confirmed_present(
                            Instant::now() + Duration::from_micros(frame * period_us),
                            period_us,
                            true,
                        )
                        .is_none()
                );
            }
            assert!(session.elapsed() < MORPH_START);

            assert!(
                session
                    .note_confirmed_present(Instant::now(), period_us, true)
                    .is_none()
            );
            assert!(session.elapsed() >= MORPH_START);
        }
    }

    #[test]
    fn refresh_intervals_round_to_the_nearest_physical_period() {
        assert_eq!(expected_refresh_intervals(16_667, 16_667), 1);
        assert_eq!(expected_refresh_intervals(33_334, 16_667), 2);
        assert_eq!(expected_refresh_intervals(24_999, 16_667), 1);
        assert_eq!(expected_refresh_intervals(25_001, 16_667), 2);
    }

    #[test]
    fn confirmed_cadence_counts_a_skip_with_a_healthy_latch() {
        let period_us = 16_667;
        let origin = Instant::now();
        let run = |skip_at: Option<u64>| {
            let mut session = test_session();
            let mut completed = None;
            for frame in 0..2_000 {
                let skipped_us = u64::from(skip_at.is_some_and(|at| frame >= at)) * period_us;
                completed = session.note_confirmed_present(
                    origin + Duration::from_micros(frame * period_us + skipped_us),
                    period_us,
                    true,
                );
                if completed.is_some() {
                    break;
                }
            }
            (session, completed.unwrap())
        };

        let (exact_session, exact) = run(None);
        assert_eq!(exact.confirmed_frames, 1_200);
        assert_eq!(exact.expected_refresh_intervals, 1_199);
        assert_eq!(exact.software_estimated_dropped_frames, 0);
        assert_eq!(exact.pacing_failures, 0);
        assert_eq!(exact_session.elapsed(), FINAL_ELAPSED);

        let (dropped_session, dropped) = run(Some(600));
        assert_eq!(dropped.confirmed_frames, 1_200);
        assert_eq!(dropped.expected_refresh_intervals, 1_200);
        assert_eq!(dropped.software_estimated_dropped_frames, 1);
        assert_eq!(dropped.pacing_failures, 0);
        assert_eq!(dropped_session.elapsed(), FINAL_ELAPSED);
    }

    #[test]
    fn fpga_repeats_are_authoritative_when_software_disagrees() {
        let owned = 1 << mister_magik_latch_contract::STATUS_MAGIK_OWNERSHIP;
        let snapshot = |owned_vblank_count, presented_vblank_count, repeated_vblank_count| {
            PresentationTelemetry {
                owned_vblank_count,
                presented_vblank_count,
                repeated_vblank_count,
                ownership_loss_count: 0,
                active_sequence: 7,
                flags: owned,
                crc: 0,
            }
        };
        let started = Instant::now();
        let software = StartupIntroCadence {
            confirmed_frames: 2,
            cabinet_wait_frames: 0,
            expected_refresh_intervals: 2,
            software_estimated_dropped_frames: 1,
            pacing_failures: 0,
            max_confirmation_gap_us: 33_334,
        };
        let mut session = test_session();
        session.capture_presentation_start(started, Ok(snapshot(100, 100, 0)));
        let fpga_zero = session.authoritative_cadence_status(
            started + Duration::from_micros(16_667),
            Ok(snapshot(101, 101, 0)),
            software,
        );
        assert_eq!(fpga_zero.dropped_frames, Some(0));
        assert!(fpga_zero.qualified);
        assert_eq!(fpga_zero.software_estimated_dropped_frames, 1);

        let fpga_repeat = session.authoritative_cadence_status(
            started + Duration::from_micros(16_667),
            Ok(snapshot(101, 100, 1)),
            StartupIntroCadence {
                software_estimated_dropped_frames: 0,
                ..software
            },
        );
        assert_eq!(fpga_repeat.dropped_frames, Some(1));
        assert!(!fpga_repeat.qualified);
    }

    #[test]
    fn missing_fpga_telemetry_fails_cadence_closed() {
        let session = test_session();
        let software = StartupIntroCadence {
            confirmed_frames: 1_200,
            cabinet_wait_frames: 0,
            expected_refresh_intervals: 1_199,
            software_estimated_dropped_frames: 0,
            pacing_failures: 0,
            max_confirmation_gap_us: 16_667,
        };
        let status = session.authoritative_cadence_status(
            Instant::now(),
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "missing capability",
            )),
            software,
        );
        assert_eq!(status.dropped_frames, None);
        assert!(!status.available);
        assert!(!status.qualified);
    }

    #[test]
    fn pal_and_ntsc_complete_at_exactly_twenty_storyboard_seconds() {
        for (period_us, expected_frames) in [(16_667, 1_200), (20_000, 1_000)] {
            let mut session = test_session();
            let mut completed = None;
            for frame in 0..expected_frames {
                completed = session.note_confirmed_present(
                    Instant::now() + Duration::from_micros(frame * period_us),
                    period_us,
                    true,
                );
            }

            assert!(completed.is_some());
            assert_eq!(session.frame(), expected_frames);
            assert_eq!(session.elapsed(), FINAL_ELAPSED);
        }
    }
}
