// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{Effect, Pixel, Preset, Rect, full};
use std::time::Duration;

const PERIOD_NS: u128 = 8_192_000_000;
#[derive(Clone, Copy)]
struct Star {
    x: f32,
    y: f32,
    depth: f32,
}
pub struct Stars {
    width: usize,
    height: usize,
    stars: Vec<Star>,
    dirty: Vec<usize>,
    // Fractional channel sums keep overlapping dim stars until final RGB565
    // rounding, rather than letting the last star overwrite earlier ones.
    sums: Vec<[u16; 3]>,
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
        .map(|_| Star {
            x: (next() % 2048) as f32 - 1024.0,
            y: (next() % 1536) as f32 - 768.0,
            depth: (next() % 1024) as f32,
        })
        .collect();
    Ok(Stars {
        width,
        height,
        stars,
        dirty: Vec::with_capacity(count * 16),
        sums: vec![[0; 3]; width * height],
    })
}
fn project(star: Star, phase: f32, width: usize, height: usize) -> (f32, f32, f32) {
    let z = 128.0 + (star.depth - phase).rem_euclid(1024.0);
    let px = width as f32 * 0.5 + star.x * width as f32 / (z * 3.0);
    let py = height as f32 * 0.5 + star.y * height as f32 / (z * 3.0);
    let smooth = |t: f32| {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let fade = smooth((z - 128.0) / 64.0) * smooth((1152.0 - z) / 128.0);
    // A compact filtered star spreads energy over adjacent pixels. Two units
    // of total energy keep its central pixels legible without adding a trail.
    let energy = (255.0 - z / 6.0).clamp(48.0, 255.0) * fade * 2.0;
    (px, py, energy)
}
// Nonnegative cubic B-spline (Mitchell B=1, C=0), four-pixel support.
// Weights sum to one and retain the fractional centroid through a pixel crossing.
fn weights(f: f32) -> [f32; 4] {
    let f2 = f * f;
    let f3 = f2 * f;
    [
        (1.0 - f).powi(3) / 6.0,
        (3.0 * f3 - 6.0 * f2 + 4.0) / 6.0,
        (-3.0 * f3 + 3.0 * f2 + 3.0 * f + 1.0) / 6.0,
        f3 / 6.0,
    ]
}
impl Stars {
    fn splat(&mut self, px: f32, py: f32, colour: [f32; 3]) {
        if px < -2.0 || py < -2.0 || px > self.width as f32 + 1.0 || py > self.height as f32 + 1.0 {
            return;
        }
        let x0 = px.floor() as i32;
        let y0 = py.floor() as i32;
        let wx = weights(px - px.floor());
        let wy = weights(py - py.floor());
        for (dy, &fy) in wy.iter().enumerate() {
            for (dx, &fx) in wx.iter().enumerate() {
                let x = x0 + dx as i32 - 1;
                let y = y0 + dy as i32 - 1;
                if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
                    continue;
                }
                let index = y as usize * self.width + x as usize;
                let weight = fx * fy * 256.0;
                for (sum, &channel) in self.sums[index].iter_mut().zip(&colour) {
                    *sum = sum.saturating_add((channel * weight).round() as u16);
                }
                self.dirty.push(index);
            }
        }
    }
}
fn resolve(channels: [u16; 3]) -> Pixel {
    let [r, g, b] = channels.map(u32::from);
    Pixel(
        ((((r + 1024) >> 11).min(31) << 11)
            | (((g + 512) >> 10).min(63) << 5)
            | ((b + 1024) >> 11).min(31)) as u16,
    )
}
impl Effect for Stars {
    fn render(&mut self, t: Duration, p: &mut [Pixel]) -> Result<Rect, String> {
        for i in self.dirty.drain(..) {
            p[i] = Pixel(0);
            self.sums[i] = [0; 3];
        }
        // Keep all fractional depth progress from the nominal display clock.
        let phase = (t.as_nanos() % PERIOD_NS) as f32 / 8_000_000.0;
        for i in 0..self.stars.len() {
            let (px, py, energy) = project(self.stars[i], phase, self.width, self.height);
            let colour = if i % 3 == 0 {
                [energy, energy / 3.0, energy / 4.0]
            } else {
                [energy / 2.0, energy, energy]
            };
            self.splat(px, py, colour);
        }
        for &i in &self.dirty {
            p[i] = resolve(self.sums[i]);
        }
        Ok(full(self.width, self.height))
    }
    fn storage_bytes(&self) -> usize {
        self.stars.capacity() * std::mem::size_of::<Star>()
            + self.dirty.capacity() * std::mem::size_of::<usize>()
            + self.sums.capacity() * std::mem::size_of::<[u16; 3]>()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reset_loop_and_clipping_leave_no_stale_stars() {
        for h in [540, 600] {
            let mut e = new(Preset::Default, 960, h).unwrap();
            let mut p = vec![Pixel(0); 960 * h];
            e.render(Duration::ZERO, &mut p).unwrap();
            let expected = p.clone();
            e.render(Duration::from_millis(500), &mut p).unwrap();
            assert_ne!(p, expected);
            e.render(Duration::from_nanos(PERIOD_NS as u64), &mut p)
                .unwrap();
            assert_eq!(p, expected);
            e.render(Duration::from_nanos(u64::MAX), &mut p).unwrap();
            e.render(Duration::ZERO, &mut p).unwrap();
            assert_eq!(p, expected);
        }
    }
    #[test]
    fn subpixel_filter_conserves_energy_and_tracks_centroid_without_stair_steps() {
        let mut e = new(Preset::Reduced, 32, 24).unwrap();
        for step in 0..=64 {
            let x = 12.0 + step as f32 / 32.0;
            let y = 10.0 + step as f32 / 64.0;
            e.sums.fill([0; 3]);
            e.dirty.clear();
            e.splat(x, y, [200.0; 3]);
            let mut total = 0.0;
            let mut mx = 0.0;
            let mut my = 0.0;
            for (i, c) in e.sums.iter().enumerate() {
                let energy = f32::from(c[0]);
                total += energy;
                mx += (i % 32) as f32 * energy;
                my += (i / 32) as f32 * energy;
            }
            assert!((total - 200.0 * 256.0).abs() < 8.0);
            assert!((mx / total - x).abs() < 0.001);
            assert!((my / total - y).abs() < 0.001);
        }
    }
    #[test]
    fn rgb565_translation_has_bounded_brightness_and_centroid_error() {
        let mut e = new(Preset::Reduced, 32, 24).unwrap();
        let mut minimum = u32::MAX;
        let mut maximum = 0;
        let mut previous = (0.0, 0.0);
        for step in 0..=64 {
            let x = 12.0 + step as f32 / 32.0;
            let y = 10.0 + step as f32 / 64.0;
            e.sums.fill([0; 3]);
            e.dirty.clear();
            e.splat(x, y, [200.0; 3]);
            let mut total = 0;
            let mut mx = 0;
            let mut my = 0;
            for (i, &c) in e.sums.iter().enumerate() {
                let energy = u32::from((resolve(c).0 >> 5) & 63);
                total += energy;
                mx += (i % 32) as u32 * energy;
                my += (i / 32) as u32 * energy;
            }
            minimum = minimum.min(total);
            maximum = maximum.max(total);
            let centroid = (mx as f32 / total as f32, my as f32 / total as f32);
            assert!((centroid.0 - x).abs() < 0.08);
            assert!((centroid.1 - y).abs() < 0.08);
            assert!(centroid.0 >= previous.0 - 0.04);
            assert!(centroid.1 >= previous.1 - 0.04);
            previous = centroid;
        }
        assert!(
            maximum - minimum <= 4,
            "RGB565 energy range: {minimum}..{maximum}"
        );
    }
    #[test]
    fn projection_has_fractional_progress_and_invisible_recycling() {
        let s = Star {
            x: 100.0,
            y: 80.0,
            depth: 512.0,
        };
        let a = project(s, 100.0, 960, 540);
        let b = project(s, 100.25, 960, 540);
        assert!(b.0 > a.0 && b.1 > a.1);
        assert!(b.0 - a.0 < 1.0);
        assert_eq!(project(s, 512.0, 960, 540).2, 0.0);
        assert!(project(s, 511.99, 960, 540).2 < 0.001);
        assert!(project(s, 512.01, 960, 540).2 < 0.001);
    }
    #[test]
    fn overlapping_stars_add_instead_of_overwriting() {
        let mut e = new(Preset::Reduced, 32, 24).unwrap();
        e.splat(12.25, 10.5, [50.0; 3]);
        let first = e.sums.clone();
        e.splat(12.25, 10.5, [50.0; 3]);
        for (a, b) in first.iter().zip(&e.sums) {
            assert_eq!(b[0], a[0] * 2);
        }
    }
}
