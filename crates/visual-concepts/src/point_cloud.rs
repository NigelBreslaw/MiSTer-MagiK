// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{Effect, Pixel, Preset, Rect, full};
use mister_magik_framebuffer_scenes::SceneBufferId;
use mister_magik_particles::{
    cabinet::ArcadeCabinetFormation,
    engine::ParticlePreset,
    magik::MagikScene,
    recipes::{embedded_cabinet_recipe, embedded_magik_recipe},
};
use std::time::Duration;
pub struct Morph {
    cabinet: ArcadeCabinetFormation,
    logo: MagikScene,
    width: usize,
    height: usize,
    logo_bytes: usize,
    last_logo: bool,
}
pub fn new(preset: Preset, width: usize, height: usize) -> Result<Morph, String> {
    let count = preset.choose(8192, 4096);
    let mut cabinet = embedded_cabinet_recipe()?;
    cabinet.particle_count = count;
    cabinet.timing.formation_ms = 2500;
    cabinet.timing.orbit_ms = 5000;
    cabinet.timing.return_ms = 1000;
    cabinet.timing.disperse_ms = 1500;
    cabinet.timing.cycle_ms = 10000;
    let mut logo = embedded_magik_recipe()?;
    logo.particle_count = count;
    logo.timing.static_ms = 0;
    logo.timing.form_ms = 2500;
    logo.timing.hold_ms = 6000;
    logo.timing.disperse_ms = 1500;
    logo.timing.cycle_ms = 10000;
    Ok(Morph {
        cabinet: ArcadeCabinetFormation::new(width, height, cabinet)?,
        logo: MagikScene::from_magik_recipe(width, height, ParticlePreset::Visual, logo)?,
        width,
        height,
        logo_bytes: 0,
        last_logo: false,
    })
}
impl Effect for Morph {
    fn render(&mut self, elapsed: Duration, pixels: &mut [Pixel]) -> Result<Rect, String> {
        let ms = elapsed.as_millis() as u64 % 20000;
        let logo = ms >= 10000;
        let id = SceneBufferId::new(0, 2).map_err(|e| e.to_string())?;
        if logo != self.last_logo || ms == 0 {
            pixels.fill(Pixel(0));
            self.logo.invalidate(id);
        }
        if logo {
            let at = Duration::from_millis(ms - 10000);
            let stats = self.logo.render_with_lookahead(
                pixels,
                id,
                at,
                Some(at + Duration::from_nanos(16_666_667)),
            )?;
            self.logo_bytes = stats.simulation_bytes + stats.renderer_scratch_bytes;
        } else {
            self.cabinet.render(pixels, Duration::from_millis(ms), 0)?;
        }
        self.last_logo = logo;
        Ok(full(self.width, self.height))
    }
    fn storage_bytes(&self) -> usize {
        self.cabinet.allocated_bytes() + self.logo_bytes
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cabinet_and_logo_phases_render() {
        let mut e = new(Preset::Reduced, 320, 180).unwrap();
        let mut p = vec![Pixel(0); 320 * 180];
        for ms in [0, 2500, 9000, 10000, 12500, 19999, 20000] {
            e.render(Duration::from_millis(ms), &mut p).unwrap();
        }
        assert!(e.storage_bytes() > 0);
    }
}
