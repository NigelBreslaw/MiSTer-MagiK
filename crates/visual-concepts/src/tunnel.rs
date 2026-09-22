// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{Effect, Pixel, Preset, Rect, aurora::expand, full, rgb};
use std::time::Duration;
#[path = "tunnel_mesh.rs"]
mod mesh;

pub struct Tunnel {
    w: usize,
    h: usize,
    lw: usize,
    lh: usize,
    // Perspective-correct texture coordinates from actual curved tube geometry.
    views: Vec<Vec<mesh::Sample>>,
    texture: Vec<u8>,
    palette: [Pixel; 256],
    low: Vec<Pixel>,
}
pub fn new(preset: Preset, w: usize, h: usize) -> Result<Tunnel, String> {
    let d = preset.choose(2, 8);
    let lw = (w / d).max(1);
    let lh = (h / d).max(1);
    let views = mesh::prepare(lw, lh, preset.choose(64, 32));
    // Thirty-two wall panels with black checker gaps. A few cream bevels break
    // up the cyan/magenta panels without flattening them into a spoke grid.
    let texture = (0..65536)
        .map(|i| {
            let u = i % 256;
            let v = i / 256;
            if (u / 8 + v / 32) % 2 == 0 {
                0
            } else if v % 32 == 0 && u / 8 % 4 == 0 {
                3
            } else if u / 8 % 4 < 2 {
                1
            } else {
                2
            }
        })
        .collect();
    let palette = std::array::from_fn(|i| {
        let shade = (i / 4) as u16;
        let (r, g, b) = match i % 4 {
            0 => (2, 5, 10),
            1 => (25, 235, 245),
            2 => (215, 55, 235),
            _ => (250, 225, 160),
        };
        rgb(
            (r * shade / 63) as u8,
            (g * shade / 63) as u8,
            (b * shade / 63) as u8,
        )
    });
    Ok(Tunnel {
        w,
        h,
        lw,
        lh,
        views,
        texture,
        palette,
        low: vec![Pixel(0); lw * lh],
    })
}
fn interpolate_wrap(a: u16, b: u16, mix: i32) -> u16 {
    a.wrapping_add(((i32::from(b.wrapping_sub(a) as i16) * mix) >> 8) as u16)
}
impl Effect for Tunnel {
    fn render(&mut self, t: Duration, p: &mut [Pixel]) -> Result<Rect, String> {
        let phase = (t.as_millis() % 8000) as usize * self.views.len() * 256 / 8000;
        let left = &self.views[phase / 256];
        let right = &self.views[(phase / 256 + 1) % self.views.len()];
        let mix = (phase % 256) as i32;
        for ((out, a), b) in self.low.iter_mut().zip(left).zip(right) {
            let u = usize::from(interpolate_wrap(a.u, b.u, mix) >> 8);
            let v = usize::from(interpolate_wrap(a.v, b.v, mix) >> 8);
            let shade = (i32::from(a.shade) * (256 - mix) + i32::from(b.shade) * mix) >> 8;
            let material = self.texture[v * 256 + u];
            *out = self.palette[shade as usize * 4 + usize::from(material)];
        }
        expand(&self.low, self.lw, self.lh, p, self.w, self.h);
        Ok(full(self.w, self.h))
    }
    fn storage_bytes(&self) -> usize {
        self.views
            .iter()
            .map(|view| view.capacity() * std::mem::size_of::<mesh::Sample>())
            .sum::<usize>()
            + self.low.capacity() * 2
            + self.texture.capacity()
            + 512
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn curved_views_move_in_depth_and_loop_exactly() {
        for h in [540, 600] {
            let mut e = new(Preset::Default, 960, h).unwrap();
            let mut p = vec![Pixel(0); 960 * h];
            e.render(Duration::ZERO, &mut p).unwrap();
            let first = p.clone();
            assert!(e.views[0].iter().any(|s| s.shade > 40));
            assert!(e.views[0].iter().any(|s| s.shade < 8));
            assert_ne!(e.views[0], e.views[16]);
            e.render(Duration::from_millis(16), &mut p).unwrap();
            assert_ne!(p, first);
            e.render(Duration::from_millis(8000), &mut p).unwrap();
            assert_eq!(p, first);
            e.render(Duration::from_millis(u64::MAX), &mut p).unwrap();
        }
    }
}
