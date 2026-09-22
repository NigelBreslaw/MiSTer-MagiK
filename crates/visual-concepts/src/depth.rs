// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{Effect, Pixel, Preset, Rect, fixture};
use mister_magik_framebuffer_scenes::launcher_navigation::{
    BrowseDirection, BrowseFrame, BrowsePhase,
};
use std::time::Duration;

pub struct Depth {
    fixture: fixture::Fixture,
    poses: Vec<Vec<Pixel>>,
    phases: usize,
}
pub fn new(preset: Preset, width: usize, height: usize) -> Result<Depth, String> {
    let fixture = fixture::Fixture::new(width, height);
    let mut launcher = fixture::prepare(width, height);
    let phases = preset.choose(25, 13);
    let r = fixture.content;
    let mut poses = Vec::with_capacity(5 * phases);
    // Cache only the bounded carousel. Forward poses are played backwards for
    // reversals; interpolation gives each display interval a distinct pose.
    for selected in 0..5 {
        for phase in 0..phases {
            let last = phase == phases - 1;
            launcher.render_frame(BrowseFrame {
                selected: if last { selected + 1 } else { selected },
                target: selected + 1,
                phase: if last {
                    BrowsePhase::Settled
                } else {
                    BrowsePhase::Flipping
                },
                direction: Some(BrowseDirection::Right),
                progress_millis: (phase * 650 / (phases - 1)) as u32,
                duration_millis: 650,
                outgoing: None,
            });
            let mut pose = Vec::with_capacity((r.x1 - r.x0) * (r.y1 - r.y0));
            let visible = fixture.rect(396, 120, 824, 495);
            for y in r.y0..r.y1 {
                for x in r.x0..r.x1 {
                    pose.push(
                        if preset == Preset::Reduced && (x < visible.x0 || x >= visible.x1) {
                            Pixel(0)
                        } else {
                            launcher.pixels()[y * width + x]
                        },
                    );
                }
            }
            poses.push(pose);
        }
    }
    Ok(Depth {
        fixture,
        poses,
        phases,
    })
}
impl Effect for Depth {
    fn render(&mut self, elapsed: Duration, pixels: &mut [Pixel]) -> Result<Rect, String> {
        let ms = elapsed.as_millis() as usize % 16000;
        let leg = ms / 1600;
        let t = (ms % 1600).min(650);
        let (transition, t) = if leg < 5 {
            (leg, t)
        } else {
            (9 - leg, 650 - t)
        };
        let phase = t * (self.phases - 1) * 256 / 650;
        let a = phase / 256;
        let b = (a + 1).min(self.phases - 1);
        let mix = (phase % 256) as u16;
        let f = &self.fixture;
        pixels.copy_from_slice(&f.base);
        let r = f.content;
        let left = &self.poses[transition * self.phases + a];
        let right = &self.poses[transition * self.phases + b];
        let stride = r.x1 - r.x0;
        for y in r.y0..r.y1 {
            let row = (y - r.y0) * stride;
            let output = &mut pixels[y * f.width + r.x0..y * f.width + r.x1];
            if mix == 0 {
                output.copy_from_slice(&left[row..row + stride]);
            } else {
                for (x, p) in output.iter_mut().enumerate() {
                    *p = fixture::blend(left[row + x], right[row + x], mix);
                }
            }
        }
        let shift = ((ms as f32 / 450.0).sin() * 4.0) as i32;
        let glint = f.rect(575, 220, 638, 222);
        for x in glint.x0..glint.x1 {
            fixture::put(
                pixels,
                f.width,
                x as i32 + shift,
                glint.y0 as i32,
                crate::rgb(230, 110, 100),
            );
        }
        Ok(r)
    }
    fn storage_bytes(&self) -> usize {
        self.fixture.storage_bytes() + self.poses.iter().map(|p| p.capacity() * 2).sum::<usize>()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direction_reversal_keeps_chrome() {
        let mut e = new(Preset::Reduced, 960, 540).unwrap();
        let mut p = vec![Pixel(0); 960 * 540];
        for ms in [0, 800, 7999, 8000, 8500, 15999] {
            e.render(Duration::from_millis(ms), &mut p).unwrap();
            assert_eq!(&p[..960 * 76], &e.fixture.base[..960 * 76]);
        }
        e.render(Duration::ZERO, &mut p).unwrap();
        let initial = p.clone();
        e.render(Duration::from_secs(16), &mut p).unwrap();
        assert_eq!(p, initial);
    }
}
