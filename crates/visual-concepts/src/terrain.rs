// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{Effect, Pixel, Preset, Rect, fixture::put, full, rgb};
use std::time::Duration;
pub struct Terrain {
    w: usize,
    h: usize,
    nx: usize,
    nz: usize,
    heights: Vec<f32>,
    points: Vec<(i32, i32)>,
    background: Vec<Pixel>,
}
pub fn new(preset: Preset, w: usize, h: usize) -> Result<Terrain, String> {
    let nx = preset.choose(48, 24);
    let nz = preset.choose(32, 16);
    let mut heights = Vec::with_capacity(nx * 256);
    for z in 0..256 {
        for x in 0..nx {
            let fx = x as f32 / nx as f32;
            heights.push(
                ((fx * 19.0 + z as f32 * 0.13).sin() + (fx * 41.0 - z as f32 * 0.073).cos()) * 0.3
                    + ((fx - 0.5).abs() * 2.0).powi(2) * 1.7,
            );
        }
    }
    let mut background = vec![Pixel(0); w * h];
    let radius = h as i32 / 9;
    for y in 0..h {
        for x in 0..w {
            let dx = x as i32 - w as i32 / 2;
            let dy = y as i32 - h as i32 / 3;
            if dx * dx + dy * dy < radius * radius && y % 5 != 0 {
                background[y * w + x] = rgb(210, 60 + (y % 90) as u8, 40);
            }
        }
    }
    Ok(Terrain {
        w,
        h,
        nx,
        nz,
        heights,
        points: vec![(0, 0); nx * nz],
        background,
    })
}
fn line(p: &mut [Pixel], w: usize, a: (i32, i32), b: (i32, i32), c: Pixel) {
    let (mut x, mut y) = a;
    let dx = (b.0 - x).abs();
    let sx = if x < b.0 { 1 } else { -1 };
    let dy = -(b.1 - y).abs();
    let sy = if y < b.1 { 1 } else { -1 };
    let mut error = dx + dy;
    for _ in 0..(dx - dy + 1).min(8192) {
        put(p, w, x, y, c);
        if (x, y) == b {
            break;
        }
        let e = error * 2;
        if e >= dy {
            error += dy;
            x += sx;
        }
        if e <= dx {
            error += dx;
            y += sy;
        }
    }
}
impl Effect for Terrain {
    fn render(&mut self, t: Duration, p: &mut [Pixel]) -> Result<Rect, String> {
        p.copy_from_slice(&self.background);
        let travel = (t.as_millis() % 128000) as f32 / 500.0;
        let origin = travel.floor() as usize;
        let fraction = travel.fract();
        for z in 0..self.nz {
            let depth = 2.0 + z as f32 * 0.55;
            for x in 0..self.nx {
                let a = self.heights[((origin + z) % 256) * self.nx + x];
                let b = self.heights[((origin + z + 1) % 256) * self.nx + x];
                let height = a + (b - a) * fraction;
                let px = self.w as f32 / 2.0
                    + (x as f32 / (self.nx - 1) as f32 - 0.5) * self.w as f32 * 3.0 / depth;
                let py = self.h as f32 * 0.47 + (2.3 - height) * self.h as f32 * 0.6 / depth;
                self.points[z * self.nx + x] = (px as i32, py as i32);
            }
        }
        for z in (0..self.nz).rev() {
            let intensity = (210 - 150 * z / self.nz) as u8;
            let c = rgb(20, intensity, intensity);
            for x in 0..self.nx {
                let a = self.points[z * self.nx + x];
                if x + 1 < self.nx {
                    line(p, self.w, a, self.points[z * self.nx + x + 1], c);
                }
                if z + 1 < self.nz {
                    line(p, self.w, a, self.points[(z + 1) * self.nx + x], c);
                }
            }
        }
        Ok(full(self.w, self.h))
    }
    fn storage_bytes(&self) -> usize {
        self.background.capacity() * 2 + self.heights.capacity() * 4 + self.points.capacity() * 8
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scrolling_is_periodic_and_bounded() {
        let mut e = new(Preset::Reduced, 960, 600).unwrap();
        let mut p = vec![Pixel(0); 960 * 600];
        e.render(Duration::ZERO, &mut p).unwrap();
        let first = p.clone();
        e.render(Duration::from_millis(999), &mut p).unwrap();
        assert_ne!(p, first);
        e.render(Duration::from_millis(128000), &mut p).unwrap();
        assert_eq!(p, first);
    }
}
