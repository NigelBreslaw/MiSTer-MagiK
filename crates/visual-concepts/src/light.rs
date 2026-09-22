// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! A translated soft sheen on the card face. Research and deliberately chosen
//! visual parameters are recorded in docs/light-sweep-research.md.
use crate::{Effect, Pixel, Preset, Rect, fixture::Fixture};
use std::time::Duration;

// Fixed spatial thresholds distribute RGB565 rounding without temporal noise.
const DITHER: [u32; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
const PERIOD_NS: u128 = 3_000_000_000;
const PEAK: u16 = 256; // 6.25% white contribution, in Q12.

pub struct Light {
    f: Fixture,
    profile: Vec<u16>,
    coverage: Vec<u16>,
    gradient: Vec<u16>,
    units: usize,
    radius: usize,
}
pub fn new(preset: Preset, width: usize, height: usize) -> Result<Light, String> {
    let f = Fixture::new(width, height);
    let r = f.card;
    let units = preset.choose(4, 2);
    let radius = (r.width() * units * 28 / 100).max(1);
    let profile = (0..=radius * 2)
        .map(|i| {
            let q = 1.0 - (i as f32 - radius as f32).abs() / radius as f32;
            (q * q * (3.0 - 2.0 * q) * f32::from(PEAK)).round() as u16
        })
        .collect();
    let face = f.rect(527, 166, 693, 334);
    let feather = (r.width() * 6 / 100).max(1);
    let mut coverage = vec![0; r.width() * r.height()];
    for y in face.y0..face.y1 {
        for x in face.x0..face.x1 {
            let edge = (x - face.x0)
                .min(face.x1 - 1 - x)
                .min(y - face.y0)
                .min(face.y1 - 1 - y);
            let q = (edge as f32 / feather as f32).min(1.0);
            coverage[(y - r.y0) * r.width() + x - r.x0] =
                (q * q * (3.0 - 2.0 * q) * 256.0).round() as u16;
        }
    }
    let gradient = vec![0; r.width() * units + r.height() * units / 4 + 1];
    Ok(Light {
        f,
        profile,
        coverage,
        gradient,
        units,
        radius,
    })
}
impl Light {
    fn prepare_gradient(&mut self, t: Duration) {
        // Move ONE profile at constant subpixel speed. Crossfading spaced
        // snapshots changes the profile's width and creates double highlights.
        let travel = self.gradient.len() + self.radius * 2;
        let centre = ((t.as_nanos() % PERIOD_NS) * travel as u128 * 256 / PERIOD_NS) as i64
            - self.radius as i64 * 256;
        for (i, alpha) in self.gradient.iter_mut().enumerate() {
            let position = i as i64 * 256 - centre + self.radius as i64 * 256;
            *alpha = if position >= 0 && position < self.radius as i64 * 512 {
                let sample = position as usize / 256;
                let mix = position as u32 & 255;
                ((u32::from(self.profile[sample]) * (256 - mix)
                    + u32::from(self.profile[sample + 1]) * mix)
                    >> 8) as u16
            } else {
                0
            };
        }
    }
}
fn sheen(pixel: Pixel, alpha: u16, threshold: u32) -> Pixel {
    let alpha = u32::from(alpha);
    let p = u32::from(pixel.0);
    let lift = |v, max| v + (((max - v) * alpha + threshold) >> 12);
    Pixel(((lift(p >> 11, 31) << 11) | (lift((p >> 5) & 63, 63) << 5) | lift(p & 31, 31)) as u16)
}
impl Effect for Light {
    fn render(&mut self, t: Duration, p: &mut [Pixel]) -> Result<Rect, String> {
        p.copy_from_slice(&self.f.base);
        self.prepare_gradient(t);
        let r = self.f.card;
        for y in 0..r.height() {
            for x in 0..r.width() {
                let alpha = ((u32::from(self.gradient[x * self.units + y * self.units / 4])
                    * u32::from(self.coverage[y * r.width() + x]))
                    >> 8) as u16;
                if alpha != 0 {
                    let i = (y + r.y0) * self.f.width + x + r.x0;
                    let threshold = DITHER[((y + r.y0) & 3) * 4 + ((x + r.x0) & 3)] * 256 + 128;
                    p[i] = sheen(self.f.base[i], alpha, threshold);
                }
            }
        }
        Ok(r)
    }
    fn storage_bytes(&self) -> usize {
        self.f.storage_bytes()
            + (self.profile.capacity() + self.coverage.capacity() + self.gradient.capacity()) * 2
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sheen_preserves_chrome_text_and_loop_endpoints() {
        for h in [540, 600] {
            for preset in [Preset::Default, Preset::Reduced] {
                let mut e = new(preset, 960, h).unwrap();
                let mut p = vec![Pixel(0); 960 * h];
                e.render(Duration::ZERO, &mut p).unwrap();
                assert_eq!(p, e.f.base);
                e.render(Duration::from_millis(1500), &mut p).unwrap();
                let first = p.clone();
                assert_ne!(p, e.f.base);
                let face = e.f.rect(527, 166, 693, 334);
                for y in 0..h {
                    for x in 0..960 {
                        if x < face.x0 || x >= face.x1 || y < face.y0 || y >= face.y1 {
                            assert_eq!(p[y * 960 + x], e.f.base[y * 960 + x]);
                        }
                    }
                }
                e.render(Duration::from_nanos(1_516_666_667), &mut p)
                    .unwrap();
                assert_ne!(p, first, "active sheen must advance each nominal frame");
                e.render(Duration::from_secs(3), &mut p).unwrap();
                assert_eq!(p, e.f.base);
            }
        }
    }
    #[test]
    fn translated_profile_keeps_its_shape_between_old_phase_boundaries() {
        let mut e = new(Preset::Default, 960, 540).unwrap();
        let mut sums = Vec::new();
        for frame in 60..120 {
            e.prepare_gradient(Duration::from_nanos(frame * 16_666_667));
            let peak = *e.gradient.iter().max().unwrap();
            assert!(peak >= PEAK - 1);
            sums.push(e.gradient.iter().map(|&a| u32::from(a)).sum::<u32>());
        }
        assert!(sums.iter().max().unwrap() - sums.iter().min().unwrap() < 512);
    }
    #[test]
    fn rgb565_sheen_is_bounded_and_never_darkens_original_art() {
        for p in 0..=u16::MAX {
            let out = sheen(Pixel(p), PEAK, 3968).0;
            for (shift, mask, limit) in [(11, 31, 2), (5, 63, 4), (0, 31, 2)] {
                let before = (p >> shift) & mask;
                let after = (out >> shift) & mask;
                assert!((before..=before + limit).contains(&after));
            }
            assert_eq!(sheen(Pixel(p), 0, 3968), Pixel(p));
        }
    }
}
