// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{Effect, Pixel, Preset, Rect, aurora::expand, full, rgb};
use std::time::Duration;
pub struct Tunnel {
    w: usize,
    h: usize,
    lw: usize,
    lh: usize,
    coords: Vec<(u8, u8)>,
    texture: Vec<Pixel>,
    low: Vec<Pixel>,
}
pub fn new(preset: Preset, w: usize, h: usize) -> Result<Tunnel, String> {
    let d = preset.choose(4, 8);
    let lw = (w / d).max(1);
    let lh = (h / d).max(1);
    let mut coords = Vec::with_capacity(lw * lh);
    for y in 0..lh {
        for x in 0..lw {
            let dx = x as f32 - lw as f32 / 2.0;
            let dy = y as f32 - lh as f32 / 2.0;
            let radius = (dx * dx + dy * dy).sqrt().max(1.0);
            let angle = ((dy.atan2(dx) / std::f32::consts::TAU + 0.5) * 256.0) as u8;
            let depth = (lw as f32 * 64.0 / radius) as u32 as u8;
            coords.push((angle, depth));
        }
    }
    let texture = (0..65536)
        .map(|i| {
            let x = i % 256;
            let y = i / 256;
            if x % 32 < 2 || y % 32 < 2 {
                rgb(210, 200, 155)
            } else if (x / 32 + y / 32) % 2 == 0 {
                rgb(22, 100, 125)
            } else {
                rgb(105, 30, 110)
            }
        })
        .collect();
    Ok(Tunnel {
        w,
        h,
        lw,
        lh,
        coords,
        texture,
        low: vec![Pixel(0); lw * lh],
    })
}
impl Effect for Tunnel {
    fn render(&mut self, t: Duration, p: &mut [Pixel]) -> Result<Rect, String> {
        let a = (t.as_millis() / 30) as u8;
        let z = (t.as_millis() / 12) as u8;
        for (out, &(u, v)) in self.low.iter_mut().zip(&self.coords) {
            *out =
                self.texture[usize::from(v.wrapping_add(z)) * 256 + usize::from(u.wrapping_add(a))];
        }
        expand(&self.low, self.lw, self.lh, p, self.w, self.h);
        Ok(full(self.w, self.h))
    }
    fn storage_bytes(&self) -> usize {
        self.coords.capacity() * 2 + self.low.capacity() * 2 + self.texture.capacity() * 2
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn center_and_large_time_are_bounded() {
        let mut e = new(Preset::Default, 960, 600).unwrap();
        let mut p = vec![Pixel(0); 960 * 600];
        for ms in [0, 3000, u64::MAX] {
            e.render(Duration::from_millis(ms), &mut p).unwrap();
        }
        assert!(p.iter().any(|p| p.0 != 0));
    }
}
