// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{Effect, Pixel, Preset, Rect, fixture::Fixture};
use std::time::Duration;
pub struct Dissolve {
    f: Fixture,
    tile: usize,
}
pub fn new(preset: Preset, w: usize, h: usize) -> Result<Dissolve, String> {
    Ok(Dissolve {
        f: Fixture::new(w, h),
        tile: preset.choose(8, 16),
    })
}
impl Effect for Dissolve {
    fn render(&mut self, t: Duration, p: &mut [Pixel]) -> Result<Rect, String> {
        let ms = t.as_millis() as usize % 3200;
        let progress = if ms < 1000 {
            0
        } else if ms < 1600 {
            (ms - 1000) * 64 / 600
        } else if ms < 2600 {
            64
        } else {
            64 - (ms - 2600) * 64 / 600
        };
        let r = self.f.content;
        p.copy_from_slice(&self.f.base);
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                let a = (x - r.x0) / self.tile;
                let b = (y - r.y0) / self.tile;
                let mut threshold = 0;
                for bit in 0..3 {
                    threshold |= (((a >> bit) ^ (b >> bit)) & 1) << (5 - bit * 2);
                    threshold |= ((b >> bit) & 1) << (4 - bit * 2);
                }
                if threshold < progress {
                    p[y * self.f.width + x] = self.f.list[y * self.f.width + x];
                }
            }
        }
        Ok(r)
    }
    fn storage_bytes(&self) -> usize {
        self.f.storage_bytes()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoints_are_exact() {
        let mut e = new(Preset::Default, 960, 540).unwrap();
        let mut p = vec![Pixel(0); 960 * 540];
        for ms in [0, 999, 3200] {
            e.render(Duration::from_millis(ms), &mut p).unwrap();
            assert_eq!(p, e.f.base);
        }
        for ms in [1600, 2500] {
            e.render(Duration::from_millis(ms), &mut p).unwrap();
            assert_eq!(p, e.f.list);
        }
    }
}
