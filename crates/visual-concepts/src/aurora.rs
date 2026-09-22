// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{Effect, Pixel, Preset, Rect, full, rgb};
use std::time::Duration;
pub struct Aurora {
    width: usize,
    height: usize,
    lw: usize,
    lh: usize,
    low: Vec<Pixel>,
    sine: [i16; 1024],
    palette: [Pixel; 256],
}
pub fn new(preset: Preset, width: usize, height: usize) -> Result<Aurora, String> {
    let divisor = preset.choose(4, 8);
    let lw = (width / divisor).max(1);
    let lh = (height / divisor).max(1);
    Ok(Aurora {
        width,
        height,
        lw,
        lh,
        low: vec![Pixel(0); lw * lh],
        sine: std::array::from_fn(|i| {
            ((i as f32 * std::f32::consts::TAU / 1024.0).sin() * 127.0) as i16
        }),
        palette: std::array::from_fn(|i| {
            let t = i as u8;
            rgb(
                t.saturating_sub(90).saturating_mul(2),
                t / 2,
                t.saturating_add(20),
            )
        }),
    })
}
pub fn expand(low: &[Pixel], lw: usize, lh: usize, p: &mut [Pixel], w: usize, h: usize) {
    for sy in 0..lh {
        let y0 = sy * h / lh;
        let y1 = (sy + 1) * h / lh;
        let row = &mut p[y0 * w..(y0 + 1) * w];
        for sx in 0..lw {
            row[sx * w / lw..(sx + 1) * w / lw].fill(low[sy * lw + sx]);
        }
        for y in y0 + 1..y1 {
            p.copy_within(y0 * w..(y0 + 1) * w, y * w);
        }
    }
}
impl Effect for Aurora {
    fn render(&mut self, t: Duration, p: &mut [Pixel]) -> Result<Rect, String> {
        let phase = (t.as_millis() / 12) as usize;
        for y in 0..self.lh {
            for x in 0..self.lw {
                let wave = (i32::from(self.sine[(x * 5 + phase) % 1024])
                    + i32::from(self.sine[(x * 3 + phase * 2) % 1024]))
                    / 4;
                let center = self.lh as i32 / 2 + wave * self.lh as i32 / 256;
                let distance = (y as i32 - center).unsigned_abs() as usize;
                let intensity = 255usize.saturating_sub(distance * 180 / self.lh.max(1) * 8);
                let dither = [0, 2, 3, 1][(y % 2) * 2 + x % 2];
                self.low[y * self.lw + x] = if intensity < 8 {
                    Pixel(0)
                } else {
                    self.palette[(intensity + dither).min(255)]
                };
            }
        }
        expand(&self.low, self.lw, self.lh, p, self.width, self.height);
        Ok(full(self.width, self.height))
    }
    fn storage_bytes(&self) -> usize {
        self.low.capacity() * 2 + 2560
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scale_covers_uneven_rows() {
        let mut p = vec![Pixel(99); 7 * 5];
        expand(
            &[Pixel(1), Pixel(2), Pixel(3), Pixel(4)],
            2,
            2,
            &mut p,
            7,
            5,
        );
        assert!(!p.contains(&Pixel(99)));
        assert_eq!(p[34], Pixel(4));
    }
}
