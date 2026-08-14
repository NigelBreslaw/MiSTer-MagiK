// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! First-run startup intro presentation over the production hidden-slot latch.

use super::*;
use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use mister_magik_fb::framebuffer::vertical_scale::{
    Rgb565FrameView, VerticalRect, VerticalRgb565Transform,
};
use mister_magik_framebuffer_scenes::{
    FramebufferScene, Rgb565Pixel as SceneRgb565Pixel, SceneBufferId, SceneClock, SceneGeometry,
    SceneTarget,
};
use mister_magik_latch_contract::{PresentationTelemetry, validate_presentation_telemetry_window};
use mister_magik_particles::intro::{
    IntroParticleDensity, IntroProjectionScale, IntroScene, IntroSceneOptions,
    PreparedLauncherSnapshot,
};
use mister_magik_particles::intro_recipe::embedded_intro_recipe;

const DEFAULT_REFRESH_PERIOD_US: u64 = 16_667;
const MORPH_START: Duration = Duration::from_secs(16);
const FINAL_ELAPSED: Duration = Duration::from_secs(20);

pub(super) struct PreparedStartupIntro {
    scene: IntroScene,
    handoff_snapshot: Vec<Rgb565Pixel>,
    composition_width: usize,
    composition_height: usize,
    initial_refresh_period_us: u64,
}

impl PreparedStartupIntro {
    pub(super) fn new(ui: &UiDisplay) -> Result<Self, String> {
        let recipe = embedded_intro_recipe()?;
        let options = if ui.output_route().is_crt() {
            IntroSceneOptions {
                particle_density: IntroParticleDensity::Half,
                projection_scale: IntroProjectionScale::crt(ui.fb_h()),
            }
        } else {
            IntroSceneOptions::default()
        };
        let scene = IntroScene::new_with_options(ui.fb_w(), ui.fb_h(), recipe, options)?;
        Ok(Self {
            scene,
            handoff_snapshot: vec![Rgb565Pixel(0); ui.render_w().saturating_mul(ui.render_h())],
            composition_width: ui.render_w(),
            composition_height: ui.render_h(),
            initial_refresh_period_us: ui
                .output_route()
                .nominal_period_us()
                .unwrap_or(DEFAULT_REFRESH_PERIOD_US),
        })
    }

    pub(super) fn attach(self, buffers: PluginLatchFrameBuffers) -> StartupIntroSession {
        StartupIntroSession {
            scene: self.scene,
            buffers: Some(buffers),
            handoff_snapshot: self.handoff_snapshot,
            composition_width: self.composition_width,
            composition_height: self.composition_height,
            snapshot_preparation: LauncherSnapshotPreparation::AwaitingFrame,
            frame: 0,
            elapsed: Duration::ZERO,
            waiting_elapsed: Duration::ZERO,
            refresh_period_us: self.initial_refresh_period_us,
            snapshot_ready: false,
            completed: false,
            confirmed_frames: 0,
            expected_refresh_intervals: 0,
            software_estimated_dropped_frames: 0,
            pacing_failures: 0,
            max_confirmation_gap_us: 0,
            last_confirmed_at: None,
            waiting_frames: 0,
            last_render_waiting: false,
            presentation_start: None,
        }
    }
}

pub(super) struct StartupIntroSession {
    scene: IntroScene,
    buffers: Option<PluginLatchFrameBuffers>,
    handoff_snapshot: Vec<Rgb565Pixel>,
    composition_width: usize,
    composition_height: usize,
    snapshot_preparation: LauncherSnapshotPreparation,
    frame: u64,
    elapsed: Duration,
    waiting_elapsed: Duration,
    refresh_period_us: u64,
    snapshot_ready: bool,
    completed: bool,
    confirmed_frames: u64,
    expected_refresh_intervals: u64,
    software_estimated_dropped_frames: u64,
    pacing_failures: u64,
    max_confirmation_gap_us: u64,
    last_confirmed_at: Option<Instant>,
    waiting_frames: u64,
    last_render_waiting: bool,
    presentation_start: Option<Result<(PresentationTelemetry, Instant), String>>,
}

enum LauncherSnapshotPreparation {
    AwaitingFrame,
    Running(mpsc::Receiver<Result<PreparedLauncherSnapshot, String>>),
    Installed,
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
        matches!(
            self.snapshot_preparation,
            LauncherSnapshotPreparation::AwaitingFrame
        )
    }

    pub(super) const fn waiting_frames(&self) -> u64 {
        self.waiting_frames
    }

    pub(super) fn begin_launcher_snapshot_preparation(
        &mut self,
        launcher_pixels: &[Rgb565Pixel],
    ) -> Result<(), String> {
        if !self.snapshot_capture_needed() {
            return Ok(());
        }
        if launcher_pixels.len() != self.handoff_snapshot.len() {
            return Err(format!(
                "launcher handoff snapshot has {} pixels, expected {}",
                launcher_pixels.len(),
                self.handoff_snapshot.len()
            ));
        }
        self.handoff_snapshot.copy_from_slice(launcher_pixels);
        let native_pixels = native_launcher_snapshot(
            launcher_pixels,
            self.composition_width,
            self.composition_height,
            self.scene.geometry().width(),
            self.scene.geometry().height(),
        )?;
        let pixels = native_pixels
            .iter()
            .map(|pixel| SceneRgb565Pixel(pixel.0))
            .collect::<Vec<_>>();
        let width = self.scene.geometry().width();
        let height = self.scene.geometry().height();
        let recipe = self.scene.recipe().clone();
        let options = self.scene.options();
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("intro-snapshot".to_string())
            .spawn(move || {
                apply_runtime_thread_policy(RuntimeThreadRole::StartupIntroSnapshot);
                let prepared =
                    IntroScene::prepare_launcher_snapshot(width, height, recipe, options, pixels);
                let _ = tx.send(prepared);
            })
            .map_err(|error| format!("failed to start launcher snapshot preparation: {error}"))?;
        self.snapshot_preparation = LauncherSnapshotPreparation::Running(rx);
        Ok(())
    }

    pub(super) fn poll_launcher_snapshot_preparation(&mut self) -> Result<bool, String> {
        let LauncherSnapshotPreparation::Running(receiver) = &self.snapshot_preparation else {
            return Ok(false);
        };
        let prepared = match receiver.try_recv() {
            Ok(prepared) => prepared?,
            Err(mpsc::TryRecvError::Empty) => return Ok(false),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err("launcher snapshot preparation worker disconnected".into());
            }
        };
        self.scene.install_launcher_snapshot(prepared)?;
        self.snapshot_preparation = LauncherSnapshotPreparation::Installed;
        self.snapshot_ready = true;
        Ok(true)
    }

    pub(super) fn render_grant(
        &mut self,
        grant: HiddenSlotRenderGrant,
    ) -> Result<CompletedHiddenFrame, String> {
        if self.completed {
            return Err("startup intro rendered after completion".into());
        }
        if grant.width != self.scene.geometry().width()
            || grant.height != self.scene.geometry().height()
            || grant.stride_pixels != grant.width
        {
            return Err(format!(
                "startup intro grant geometry {}x{} stride={} does not match {}x{}",
                grant.width,
                grant.height,
                grant.stride_pixels,
                self.scene.geometry().width(),
                self.scene.geometry().height()
            ));
        }
        let waiting_for_launcher = self.elapsed >= MORPH_START && !self.snapshot_ready;
        let slot = grant
            .slot_index
            .checked_sub(1)
            .ok_or("startup intro received invalid hidden slot zero")?;
        let buffer_id = SceneBufferId::new(slot, 2).map_err(|error| error.to_string())?;
        let geometry = SceneGeometry::new(grant.width, grant.height, grant.stride_pixels)
            .map_err(|error| error.to_string())?;
        let elapsed = self.elapsed;
        let next_elapsed = (elapsed < FINAL_ELAPSED).then(|| {
            elapsed
                .saturating_add(Duration::from_micros(self.refresh_period_us))
                .min(FINAL_ELAPSED)
        });
        let buffers = self
            .buffers
            .as_mut()
            .ok_or("startup intro hidden mappings are unavailable")?;
        let buffer = buffers.buffer_mut(grant.slot_index);
        let scene_pixels = scene_pixels_mut(buffer);
        let target = SceneTarget::new(scene_pixels, geometry, buffer_id)
            .map_err(|error| error.to_string())?;
        if waiting_for_launcher {
            self.scene
                .render_waiting_for_launcher(
                    target,
                    SceneClock {
                        frame: self.frame,
                        elapsed: self.waiting_elapsed,
                        next_elapsed: Some(
                            self.waiting_elapsed
                                .saturating_add(Duration::from_micros(self.refresh_period_us)),
                        ),
                    },
                )
                .map_err(|error| error.to_string())?;
        } else {
            self.scene
                .render(
                    target,
                    SceneClock {
                        frame: self.frame,
                        elapsed,
                        next_elapsed,
                    },
                )
                .map_err(|error| error.to_string())?;
        }
        self.last_render_waiting = waiting_for_launcher;
        buffer.publish_writes();
        Ok(CompletedHiddenFrame {
            grant,
            source_evidence: None,
        })
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
        let refresh_period = Duration::from_micros(self.refresh_period_us);
        if self.last_render_waiting {
            self.waiting_frames = self.waiting_frames.saturating_add(1);
            self.waiting_elapsed = self.waiting_elapsed.saturating_add(refresh_period);
        } else {
            self.elapsed = self
                .elapsed
                .saturating_add(refresh_period)
                .min(FINAL_ELAPSED);
            self.completed = self.elapsed >= FINAL_ELAPSED;
        }
        self.frame = self.frame.saturating_add(1);
        self.completed.then_some(self.cadence())
    }

    pub(super) const fn cadence(&self) -> StartupIntroCadence {
        StartupIntroCadence {
            confirmed_frames: self.confirmed_frames,
            cabinet_wait_frames: self.waiting_frames,
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
        target.restore_presentation_cached(&self.handoff_snapshot)
    }

    pub(super) fn take_buffers(&mut self) -> Option<PluginLatchFrameBuffers> {
        self.buffers.take()
    }

    #[cfg(test)]
    pub(super) fn frame(&self) -> u64 {
        self.frame
    }

    #[cfg(test)]
    pub(super) fn elapsed(&self) -> Duration {
        self.elapsed
    }
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

fn native_launcher_snapshot(
    pixels: &[Rgb565Pixel],
    composition_width: usize,
    composition_height: usize,
    native_width: usize,
    native_height: usize,
) -> Result<Vec<Rgb565Pixel>, String> {
    if pixels.len() != composition_width.saturating_mul(composition_height) {
        return Err("launcher snapshot does not match the composition geometry".into());
    }
    if composition_width != native_width {
        return Err(format!(
            "launcher snapshot width {composition_width} does not match native width {native_width}"
        ));
    }
    if composition_height == native_height {
        return Ok(pixels.to_vec());
    }
    let transform = VerticalRgb565Transform::new(native_width, composition_height, native_height)
        .map_err(str::to_string)?;
    let mut native = vec![Rgb565Pixel(0); native_width.saturating_mul(native_height)];
    let copied = transform
        .copy_rect(
            Rgb565FrameView {
                pixels,
                width: composition_width,
                height: composition_height,
                stride_pixels: composition_width,
            },
            VerticalRect {
                x0: 0,
                y0: 0,
                x1: composition_width,
                y1: composition_height,
            },
            &mut native,
            native_width,
        )
        .map_err(str::to_string)?;
    if copied.is_none() {
        return Err("launcher snapshot transform produced no native rows".into());
    }
    Ok(native)
}

fn scene_pixels_mut(buffer: &mut ScanoutSlotsRgb565Framebuffer) -> &mut [SceneRgb565Pixel] {
    let pixels = buffer.pixels_mut();
    debug_assert_eq!(
        std::mem::size_of::<Rgb565Pixel>(),
        std::mem::size_of::<SceneRgb565Pixel>()
    );
    debug_assert_eq!(
        std::mem::align_of::<Rgb565Pixel>(),
        std::mem::align_of::<SceneRgb565Pixel>()
    );
    // SAFETY: both crates define a one-word RGB565 tuple pixel and the
    // scanout runtime already enforces the Slint pixel's u16 size/alignment at
    // this mapping boundary. The returned mutable slice cannot outlive the
    // exclusive framebuffer borrow.
    unsafe {
        std::slice::from_raw_parts_mut(pixels.as_mut_ptr().cast::<SceneRgb565Pixel>(), pixels.len())
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

    fn test_session() -> StartupIntroSession {
        let ui = UiDisplay::for_framebuffer(320, 180);
        let prepared = PreparedStartupIntro::new(&ui).unwrap();
        StartupIntroSession {
            scene: prepared.scene,
            buffers: None,
            handoff_snapshot: prepared.handoff_snapshot,
            composition_width: prepared.composition_width,
            composition_height: prepared.composition_height,
            snapshot_preparation: LauncherSnapshotPreparation::AwaitingFrame,
            frame: 0,
            elapsed: Duration::ZERO,
            waiting_elapsed: Duration::ZERO,
            refresh_period_us: prepared.initial_refresh_period_us,
            snapshot_ready: false,
            completed: false,
            confirmed_frames: 0,
            expected_refresh_intervals: 0,
            software_estimated_dropped_frames: 0,
            pacing_failures: 0,
            max_confirmation_gap_us: 0,
            last_confirmed_at: None,
            waiting_frames: 0,
            last_render_waiting: false,
            presentation_start: None,
        }
    }

    fn crt_display(route: &str) -> UiDisplay {
        let settings = format!("schema=1&output={route}");
        let plan = UiDisplayPlan::from_runtime_or_mister_ini_text(
            None,
            "[Menu]\nvideo_mode=8\n",
            Some(&settings),
            None,
        )
        .expect("supported CRT route");
        UiDisplay::for_plan(plan)
    }

    #[test]
    fn crt_intro_profiles_use_native_geometry_and_half_density() {
        for (route, height) in [
            ("crt-240p60", 240),
            ("crt-288p50", 288),
            ("crt-480p60", 480),
            ("crt-576p50", 576),
        ] {
            let ui = crt_display(route);
            let prepared = PreparedStartupIntro::new(&ui).unwrap();

            assert_eq!(
                (
                    prepared.scene.geometry().width(),
                    prepared.scene.geometry().height()
                ),
                (640, height),
                "{route}"
            );
            assert_eq!(
                prepared.scene.options().particle_density,
                IntroParticleDensity::Half,
                "{route}"
            );
            assert_eq!(
                prepared.scene.options().projection_scale,
                IntroProjectionScale::crt(height),
                "{route}"
            );
        }
    }

    #[test]
    fn crt_240_launcher_snapshot_uses_the_centered_vertical_transform() {
        let pixels = (0..480)
            .flat_map(|row| std::iter::repeat_n(Rgb565Pixel(row as u16), 640))
            .collect::<Vec<_>>();
        let native = native_launcher_snapshot(&pixels, 640, 480, 640, 240).unwrap();

        for row in [0, 1, 2, 239] {
            let expected_source_row = row * 2 + 1;
            assert!(
                native[row * 640..(row + 1) * 640]
                    .iter()
                    .all(|pixel| *pixel == Rgb565Pixel(expected_source_row as u16))
            );
        }
    }

    #[test]
    fn snapshot_failure_leaves_the_original_composition_cache_intact() {
        let mut session = test_session();
        let original = session.handoff_snapshot.clone();

        assert!(
            session
                .begin_launcher_snapshot_preparation(&[Rgb565Pixel(7)])
                .is_err()
        );
        assert_eq!(session.handoff_snapshot, original);
        assert!(session.snapshot_capture_needed());
    }

    #[test]
    fn handoff_restores_the_original_composition_cache() {
        let ui = UiDisplay::for_framebuffer(320, 180);
        let mut session = test_session();
        for (index, pixel) in session.handoff_snapshot.iter_mut().enumerate() {
            *pixel = Rgb565Pixel(index as u16);
        }
        let expected = session.handoff_snapshot.clone();
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

    #[test]
    fn cabinet_wait_frames_do_not_advance_the_morph_clock() {
        let mut session = test_session();
        session.frame = 800;
        session.elapsed = MORPH_START;
        session.last_render_waiting = true;
        let origin = Instant::now();

        assert!(
            session
                .note_confirmed_present(origin, 20_000, true)
                .is_none()
        );
        assert_eq!(session.frame(), 801);
        assert_eq!(session.elapsed(), MORPH_START);
        assert_eq!(session.waiting_elapsed, Duration::from_millis(20));
        assert_eq!(session.waiting_frames(), 1);

        session.snapshot_ready = true;
        session.last_render_waiting = false;
        assert!(
            session
                .note_confirmed_present(origin + Duration::from_micros(20_000), 20_000, true,)
                .is_none()
        );
        assert_eq!(session.frame(), 802);
        assert_eq!(session.elapsed(), MORPH_START + Duration::from_millis(20));
        assert_eq!(session.waiting_frames(), 1);
    }
}
