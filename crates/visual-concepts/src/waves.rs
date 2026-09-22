// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{Effect, Pixel, Preset, Rect, fixture::Fixture};
use std::time::Duration;
pub struct Waves {
    f: Fixture,
    amplitude: i32,
    sine: [i16; 256],
}
pub fn new(preset: Preset, w: usize, h: usize) -> Result<Waves, String> {
    Ok(Waves {
        f: Fixture::new(w, h),
        amplitude: preset.choose(8, 4) as i32,
        sine: std::array::from_fn(|i| {
            ((i as f32 * std::f32::consts::TAU / 256.0).sin() * 256.0) as i16
        }),
    })
}
impl Effect for Waves {
    fn render(&mut self, t: Duration, p: &mut [Pixel]) -> Result<Rect, String> {
        p.copy_from_slice(&self.f.base);
        for r in [self.f.rect(521, 162, 696, 331), self.f.floor] {
            for y in r.y0..r.y1 {
                let shift = i32::from(self.sine[(y * 5 + t.as_millis() as usize / 8) % 256])
                    * self.amplitude
                    / 256;
                for x in r.x0..r.x1 {
                    let sx = x as i32 + shift;
                    p[y * self.f.width + x] = if sx >= r.x0 as i32 && sx < r.x1 as i32 {
                        self.f.base[y * self.f.width + sx as usize]
                    } else {
                        Pixel(0)
                    };
                }
            }
        }
        Ok(self.f.content)
    }
    fn storage_bytes(&self) -> usize {
        self.f.storage_bytes() + 512
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waves_preserve_title_and_labels() {
        let mut e = new(Preset::Default, 960, 540).unwrap();
        let mut p = vec![Pixel(0); 960 * 540];
        e.render(Duration::from_millis(700), &mut p).unwrap();
        let title = e.f.rect(520, 338, 700, 407);
        for y in title.y0..title.y1 {
            assert_eq!(
                &p[y * 960 + title.x0..y * 960 + title.x1],
                &e.f.base[y * 960 + title.x0..y * 960 + title.x1]
            );
        }
    }
}
