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
use mister_magik_particles::intro::{
    IntroParticleDensity, IntroProjectionScale, IntroScene, IntroSceneOptions,
    PreparedLauncherSnapshot,
};
use mister_magik_particles::intro_recipe::embedded_intro_recipe;

const INTRO_FPS: u64 = 60;
const MORPH_FRAME: u64 = 16 * INTRO_FPS;
const FINAL_FRAME: u64 = 20 * INTRO_FPS;

pub(super) struct PreparedStartupIntro {
    scene: IntroScene,
    handoff_snapshot: Vec<Rgb565Pixel>,
    composition_width: usize,
    composition_height: usize,
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
            snapshot_ready: false,
            completed: false,
            confirmed_frames: 0,
            expected_refresh_intervals: 0,
            skipped_refreshes: 0,
            pacing_failures: 0,
            max_confirmation_gap_us: 0,
            last_confirmed_at: None,
            waiting_frames: 0,
            last_render_waiting: false,
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
    snapshot_ready: bool,
    completed: bool,
    confirmed_frames: u64,
    expected_refresh_intervals: u64,
    skipped_refreshes: u64,
    pacing_failures: u64,
    max_confirmation_gap_us: u64,
    last_confirmed_at: Option<Instant>,
    waiting_frames: u64,
    last_render_waiting: bool,
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
    pub(super) skipped_refreshes: u64,
    pub(super) pacing_failures: u64,
    pub(super) max_confirmation_gap_us: u64,
}

impl StartupIntroSession {
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
        let waiting_for_launcher = self.frame >= MORPH_FRAME && !self.snapshot_ready;
        let slot = grant
            .slot_index
            .checked_sub(1)
            .ok_or("startup intro received invalid hidden slot zero")?;
        let buffer_id = SceneBufferId::new(slot, 2).map_err(|error| error.to_string())?;
        let geometry = SceneGeometry::new(grant.width, grant.height, grant.stride_pixels)
            .map_err(|error| error.to_string())?;
        let elapsed = intro_frame_elapsed(self.frame);
        let next_elapsed =
            (self.frame < FINAL_FRAME).then(|| intro_frame_elapsed(self.frame.saturating_add(1)));
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
                .render_waiting_for_launcher(target, self.waiting_frames)
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
        Ok(CompletedHiddenFrame { grant })
    }

    /// Advances only after the latch reports this sequence active at the
    /// physical scanout boundary. Latch protocol drops and missed refreshes
    /// are deliberately separate signals: a healthy latch may still repeat a
    /// frame when rendering takes longer than one refresh interval.
    pub(super) fn note_confirmed_present(
        &mut self,
        confirmed_at: Instant,
        refresh_period_us: u64,
        vsync_confirmed: bool,
    ) -> Option<StartupIntroCadence> {
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
            self.skipped_refreshes = self
                .skipped_refreshes
                .saturating_add(expected.saturating_sub(1));
        }
        if self.last_render_waiting {
            self.waiting_frames = self.waiting_frames.saturating_add(1);
        } else if self.frame >= FINAL_FRAME {
            self.completed = true;
        } else {
            self.frame = self.frame.saturating_add(1);
        }
        self.completed.then_some(self.cadence())
    }

    pub(super) const fn cadence(&self) -> StartupIntroCadence {
        StartupIntroCadence {
            confirmed_frames: self.confirmed_frames,
            cabinet_wait_frames: self.waiting_frames,
            expected_refresh_intervals: self.expected_refresh_intervals,
            skipped_refreshes: self.skipped_refreshes,
            pacing_failures: self.pacing_failures,
            max_confirmation_gap_us: self.max_confirmation_gap_us,
        }
    }

    pub(super) fn restore_handoff_snapshot(&self, target: &mut LayerTarget<'_>) -> bool {
        target.restore_cached(&self.handoff_snapshot)
    }

    pub(super) fn take_buffers(&mut self) -> Option<PluginLatchFrameBuffers> {
        self.buffers.take()
    }

    #[cfg(test)]
    pub(super) fn frame(&self) -> u64 {
        self.frame
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

fn intro_frame_elapsed(frame: u64) -> Duration {
    Duration::from_nanos(frame.saturating_mul(1_000_000_000) / INTRO_FPS)
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
            snapshot_ready: false,
            completed: false,
            confirmed_frames: 0,
            expected_refresh_intervals: 0,
            skipped_refreshes: 0,
            pacing_failures: 0,
            max_confirmation_gap_us: 0,
            last_confirmed_at: None,
            waiting_frames: 0,
            last_render_waiting: false,
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
    fn rational_clock_hits_exact_storyboard_boundaries() {
        assert_eq!(intro_frame_elapsed(MORPH_FRAME), Duration::from_secs(16));
        assert_eq!(intro_frame_elapsed(FINAL_FRAME), Duration::from_secs(20));
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
            for frame in 0..=FINAL_FRAME {
                let skipped_us = u64::from(skip_at.is_some_and(|at| frame >= at)) * period_us;
                completed = session.note_confirmed_present(
                    origin + Duration::from_micros(frame * period_us + skipped_us),
                    period_us,
                    true,
                );
            }
            completed.unwrap()
        };

        let exact = run(None);
        assert_eq!(exact.confirmed_frames, FINAL_FRAME + 1);
        assert_eq!(exact.expected_refresh_intervals, FINAL_FRAME);
        assert_eq!(exact.skipped_refreshes, 0);
        assert_eq!(exact.pacing_failures, 0);

        let skipped = run(Some(600));
        assert_eq!(skipped.confirmed_frames, FINAL_FRAME + 1);
        assert_eq!(skipped.expected_refresh_intervals, FINAL_FRAME + 1);
        assert_eq!(skipped.skipped_refreshes, 1);
        assert_eq!(skipped.pacing_failures, 0);
    }

    #[test]
    fn cabinet_wait_frames_do_not_advance_the_morph_clock() {
        let mut session = test_session();
        session.frame = MORPH_FRAME;
        session.last_render_waiting = true;
        let origin = Instant::now();

        assert!(
            session
                .note_confirmed_present(origin, 16_667, true)
                .is_none()
        );
        assert_eq!(session.frame(), MORPH_FRAME);
        assert_eq!(session.waiting_frames(), 1);

        session.snapshot_ready = true;
        session.last_render_waiting = false;
        assert!(
            session
                .note_confirmed_present(origin + Duration::from_micros(16_667), 16_667, true,)
                .is_none()
        );
        assert_eq!(session.frame(), MORPH_FRAME + 1);
        assert_eq!(session.waiting_frames(), 1);
    }
}
