// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{Effect, Pixel, Preset, Rect, full, rgb};
use std::time::Duration;
pub struct Stars {
    width: usize,
    height: usize,
    stars: Vec<(i32, i32, u32)>,
    dirty: Vec<usize>,
    trail: usize,
}
pub fn new(preset: Preset, width: usize, height: usize) -> Result<Stars, String> {
    let count = preset.choose(256, 128);
    let mut seed = 0x4d414749u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let stars = (0..count)
        .map(|_| {
            (
                (next() % 2048) as i32 - 1024,
                (next() % 1536) as i32 - 768,
                next() % 1024,
            )
        })
        .collect();
    Ok(Stars {
        width,
        height,
        stars,
        dirty: Vec::with_capacity(count + 3 * 24),
        trail: preset.choose(24, 12),
    })
}
impl Effect for Stars {
    fn render(&mut self, t: Duration, p: &mut [Pixel]) -> Result<Rect, String> {
        for i in self.dirty.drain(..) {
            p[i] = Pixel(0);
        }
        let tick = (t.as_millis() / 8) as u32;
        for (i, &(x, y, seed)) in self.stars.iter().enumerate() {
            let z = 128 + (seed.wrapping_sub(tick) % 1024) as i32;
            let px = self.width as i32 / 2 + x * self.width as i32 / (z * 3);
            let py = self.height as i32 / 2 + y * self.height as i32 / (z * 3);
            let brightness = (255 - z / 6).clamp(48, 255) as u8;
            let colour = if i % 3 == 0 {
                rgb(brightness, brightness / 3, brightness / 4)
            } else {
                rgb(brightness / 2, brightness, brightness)
            };
            let count = if i < 3 { self.trail } else { 1 };
            let dx = px - self.width as i32 / 2;
            let dy = py - self.height as i32 / 2;
            let length = ((dx as f64).hypot(dy as f64).ceil() as i32).max(1);
            for n in 0..count {
                let tx = px - dx * n as i32 / length;
                let ty = py - dy * n as i32 / length;
                if tx >= 0 && ty >= 0 && (tx as usize) < self.width && (ty as usize) < self.height {
                    let offset = ty as usize * self.width + tx as usize;
                    p[offset] = colour;
                    self.dirty.push(offset);
                }
            }
        }
        Ok(full(self.width, self.height))
    }
    fn storage_bytes(&self) -> usize {
        self.stars.capacity() * 12 + self.dirty.capacity() * std::mem::size_of::<usize>()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reset_has_no_trails() {
        let mut e = new(Preset::Default, 960, 540).unwrap();
        let mut p = vec![Pixel(0); 960 * 540];
        e.render(Duration::ZERO, &mut p).unwrap();
        let expected = p.clone();
        e.render(Duration::from_millis(500), &mut p).unwrap();
        e.render(Duration::ZERO, &mut p).unwrap();
        assert_eq!(p, expected);
    }
}
