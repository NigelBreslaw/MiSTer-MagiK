// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::super::{BACKGROUND, CREAM, LauncherTypography};
use crate::Rgb565Pixel;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LauncherConnectionState {
    pub internet: bool,
    pub usb: bool,
}

pub(crate) struct LauncherConnectionIndicator {
    pub(in crate::launcher) state: LauncherConnectionState,
    rect: crate::Rgb565Rect,
    pixels: Vec<Rgb565Pixel>,
}

impl LauncherConnectionIndicator {
    pub(in crate::launcher) fn new(
        scene: super::super::LauncherScene,
        state: LauncherConnectionState,
        typography: LauncherTypography<'_>,
    ) -> Self {
        let layout = super::Layout::for_scene(scene);
        let (sx, sy, clock_left, top, crt) = match layout {
            Some(layout) => (
                layout.sx,
                layout.sy,
                layout.width - layout.margin_x - if layout.crt { 30 * layout.sx } else { 45 },
                layout.margin_y + if layout.crt { 0 } else { 4 },
                layout.crt,
            ),
            None => (1, 1, 875, 26, false),
        };
        let (size, badge, gap) = if crt { (12, 24, 6) } else { (15, 32, 8) };
        let (globe_w, globe_h, badge_w, gap) = (size * sx, size * sy, badge * sx, gap * sx);
        let width = badge_w + gap + globe_w;
        let height = globe_h;
        let rect = crate::Rgb565Rect {
            x0: clock_left.saturating_sub(gap + width),
            y0: top,
            x1: clock_left.saturating_sub(gap),
            y1: top + height,
        };
        let mut pixels = vec![Rgb565Pixel(BACKGROUND); width * height];
        let globe_left = badge_w + gap;
        if state.usb {
            for y in 0..height {
                for x in 0..badge_w {
                    if y == 0 || y == height - 1 || x == 0 || x == badge_w - 1 {
                        pixels[y * width + x] = Rgb565Pixel(CREAM);
                    }
                }
            }
            let fonts = layout.map(|layout| layout.fonts(Some(typography)));
            let font = fonts.as_ref().map_or(typography.metadata, |fonts| {
                fonts.get(super::super::TextRole::Metadata)
            });
            let ink_top = "USB"
                .chars()
                .filter_map(|c| font.glyph(c))
                .map(|g| font.ascent - g.top)
                .min()
                .unwrap_or(0);
            let ink_bottom = "USB"
                .chars()
                .filter_map(|c| font.glyph(c))
                .map(|g| font.ascent - g.top + g.height as i32)
                .max()
                .unwrap_or(0);
            font.draw(
                &mut pixels,
                width,
                height,
                (badge_w.saturating_sub(font.measure("USB")) / 2) as i32,
                (height as i32 - (ink_bottom - ink_top)) / 2 - ink_top,
                "USB",
                CREAM,
            );
        }
        // A supersampled 32x32 alpha mask keeps the small HDMI globe smooth without
        // SVG decoding, Slint images, or a second composition layer.
        const MASK: &[u8; 1024] = include_bytes!("internet.alpha");
        let colour = if state.internet {
            CREAM
        } else {
            super::super::rgb(87, 97, 94)
        };
        for y in 0..globe_h {
            for x in 0..globe_w {
                let source_x = ((x as f32 + 0.5) * 32.0 / globe_w as f32 - 0.5).clamp(0.0, 31.0);
                let source_y = ((y as f32 + 0.5) * 32.0 / globe_h as f32 - 0.5).clamp(0.0, 31.0);
                let (ix, iy) = (source_x as usize, source_y as usize);
                let (fx, fy) = (source_x - ix as f32, source_y - iy as f32);
                let alpha =
                    |dx: usize, dy: usize| MASK[(iy + dy).min(31) * 32 + (ix + dx).min(31)] as f32;
                let coverage = ((alpha(0, 0) * (1.0 - fx) + alpha(1, 0) * fx) * (1.0 - fy)
                    + (alpha(0, 1) * (1.0 - fx) + alpha(1, 1) * fx) * fy)
                    as u16;
                {
                    let red = ((colour >> 11) & 31) * coverage / 255;
                    let green = ((colour >> 5) & 63) * coverage / 255;
                    let blue = (colour & 31) * coverage / 255;
                    pixels[y * width + globe_left + x] =
                        Rgb565Pixel((red << 11) | (green << 5) | blue);
                }
            }
        }
        Self {
            state,
            rect,
            pixels,
        }
    }

    pub(in crate::launcher) fn paint(&self, output: &mut [Rgb565Pixel], stride: usize) {
        let width = self.rect.width();
        for (row, source) in self.pixels.chunks_exact(width).enumerate() {
            let at = (self.rect.y0 + row) * stride + self.rect.x0;
            output[at..at + width].copy_from_slice(source);
        }
    }
}
