// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{Effect, Pixel, Preset, Rect, full, rgb, scale::expand};
use std::time::Duration;
#[path = "tunnel_mesh.rs"]
mod mesh;

pub struct Tunnel {
    w: usize,
    h: usize,
    lw: usize,
    lh: usize,
    // One geometry-correct palette-index image per nominal display frame.
    frames: Vec<u8>,
    texture: Vec<u8>,
    palette: [Pixel; 256],
    low: Vec<Pixel>,
}
pub fn new(preset: Preset, w: usize, h: usize) -> Result<Tunnel, String> {
    let d = preset.choose(2, 8);
    let lw = (w / d).max(1);
    let lh = (h / d).max(1);
    // Thirty-two wall panels with black checker gaps. A few cream bevels break
    // up the cyan/magenta panels without flattening them into a spoke grid.
    let texture: Vec<u8> = (0..65536)
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
    let mut frames = Vec::with_capacity(lw * lh * 480);
    // Keep only one temporary coordinate/depth map alive while preparing. The
    // complete default 480x270 sequence costs 62,208,000 bytes, not 480 UV maps.
    for frame in 0..480 {
        let view = mesh::view(lw, lh, frame as f32 * std::f32::consts::TAU / 480.0);
        frames.extend(
            view.iter().map(|s| {
                s.shade * 4 + texture[usize::from(s.v >> 8) * 256 + usize::from(s.u >> 8)]
            }),
        );
    }
    Ok(Tunnel {
        w,
        h,
        lw,
        lh,
        frames,
        texture,
        palette,
        low: vec![Pixel(0); lw * lh],
    })
}
impl Effect for Tunnel {
    fn render(&mut self, t: Duration, p: &mut [Pixel]) -> Result<Rect, String> {
        let frame = (t.as_nanos() * 60 / 1_000_000_000 % 480) as usize;
        let start = frame * self.low.len();
        for (out, &index) in self.low.iter_mut().zip(&self.frames[start..]) {
            *out = self.palette[usize::from(index)];
        }
        expand(&self.low, self.lw, self.lh, p, self.w, self.h);
        Ok(full(self.w, self.h))
    }
    fn storage_bytes(&self) -> usize {
        self.frames.capacity() + self.low.capacity() * 2 + self.texture.capacity() + 512
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tight_bend_frame_matches_direct_geometry() {
        let mut e = new(Preset::Default, 960, 540).unwrap();
        let mut p = vec![Pixel(0); 960 * 540];
        e.render(Duration::from_nanos(71 * 16_666_667), &mut p)
            .unwrap();
        let exact = mesh::view(e.lw, e.lh, 71.0 * std::f32::consts::TAU / 480.0);
        let wrong = e
            .low
            .iter()
            .zip(exact)
            .filter(|(pixel, s)| {
                let material = e.texture[usize::from(s.v >> 8) * 256 + usize::from(s.u >> 8)];
                **pixel != e.palette[usize::from(s.shade) * 4 + usize::from(material)]
            })
            .count();
        assert_eq!(
            wrong, 0,
            "tight-bend pixels disagree with the actual projected surface"
        );
    }
    #[test]
    fn curved_views_move_in_depth_and_loop_exactly() {
        for h in [540, 600] {
            let mut e = new(Preset::Default, 960, h).unwrap();
            let mut p = vec![Pixel(0); 960 * h];
            e.render(Duration::ZERO, &mut p).unwrap();
            let first = p.clone();
            assert!(e.frames[..e.low.len()].iter().any(|s| s / 4 > 40));
            assert!(e.frames[..e.low.len()].iter().any(|s| s / 4 < 8));
            assert_ne!(
                e.frames[..e.low.len()],
                e.frames[120 * e.low.len()..121 * e.low.len()]
            );
            e.render(Duration::from_nanos(16_666_667), &mut p).unwrap();
            assert_ne!(p, first);
            e.render(Duration::from_millis(8000), &mut p).unwrap();
            assert_eq!(p, first);
            e.render(Duration::from_millis(u64::MAX), &mut p).unwrap();
        }
    }
}
