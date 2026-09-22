// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    Effect, Pixel, Preset, Rect,
    fixture::{Fixture, blend},
    rgb,
};
use std::time::Duration;
pub struct Light {
    f: Fixture,
    phases: Vec<Vec<Pixel>>,
}
pub fn new(preset: Preset, width: usize, height: usize) -> Result<Light, String> {
    let f = Fixture::new(width, height);
    let r = f.card;
    let count = preset.choose(16, 8);
    let phases = (0..count)
        .map(|phase| {
            let mut p = Vec::with_capacity(r.width() * r.height());
            for y in 0..r.height() {
                for x in 0..r.width() {
                    let line = phase as i32 * (r.width() + r.height() + 80) as i32
                        / (count - 1) as i32
                        - 40;
                    let distance = (x as i32 + y as i32 - line).unsigned_abs() as usize;
                    let highlight = 32usize.saturating_sub(distance) * 3;
                    p.push(blend(
                        f.base[(y + r.y0) * width + x + r.x0],
                        rgb(255, 238, 220),
                        highlight as u16,
                    ));
                }
            }
            p
        })
        .collect();
    Ok(Light { f, phases })
}
impl Effect for Light {
    fn render(&mut self, t: Duration, p: &mut [Pixel]) -> Result<Rect, String> {
        p.copy_from_slice(&self.f.base);
        let r = self.f.card;
        let phase = (t.as_millis() % 3000) as usize * (self.phases.len() - 1) * 256 / 3000;
        let a = phase / 256;
        let b = (a + 1).min(self.phases.len() - 1);
        for y in 0..r.height() {
            for x in 0..r.width() {
                let i = y * r.width() + x;
                p[(y + r.y0) * self.f.width + x + r.x0] =
                    blend(self.phases[a][i], self.phases[b][i], (phase % 256) as u16);
            }
        }
        Ok(r)
    }
    fn storage_bytes(&self) -> usize {
        self.f.storage_bytes() + self.phases.iter().map(|p| p.capacity() * 2).sum::<usize>()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lighting_only_changes_selected_card() {
        let mut e = new(Preset::Reduced, 960, 540).unwrap();
        let mut p = vec![Pixel(0); 960 * 540];
        let r = e.render(Duration::from_millis(1400), &mut p).unwrap();
        for y in 0..540 {
            for x in 0..960 {
                if x < r.x0 || x >= r.x1 || y < r.y0 || y >= r.y1 {
                    assert_eq!(p[y * 960 + x], e.f.base[y * 960 + x]);
                }
            }
        }
        assert_ne!(p, e.f.base);
    }
}
