// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{Effect, Pixel, Preset, Rect, fixture};
use mister_magik_framebuffer_scenes::{
    launcher::PreparedLauncher,
    launcher_navigation::{BrowseDirection, BrowseFrame, BrowsePhase},
};
use std::time::Duration;
pub struct Depth {
    launcher: PreparedLauncher,
    fixture: fixture::Fixture,
    reduced: bool,
}
pub fn new(preset: Preset, width: usize, height: usize) -> Result<Depth, String> {
    Ok(Depth {
        launcher: fixture::prepare(width, height),
        fixture: fixture::Fixture::new(width, height),
        reduced: preset == Preset::Reduced,
    })
}
impl Effect for Depth {
    fn render(&mut self, elapsed: Duration, pixels: &mut [Pixel]) -> Result<Rect, String> {
        let ms = elapsed.as_millis() as u64;
        let leg = (ms / 1600) % 10;
        let forward = leg < 5;
        let selected = if forward {
            leg as usize
        } else {
            10 - leg as usize
        };
        let target = if forward { selected + 1 } else { selected - 1 };
        let t = (ms % 1600).min(650) as u32;
        self.launcher.render_into(
            BrowseFrame {
                selected,
                target,
                phase: if t < 650 {
                    BrowsePhase::Flipping
                } else {
                    BrowsePhase::Settled
                },
                direction: Some(if forward {
                    BrowseDirection::Right
                } else {
                    BrowseDirection::Left
                }),
                progress_millis: t,
                duration_millis: 650,
                outgoing: None,
            },
            pixels,
        );
        let f = &self.fixture;
        // A separate foreground glint shifts over the flattened artwork.
        let shift = ((ms as f32 / 450.0).sin() * 4.0) as i32;
        let r = f.rect(575, 220, 638, 222);
        for x in r.x0..r.x1 {
            fixture::put(
                pixels,
                f.width,
                x as i32 + shift,
                r.y0 as i32,
                crate::rgb(230, 110, 100),
            );
        }
        if self.reduced {
            for r in [f.rect(296, 120, 396, 495), f.rect(824, 120, 934, 495)] {
                for y in r.y0..r.y1 {
                    pixels[y * f.width + r.x0..y * f.width + r.x1]
                        .copy_from_slice(&f.base[y * f.width + r.x0..y * f.width + r.x1]);
                }
            }
        }
        Ok(self.launcher.damage()[0])
    }
    fn storage_bytes(&self) -> usize {
        self.fixture.storage_bytes() + self.launcher.cached_raster_bytes()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direction_reversal_keeps_chrome() {
        let mut e = new(Preset::Default, 960, 540).unwrap();
        let mut p = vec![Pixel(0); 960 * 540];
        for ms in [0, 800, 7999, 8000, 8500, 15999] {
            e.render(Duration::from_millis(ms), &mut p).unwrap();
            assert_eq!(&p[..960 * 76], &e.fixture.base[..960 * 76]);
        }
    }
}
