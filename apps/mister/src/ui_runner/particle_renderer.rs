// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::bitmap_text::{ConsoleFont, ConsoleTypeface};
use mister_magik_fb::particle_engine::{
    ParticleConfig, ParticleEngine, ParticleFrameStats, ParticlePhase, ParticlePreset, TargetMask,
};
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::{Duration, Instant};

const MAGIK_FONT_PX: f32 = 128.0;
const MAGIK_TEXT: &str = "MagiK";
const MAGIK_MASK_THRESHOLD: u8 = 128;
const MAGIK_MASK_SAMPLE_STEP: usize = 2;
const CAPACITY_COLOR: Rgb565Pixel = Rgb565Pixel(0xbdf7);
const VISUAL_PALETTE: [Rgb565Pixel; 4] = [
    Rgb565Pixel(0x2104),
    Rgb565Pixel(0x5aeb),
    Rgb565Pixel(0xbdf7),
    Rgb565Pixel(0xffff),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ParticleRenderStats {
    pub(super) count: usize,
    pub(super) visible: usize,
    pub(super) phase: ParticlePhase,
    pub(super) cycle: u64,
    pub(super) simulation_us: u128,
    pub(super) clear_us: u128,
    pub(super) raster_us: u128,
}

pub(super) struct ParticleRenderer {
    engine: ParticleEngine,
}

impl ParticleRenderer {
    pub(super) fn new_magik(config: ParticleConfig) -> Result<Self, String> {
        let mask = magik_target_mask()?;
        Ok(Self {
            engine: ParticleEngine::new(config, mask)?,
        })
    }

    #[cfg(test)]
    fn new(config: ParticleConfig, mask: TargetMask) -> Result<Self, String> {
        Ok(Self {
            engine: ParticleEngine::new(config, mask)?,
        })
    }

    pub(super) fn preset(&self) -> ParticlePreset {
        self.engine.config().preset
    }

    pub(super) fn particle_count(&self) -> usize {
        self.engine.particle_count()
    }

    pub(super) fn render(
        &mut self,
        destination: &mut [Rgb565Pixel],
        elapsed: Duration,
    ) -> Result<ParticleRenderStats, String> {
        let config = self.engine.config();
        let frame_len = config.width.saturating_mul(config.height);
        if destination.len() != frame_len {
            return Err(format!(
                "particle destination has {} pixels, expected {frame_len}",
                destination.len()
            ));
        }
        let simulation_started = Instant::now();
        let frame = self.engine.step(elapsed);
        let simulation_us = simulation_started.elapsed().as_micros();
        let clear_started = Instant::now();
        destination.fill(Rgb565Pixel(0));
        let clear_us = clear_started.elapsed().as_micros();
        let raster_started = Instant::now();
        let visible = self.raster(destination);
        let raster_us = raster_started.elapsed().as_micros();
        Ok(stats(frame, visible, simulation_us, clear_us, raster_us))
    }

    fn raster(&self, destination: &mut [Rgb565Pixel]) -> usize {
        let config = self.engine.config();
        let mut visible = 0usize;
        for index in 0..self.engine.particle_count() {
            let Some(particle) = self.engine.project(index) else {
                continue;
            };
            visible += 1;
            let offset = particle.y as usize * config.width + particle.x as usize;
            match config.preset {
                ParticlePreset::Capacity => destination[offset] = CAPACITY_COLOR,
                ParticlePreset::Visual => {
                    let palette_index = (particle.brightness_key >> 30) as usize;
                    destination[offset] = VISUAL_PALETTE[palette_index];
                    if palette_index == VISUAL_PALETTE.len() - 1
                        && particle.x + 1 < config.width as i32
                    {
                        destination[offset + 1] = VISUAL_PALETTE[2];
                    }
                }
            }
        }
        visible
    }
}

fn magik_target_mask() -> Result<TargetMask, String> {
    let mut font = ConsoleFont::new_with_typeface(MAGIK_FONT_PX, ConsoleTypeface::PressStart2P);
    let alpha = font
        .rasterize_alpha_mask(MAGIK_TEXT)
        .ok_or("Press Start 2P produced no MagiK alpha mask")?;
    TargetMask::from_alpha(
        alpha.width,
        alpha.height,
        alpha.stride,
        &alpha.alpha,
        MAGIK_MASK_THRESHOLD,
        MAGIK_MASK_SAMPLE_STEP,
    )
}

fn stats(
    frame: ParticleFrameStats,
    visible: usize,
    simulation_us: u128,
    clear_us: u128,
    raster_us: u128,
) -> ParticleRenderStats {
    ParticleRenderStats {
        count: frame.count,
        visible,
        phase: frame.phase,
        cycle: frame.cycle,
        simulation_us,
        clear_us,
        raster_us,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask() -> TargetMask {
        TargetMask::from_alpha(4, 4, 4, &[255; 16], 128, 1).unwrap()
    }

    fn config(preset: ParticlePreset) -> ParticleConfig {
        ParticleConfig {
            count: 64,
            width: 32,
            height: 24,
            seed: 7,
            preset,
        }
    }

    fn render_at_hold(renderer: &mut ParticleRenderer) -> Vec<Rgb565Pixel> {
        let mut pixels = vec![Rgb565Pixel(0); 32 * 24];
        for frame in 0..=360 {
            renderer
                .render(&mut pixels, Duration::from_micros(frame * 16_667))
                .unwrap();
        }
        pixels
    }

    #[test]
    fn magik_mask_fits_the_960_by_540_viewport() {
        let mask = magik_target_mask().unwrap();
        assert!(mask.width() < 960);
        assert!(mask.height() < 540);
        assert!(mask.points().len() > 1_000);
    }

    #[test]
    fn capacity_preset_draws_only_single_particle_pixels() {
        let mut renderer = ParticleRenderer::new(config(ParticlePreset::Capacity), mask()).unwrap();
        let pixels = render_at_hold(&mut renderer);
        assert!(pixels.iter().any(|pixel| *pixel == CAPACITY_COLOR));
        assert!(pixels.iter().all(|pixel| matches!(pixel.0, 0 | 0xbdf7)));
    }

    #[test]
    fn visual_preset_uses_the_phosphor_palette() {
        let mut renderer = ParticleRenderer::new(config(ParticlePreset::Visual), mask()).unwrap();
        let pixels = render_at_hold(&mut renderer);
        assert!(pixels.iter().any(|pixel| VISUAL_PALETTE.contains(pixel)));
        assert!(
            pixels
                .iter()
                .all(|pixel| pixel.0 == 0 || VISUAL_PALETTE.contains(pixel))
        );
    }

    #[test]
    fn destination_geometry_must_match_exactly() {
        let mut renderer = ParticleRenderer::new(config(ParticlePreset::Capacity), mask()).unwrap();
        assert!(
            renderer
                .render(&mut [Rgb565Pixel(0); 1], Duration::ZERO)
                .unwrap_err()
                .contains("expected 768")
        );
    }
}
