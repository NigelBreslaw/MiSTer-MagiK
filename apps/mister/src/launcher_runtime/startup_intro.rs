// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral first-run intro playback shared by MiSTer and macOS.

use crate::framebuffer::vertical_scale::{Rgb565FrameView, VerticalRect, VerticalRgb565Transform};
use crate::ui_display::UiDisplay;
use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use mister_magik_framebuffer_scenes::{
    FramebufferScene, Rgb565Pixel as SceneRgb565Pixel, SceneBufferId, SceneClock, SceneGeometry,
    SceneTarget,
};
use mister_magik_particles::intro::{
    IntroParticleDensity, IntroProjectionScale, IntroScene, IntroSceneOptions,
    PreparedLauncherSnapshot,
};
use mister_magik_particles::intro_recipe::embedded_intro_recipe;
use slint::platform::software_renderer::Rgb565Pixel;
use std::sync::mpsc;
use std::time::Duration;

const DEFAULT_REFRESH_PERIOD_US: u64 = 16_667;
pub const MORPH_START: Duration = Duration::from_secs(16);
pub const FINAL_ELAPSED: Duration = Duration::from_secs(20);

pub struct StartupIntroPlayback {
    scene: IntroScene,
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
    waiting_frames: u64,
    last_render_waiting: bool,
}

enum LauncherSnapshotPreparation {
    AwaitingFrame,
    Running(mpsc::Receiver<Result<PreparedLauncherSnapshot, String>>),
    Installed,
}

impl StartupIntroPlayback {
    pub fn new(ui: &UiDisplay) -> Result<Self, String> {
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
            snapshot_preparation: LauncherSnapshotPreparation::AwaitingFrame,
            frame: 0,
            elapsed: Duration::ZERO,
            waiting_elapsed: Duration::ZERO,
            refresh_period_us: ui
                .output_route()
                .nominal_period_us()
                .unwrap_or(DEFAULT_REFRESH_PERIOD_US),
            snapshot_ready: false,
            completed: false,
            waiting_frames: 0,
            last_render_waiting: false,
        })
    }

    pub fn snapshot_capture_needed(&self) -> bool {
        matches!(
            self.snapshot_preparation,
            LauncherSnapshotPreparation::AwaitingFrame
        )
    }

    pub fn begin_launcher_snapshot_preparation(
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

    pub fn poll_launcher_snapshot_preparation(&mut self) -> Result<bool, String> {
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

    pub fn render_into(
        &mut self,
        pixels: &mut [Rgb565Pixel],
        buffer_index: u8,
        stride_pixels: usize,
    ) -> Result<(), String> {
        if self.completed {
            return Err("startup intro rendered after completion".into());
        }
        let geometry = self.scene.geometry();
        if stride_pixels != geometry.width()
            || pixels.len() != stride_pixels.saturating_mul(geometry.height())
        {
            return Err(format!(
                "startup intro target has {} pixels at stride {}, expected {}x{}",
                pixels.len(),
                stride_pixels,
                geometry.width(),
                geometry.height()
            ));
        }
        let buffer_id = SceneBufferId::new(buffer_index, 2).map_err(|error| error.to_string())?;
        let target_geometry =
            SceneGeometry::new(geometry.width(), geometry.height(), stride_pixels)
                .map_err(|error| error.to_string())?;
        let waiting_for_launcher = self.elapsed >= MORPH_START && !self.snapshot_ready;
        let elapsed = self.elapsed;
        let next_elapsed = (elapsed < FINAL_ELAPSED).then(|| {
            elapsed
                .saturating_add(Duration::from_micros(self.refresh_period_us))
                .min(FINAL_ELAPSED)
        });
        let target = SceneTarget::new(scene_pixels_mut(pixels), target_geometry, buffer_id)
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
        Ok(())
    }

    pub fn note_presented(&mut self, refresh_period_us: u64) -> bool {
        if refresh_period_us != 0 {
            self.refresh_period_us = refresh_period_us;
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
        self.completed
    }

    pub fn restore_handoff_snapshot(&self, target: &mut [Rgb565Pixel]) -> bool {
        if target.len() != self.handoff_snapshot.len() {
            return false;
        }
        target.copy_from_slice(&self.handoff_snapshot);
        true
    }

    pub fn handoff_snapshot(&self) -> &[Rgb565Pixel] {
        &self.handoff_snapshot
    }

    pub fn geometry(&self) -> SceneGeometry {
        self.scene.geometry()
    }

    pub fn options(&self) -> IntroSceneOptions {
        self.scene.options()
    }

    pub const fn frame(&self) -> u64 {
        self.frame
    }

    pub const fn elapsed(&self) -> Duration {
        self.elapsed
    }

    pub const fn refresh_period_us(&self) -> u64 {
        self.refresh_period_us
    }

    pub const fn waiting_frames(&self) -> u64 {
        self.waiting_frames
    }

    pub const fn last_render_waiting(&self) -> bool {
        self.last_render_waiting
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

fn scene_pixels_mut(pixels: &mut [Rgb565Pixel]) -> &mut [SceneRgb565Pixel] {
    debug_assert_eq!(
        std::mem::size_of::<Rgb565Pixel>(),
        std::mem::size_of::<SceneRgb565Pixel>()
    );
    debug_assert_eq!(
        std::mem::align_of::<Rgb565Pixel>(),
        std::mem::align_of::<SceneRgb565Pixel>()
    );
    // SAFETY: both pixel types are transparent one-word RGB565 tuple structs.
    unsafe {
        std::slice::from_raw_parts_mut(pixels.as_mut_ptr().cast::<SceneRgb565Pixel>(), pixels.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_display::UiDisplayPlan;

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
    fn crt_profiles_use_native_geometry_and_half_density() {
        for (route, height) in [
            ("crt-240p60", 240),
            ("crt-288p50", 288),
            ("crt-480p60", 480),
            ("crt-576p50", 576),
        ] {
            let playback = StartupIntroPlayback::new(&crt_display(route)).unwrap();
            assert_eq!(
                (playback.geometry().width(), playback.geometry().height()),
                (640, height)
            );
            assert_eq!(
                playback.options().particle_density,
                IntroParticleDensity::Half
            );
            assert_eq!(
                playback.options().projection_scale,
                IntroProjectionScale::crt(height)
            );
        }
    }

    #[test]
    fn snapshot_transform_centres_480_lines_into_240_lines() {
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
    fn handoff_restore_requires_the_composition_geometry() {
        let ui = UiDisplay::for_framebuffer(320, 180);
        let mut playback = StartupIntroPlayback::new(&ui).unwrap();
        let expected = vec![Rgb565Pixel(0x1234); 320 * 180];
        playback
            .begin_launcher_snapshot_preparation(&expected)
            .unwrap();
        let mut wrong = vec![Rgb565Pixel(0); 1];
        assert!(!playback.restore_handoff_snapshot(&mut wrong));
        let mut restored = vec![Rgb565Pixel(0); expected.len()];
        assert!(playback.restore_handoff_snapshot(&mut restored));
        assert_eq!(restored, expected);
    }

    #[test]
    fn cabinet_wait_frames_do_not_advance_the_storyboard_clock() {
        let ui = UiDisplay::for_framebuffer(320, 180);
        let mut playback = StartupIntroPlayback::new(&ui).unwrap();
        playback.frame = 800;
        playback.elapsed = MORPH_START;
        playback.last_render_waiting = true;

        assert!(!playback.note_presented(20_000));
        assert_eq!(playback.frame(), 801);
        assert_eq!(playback.elapsed(), MORPH_START);
        assert_eq!(playback.waiting_elapsed, Duration::from_millis(20));
        assert_eq!(playback.waiting_frames(), 1);

        playback.snapshot_ready = true;
        playback.last_render_waiting = false;
        assert!(!playback.note_presented(20_000));
        assert_eq!(playback.frame(), 802);
        assert_eq!(playback.elapsed(), MORPH_START + Duration::from_millis(20));
        assert_eq!(playback.waiting_frames(), 1);
    }
}
