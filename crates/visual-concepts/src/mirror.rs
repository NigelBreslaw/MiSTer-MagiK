// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    Effect, Pixel, Preset, Rect,
    fixture::{Fixture, blend},
};
use std::time::Duration;
pub struct Mirror {
    f: Fixture,
    reflection: Vec<Pixel>,
    rows: usize,
    amplitude: i32,
    sine: [i16; 256],
}
pub fn new(preset: Preset, w: usize, h: usize) -> Result<Mirror, String> {
    let f = Fixture::new(w, h);
    let r = f.floor;
    let rows = preset.choose(64, 32).min(r.height());
    let mut reflection = vec![Pixel(0); r.width() * rows];
    for y in 0..rows {
        for x in 0..r.width() {
            reflection[y * r.width() + x] = blend(
                f.base[(r.y0 - 1 - y) * w + r.x0 + x],
                Pixel(0),
                (150 + 106 * y / rows) as u16,
            );
        }
    }
    Ok(Mirror {
        f,
        reflection,
        rows,
        amplitude: preset.choose(3, 1) as i32,
        sine: std::array::from_fn(|i| {
            ((i as f32 * std::f32::consts::TAU / 256.0).sin() * 256.0) as i16
        }),
    })
}
impl Effect for Mirror {
    fn render(&mut self, t: Duration, p: &mut [Pixel]) -> Result<Rect, String> {
        p.copy_from_slice(&self.f.base);
        let r = self.f.floor;
        for y in r.y0..r.y1 {
            p[y * self.f.width + r.x0..y * self.f.width + r.x1].fill(Pixel(0));
        }
        for y in 0..self.rows {
            let shift = i32::from(self.sine[(y * 9 + t.as_millis() as usize / 16) % 256])
                * self.amplitude
                / 256;
            for x in 0..r.width() {
                let sx = x as i32 + shift;
                if sx >= 0 && (sx as usize) < r.width() {
                    p[(y + r.y0) * self.f.width + r.x0 + x] =
                        self.reflection[y * r.width() + sx as usize];
                }
            }
        }
        Ok(r)
    }
    fn storage_bytes(&self) -> usize {
        self.f.storage_bytes() + self.reflection.capacity() * 2 + 512
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mirror_does_not_touch_cards() {
        let mut e = new(Preset::Default, 960, 600).unwrap();
        let mut p = vec![Pixel(0); 960 * 600];
        let r = e.render(Duration::from_millis(250), &mut p).unwrap();
        assert_eq!(&p[..r.y0 * 960], &e.f.base[..r.y0 * 960]);
    }
}
