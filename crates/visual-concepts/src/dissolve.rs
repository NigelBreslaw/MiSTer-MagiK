// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{Effect, Pixel, Preset, Rect, fixture::Fixture};
use std::time::Duration;
pub struct Dissolve {
    f: Fixture,
    list: Vec<Pixel>,
    tiles: Vec<(Rect, usize)>,
}
pub fn new(preset: Preset, w: usize, h: usize) -> Result<Dissolve, String> {
    let f = Fixture::new(w, h);
    let tile = preset.choose(8, 16);
    let r = f.content;
    let mut tiles = Vec::new();
    for y in (r.y0..r.y1).step_by(tile) {
        for x in (r.x0..r.x1).step_by(tile) {
            let a = (x - r.x0) / tile;
            let b = (y - r.y0) / tile;
            let mut threshold = 0;
            for bit in 0..3 {
                threshold |= (((a >> bit) ^ (b >> bit)) & 1) << (5 - bit * 2);
                threshold |= ((b >> bit) & 1) << (4 - bit * 2);
            }
            tiles.push((
                Rect {
                    x0: x,
                    y0: y,
                    x1: (x + tile).min(r.x1),
                    y1: (y + tile).min(r.y1),
                },
                threshold,
            ));
        }
    }
    Ok(Dissolve {
        list: f.list(),
        f,
        tiles,
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
        if progress == 64 {
            p.copy_from_slice(&self.list);
        } else {
            p.copy_from_slice(&self.f.base);
            if progress > 0 {
                for &(tile, threshold) in &self.tiles {
                    if threshold < progress {
                        for y in tile.y0..tile.y1 {
                            let row = y * self.f.width + tile.x0..y * self.f.width + tile.x1;
                            p[row.clone()].copy_from_slice(&self.list[row]);
                        }
                    }
                }
            }
        }
        Ok(r)
    }
    fn storage_bytes(&self) -> usize {
        self.f.storage_bytes()
            + self.list.capacity() * 2
            + self.tiles.capacity() * std::mem::size_of::<(Rect, usize)>()
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
            assert_eq!(p, e.list);
        }
    }
}
