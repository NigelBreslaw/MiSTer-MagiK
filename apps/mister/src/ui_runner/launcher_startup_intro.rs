// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! First-run startup intro presentation over the production hidden-slot latch.

use super::*;
use mister_magik_framebuffer_scenes::{
    FramebufferScene, Rgb565Pixel as SceneRgb565Pixel, SceneBufferId, SceneClock, SceneGeometry,
    SceneTarget,
};
use mister_magik_particles::intro::IntroScene;
use mister_magik_particles::intro_recipe::embedded_intro_recipe;

const INTRO_FPS: u64 = 60;
const SNAPSHOT_FRAME: u64 = 18 * INTRO_FPS;
const FINAL_FRAME: u64 = 20 * INTRO_FPS;

pub(super) struct PreparedStartupIntro {
    scene: IntroScene,
    handoff_snapshot: Vec<Rgb565Pixel>,
    scene_handoff_snapshot: Vec<SceneRgb565Pixel>,
}

impl PreparedStartupIntro {
    pub(super) fn new(width: usize, height: usize) -> Result<Self, String> {
        let recipe = embedded_intro_recipe()?;
        let scene = IntroScene::new(width, height, recipe)?;
        Ok(Self {
            scene,
            handoff_snapshot: vec![Rgb565Pixel(0); width.saturating_mul(height)],
            scene_handoff_snapshot: vec![SceneRgb565Pixel(0); width.saturating_mul(height)],
        })
    }

    pub(super) fn attach(self, buffers: PluginLatchFrameBuffers) -> StartupIntroSession {
        StartupIntroSession {
            scene: self.scene,
            buffers: Some(buffers),
            handoff_snapshot: self.handoff_snapshot,
            scene_handoff_snapshot: self.scene_handoff_snapshot,
            frame: 0,
            snapshot_ready: false,
            completed: false,
        }
    }
}

pub(super) struct StartupIntroSession {
    scene: IntroScene,
    buffers: Option<PluginLatchFrameBuffers>,
    handoff_snapshot: Vec<Rgb565Pixel>,
    scene_handoff_snapshot: Vec<SceneRgb565Pixel>,
    frame: u64,
    snapshot_ready: bool,
    completed: bool,
}

impl StartupIntroSession {
    pub(super) fn snapshot_due(&self) -> bool {
        !self.snapshot_ready && self.frame >= SNAPSHOT_FRAME
    }

    pub(super) fn install_launcher_snapshot(
        &mut self,
        launcher_pixels: &[Rgb565Pixel],
    ) -> Result<(), String> {
        if launcher_pixels.len() != self.handoff_snapshot.len() {
            return Err(format!(
                "launcher handoff snapshot has {} pixels, expected {}",
                launcher_pixels.len(),
                self.handoff_snapshot.len()
            ));
        }
        self.handoff_snapshot.copy_from_slice(launcher_pixels);
        for (scene_pixel, launcher_pixel) in
            self.scene_handoff_snapshot.iter_mut().zip(launcher_pixels)
        {
            scene_pixel.0 = launcher_pixel.0;
        }
        self.scene
            .replace_launcher_snapshot(&self.scene_handoff_snapshot)?;
        self.snapshot_ready = true;
        Ok(())
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
        if self.frame >= SNAPSHOT_FRAME && !self.snapshot_ready {
            return Err("startup intro reached handoff cue without a launcher snapshot".into());
        }
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
        self.scene
            .render(
                SceneTarget::new(scene_pixels, geometry, buffer_id)
                    .map_err(|error| error.to_string())?,
                SceneClock {
                    frame: self.frame,
                    elapsed,
                    next_elapsed,
                },
            )
            .map_err(|error| error.to_string())?;
        buffer.publish_writes();
        Ok(CompletedHiddenFrame { grant })
    }

    /// Advances only after the presenter confirms that the direct frame was
    /// posted successfully. A missed grant or failed presentation therefore
    /// retries the same logical 60 Hz timestamp.
    pub(super) fn note_presented(&mut self) -> bool {
        if self.frame >= FINAL_FRAME {
            self.completed = true;
        } else {
            self.frame = self.frame.saturating_add(1);
        }
        self.completed
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_clock_hits_exact_storyboard_boundaries() {
        assert_eq!(intro_frame_elapsed(SNAPSHOT_FRAME), Duration::from_secs(18));
        assert_eq!(intro_frame_elapsed(FINAL_FRAME), Duration::from_secs(20));
    }
}
