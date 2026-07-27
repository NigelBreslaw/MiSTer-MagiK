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
const HIDDEN_SLOT_COUNT: usize = 2;
const FULL_CLEAR_DIRTY_DIVISOR: usize = 4;

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
    dirty_slots: [ParticleDirtySlot; HIDDEN_SLOT_COUNT],
}

struct ParticleDirtySlot {
    initialized: bool,
    offsets: Vec<u32>,
}

impl ParticleRenderer {
    pub(super) fn new_magik(config: ParticleConfig) -> Result<Self, String> {
        let mask = magik_target_mask()?;
        Self::new(config, mask)
    }

    fn new(config: ParticleConfig, mask: TargetMask) -> Result<Self, String> {
        let write_capacity = match config.preset {
            ParticlePreset::Capacity => config.count,
            ParticlePreset::Visual => config.count.saturating_mul(2),
        };
        Ok(Self {
            engine: ParticleEngine::new(config, mask)?,
            dirty_slots: std::array::from_fn(|_| ParticleDirtySlot {
                initialized: false,
                offsets: Vec::with_capacity(write_capacity),
            }),
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
        hidden_slot: u8,
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
        let slot_offset = hidden_slot_offset(hidden_slot)?;
        let simulation_started = Instant::now();
        let frame = self.engine.step(elapsed);
        let simulation_us = simulation_started.elapsed().as_micros();
        let clear_started = Instant::now();
        let mut dirty_offsets = self.prepare_hidden_slot(destination, slot_offset);
        let clear_us = clear_started.elapsed().as_micros();
        let raster_started = Instant::now();
        let visible = self.raster(destination, &mut dirty_offsets);
        let raster_us = raster_started.elapsed().as_micros();
        self.dirty_slots[slot_offset].offsets = dirty_offsets;
        Ok(stats(frame, visible, simulation_us, clear_us, raster_us))
    }

    pub(super) fn invalidate_hidden_slot(&mut self, hidden_slot: u8) {
        if let Ok(slot_offset) = hidden_slot_offset(hidden_slot) {
            let slot = &mut self.dirty_slots[slot_offset];
            slot.initialized = false;
            slot.offsets.clear();
        }
    }

    fn prepare_hidden_slot(
        &mut self,
        destination: &mut [Rgb565Pixel],
        slot_offset: usize,
    ) -> Vec<u32> {
        let slot = &mut self.dirty_slots[slot_offset];
        if !slot.initialized || slot.offsets.len() >= destination.len() / FULL_CLEAR_DIRTY_DIVISOR {
            destination.fill(Rgb565Pixel(0));
        } else {
            for &offset in &slot.offsets {
                destination[offset as usize] = Rgb565Pixel(0);
            }
        }
        slot.initialized = true;
        let mut offsets = std::mem::take(&mut slot.offsets);
        offsets.clear();
        offsets
    }

    fn raster(&self, destination: &mut [Rgb565Pixel], dirty_offsets: &mut Vec<u32>) -> usize {
        match self.engine.config().preset {
            ParticlePreset::Capacity => self.raster_capacity(destination, dirty_offsets),
            ParticlePreset::Visual => self.raster_visual(destination, dirty_offsets),
        }
    }

    fn raster_capacity(
        &self,
        destination: &mut [Rgb565Pixel],
        dirty_offsets: &mut Vec<u32>,
    ) -> usize {
        let width = self.engine.config().width;
        let mut visible = 0usize;
        for index in 0..self.engine.particle_count() {
            let Some(particle) = self.engine.project(index) else {
                continue;
            };
            visible += 1;
            let offset = particle.y as usize * width + particle.x as usize;
            destination[offset] = CAPACITY_COLOR;
            dirty_offsets.push(offset as u32);
        }
        visible
    }

    fn raster_visual(
        &self,
        destination: &mut [Rgb565Pixel],
        dirty_offsets: &mut Vec<u32>,
    ) -> usize {
        let width = self.engine.config().width;
        let mut visible = 0usize;
        for index in 0..self.engine.particle_count() {
            let Some(particle) = self.engine.project(index) else {
                continue;
            };
            visible += 1;
            let offset = particle.y as usize * width + particle.x as usize;
            let palette_index = (self.engine.flicker_key(index) >> 30) as usize;
            destination[offset] = VISUAL_PALETTE[palette_index];
            dirty_offsets.push(offset as u32);
            if palette_index == VISUAL_PALETTE.len() - 1 && particle.x + 1 < width as i32 {
                destination[offset + 1] = VISUAL_PALETTE[2];
                dirty_offsets.push((offset + 1) as u32);
            }
        }
        visible
    }
}

fn hidden_slot_offset(hidden_slot: u8) -> Result<usize, String> {
    match hidden_slot {
        1 | 2 => Ok(usize::from(hidden_slot - 1)),
        _ => Err(format!(
            "particle hidden slot must be 1 or 2, got {hidden_slot}"
        )),
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
                .render(&mut pixels, 1, Duration::from_micros(frame * 16_667))
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
                .render(&mut [Rgb565Pixel(0); 1], 1, Duration::ZERO)
                .unwrap_err()
                .contains("expected 768")
        );
    }

    #[test]
    fn hidden_slots_clear_only_their_own_previous_pixels() {
        let mut renderer = ParticleRenderer::new(config(ParticlePreset::Capacity), mask()).unwrap();
        renderer.dirty_slots[0].initialized = true;
        renderer.dirty_slots[0].offsets.extend([1, 3]);
        renderer.dirty_slots[1].initialized = true;
        renderer.dirty_slots[1].offsets.push(2);
        let mut slot1 = [Rgb565Pixel(0x1234); 32 * 24];
        let mut slot2 = [Rgb565Pixel(0x5678); 32 * 24];
        let _slot1_offsets = renderer.prepare_hidden_slot(&mut slot1, 0);
        let _slot2_offsets = renderer.prepare_hidden_slot(&mut slot2, 1);
        assert_eq!(slot1[1], Rgb565Pixel(0));
        assert_eq!(slot1[3], Rgb565Pixel(0));
        assert_eq!(slot2[2], Rgb565Pixel(0));
        assert_eq!(slot1[2], Rgb565Pixel(0x1234));
        assert_eq!(slot2[1], Rgb565Pixel(0x5678));
    }

    #[test]
    fn visual_dirty_history_records_every_written_pixel_without_growth() {
        let mut renderer = ParticleRenderer::new(config(ParticlePreset::Visual), mask()).unwrap();
        let capacities = renderer
            .dirty_slots
            .each_ref()
            .map(|slot| slot.offsets.capacity());
        let mut pixels = vec![Rgb565Pixel(0); 32 * 24];
        renderer
            .render(&mut pixels, 1, Duration::from_secs(6))
            .unwrap();
        for (offset, pixel) in pixels.iter().enumerate() {
            if *pixel != Rgb565Pixel(0) {
                assert!(renderer.dirty_slots[0].offsets.contains(&(offset as u32)));
            }
        }
        renderer
            .render(&mut pixels, 1, Duration::from_micros(6_016_667))
            .unwrap();
        assert_eq!(
            capacities,
            renderer
                .dirty_slots
                .each_ref()
                .map(|slot| slot.offsets.capacity())
        );
    }

    #[test]
    fn invalidation_forces_a_full_clear_on_next_slot_use() {
        let mut renderer = ParticleRenderer::new(config(ParticlePreset::Capacity), mask()).unwrap();
        let mut pixels = vec![Rgb565Pixel(0); 32 * 24];
        renderer.render(&mut pixels, 1, Duration::ZERO).unwrap();
        pixels[767] = Rgb565Pixel(0xffff);
        renderer.invalidate_hidden_slot(1);
        let _offsets = renderer.prepare_hidden_slot(&mut pixels, 0);
        assert_eq!(pixels[767], Rgb565Pixel(0));
    }

    #[test]
    fn dense_dirty_history_uses_the_adaptive_full_clear() {
        let mut renderer = ParticleRenderer::new(config(ParticlePreset::Capacity), mask()).unwrap();
        renderer.dirty_slots[0].initialized = true;
        renderer.dirty_slots[0].offsets.extend(0..192);
        let mut pixels = vec![Rgb565Pixel(0xffff); 32 * 24];
        let _offsets = renderer.prepare_hidden_slot(&mut pixels, 0);
        assert!(pixels.iter().all(|pixel| *pixel == Rgb565Pixel(0)));
    }

    #[test]
    fn hidden_slot_indices_are_strictly_bounded() {
        assert!(hidden_slot_offset(0).is_err());
        assert_eq!(hidden_slot_offset(1), Ok(0));
        assert_eq!(hidden_slot_offset(2), Ok(1));
        assert!(hidden_slot_offset(3).is_err());
    }
}
