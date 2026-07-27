// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral composition helpers shared by the device and visual preview.

use crate::arcade_catalog::{ArcadeGameEntry, ArcadeGameView};
use crate::arcade_list_renderer::{ArcadeListGeometry, ArcadeListRenderer, ArcadeListUpdate};
use mister_magik_mister_runtime::framebuffer::target::{DirtyRect, UiFrameTarget};
use slint::platform::software_renderer::Rgb565Pixel;

pub struct ArcadeVisualLayer {
    renderer: ArcadeListRenderer,
}

impl ArcadeVisualLayer {
    pub fn new(frame_width: usize, frame_height: usize) -> Self {
        let mut renderer = ArcadeListRenderer::new();
        let mut geometry = ArcadeListGeometry::NORMAL;
        geometry.width = geometry.width.min(frame_width.saturating_sub(geometry.x));
        renderer.set_geometry_for_render_h(geometry, frame_height);
        Self { renderer }
    }

    pub fn compose(
        &mut self,
        target: &mut UiFrameTarget,
        games: &[ArcadeGameEntry],
        selected: usize,
        force: bool,
    ) -> Option<DirtyRect> {
        let update = self.renderer.draw(
            ArcadeGameView::Contiguous(games),
            selected,
            selected as f32,
            force,
        )?;
        let rect = match update {
            ArcadeListUpdate::Full(rect) | ArcadeListUpdate::Scroll { rect, .. } => rect,
        };
        self.renderer.compose_layer_to_cached(target, true);
        Some(rect)
    }

    pub fn dirty_rect(&self) -> DirtyRect {
        self.renderer.dirty_rect()
    }

    pub fn invalidate(&mut self) {
        self.renderer.invalidate_presented_layer();
    }
}

#[derive(Clone, Copy)]
pub enum PreviewPixels<'a> {
    Empty,
    Rgb565 {
        pixels: &'a [Rgb565Pixel],
        stride_pixels: usize,
    },
    Rgb8(&'a [u8]),
}

#[derive(Clone, Copy)]
pub struct PreviewFrame<'a> {
    pub pixels: PreviewPixels<'a>,
    pub source_width: usize,
    pub source_height: usize,
    pub display_width: usize,
    pub display_height: usize,
}

#[derive(Clone, Copy)]
pub struct PreviewSurface {
    pub x: usize,
    pub y: usize,
    pub stride: usize,
}

impl PreviewSurface {
    pub const fn full(stride: usize) -> Self {
        Self { x: 0, y: 0, stride }
    }

    fn row_start(self, y: usize, x: usize) -> usize {
        (y - self.y) * self.stride + (x - self.x)
    }
}

pub fn hdmi_preview_rect(frame_width: usize, frame_height: usize) -> DirtyRect {
    const CABINET_WIDTH: usize = 336;
    const CABINET_HEIGHT: usize = 520;
    const PREVIEW_X: usize = 8;
    const PREVIEW_Y: usize = 92;
    const PREVIEW_WIDTH: usize = 320;
    const PREVIEW_HEIGHT: usize = 320;

    let right_x = frame_width / 2;
    let right_width = frame_width.saturating_sub(right_x);
    let cabinet_x = right_x + right_width.saturating_sub(CABINET_WIDTH) / 2;
    let cabinet_y = frame_height.saturating_sub(CABINET_HEIGHT) / 2;
    DirtyRect {
        x0: (cabinet_x + PREVIEW_X).min(frame_width),
        y0: (cabinet_y + PREVIEW_Y).min(frame_height),
        x1: (cabinet_x + PREVIEW_X + PREVIEW_WIDTH).min(frame_width),
        y1: (cabinet_y + PREVIEW_Y + PREVIEW_HEIGHT).min(frame_height),
    }
}

pub fn compose_preview_frame(
    destination: &mut [Rgb565Pixel],
    frame_width: usize,
    frame_height: usize,
    screen: DirtyRect,
    frame: PreviewFrame<'_>,
    clear_screen: bool,
    surface: PreviewSurface,
) -> Option<DirtyRect> {
    if frame.source_width == 0
        || frame.source_height == 0
        || frame.display_width == 0
        || frame.display_height == 0
    {
        return matches!(frame.pixels, PreviewPixels::Empty).then_some(screen);
    }

    let image_x = screen.x0 as isize + (screen.width() as isize - frame.display_width as isize) / 2;
    let image_y = screen.y0 as isize + (screen.rows() as isize - frame.display_height as isize) / 2;
    let rect = DirtyRect {
        x0: screen.x0.max(image_x.max(0) as usize),
        y0: screen.y0.max(image_y.max(0) as usize),
        x1: screen
            .x1
            .min((image_x + frame.display_width as isize).max(0) as usize)
            .min(frame_width),
        y1: screen
            .y1
            .min((image_y + frame.display_height as isize).max(0) as usize)
            .min(frame_height),
    };
    if rect.x1 <= rect.x0 || rect.y1 <= rect.y0 {
        return None;
    }

    if clear_screen || matches!(frame.pixels, PreviewPixels::Empty) {
        clear_rect(destination, screen, frame_width, frame_height, surface);
    }
    match frame.pixels {
        PreviewPixels::Empty => {}
        PreviewPixels::Rgb565 {
            pixels,
            stride_pixels,
        } => {
            for y in rect.y0..rect.y1 {
                let source_y = ((y as isize - image_y).max(0) as usize * frame.source_height
                    / frame.display_height)
                    .min(frame.source_height - 1);
                for x in rect.x0..rect.x1 {
                    let source_x = ((x as isize - image_x).max(0) as usize * frame.source_width
                        / frame.display_width)
                        .min(frame.source_width - 1);
                    let source_index = source_y * stride_pixels + source_x;
                    if let Some(pixel) = pixels.get(source_index) {
                        destination[surface.row_start(y, x)] = *pixel;
                    }
                }
            }
        }
        PreviewPixels::Rgb8(pixels) => {
            for y in rect.y0..rect.y1 {
                let source_y = ((y as isize - image_y).max(0) as usize * frame.source_height
                    / frame.display_height)
                    .min(frame.source_height - 1);
                for x in rect.x0..rect.x1 {
                    let source_x = ((x as isize - image_x).max(0) as usize * frame.source_width
                        / frame.display_width)
                        .min(frame.source_width - 1);
                    let source_index = (source_y * frame.source_width + source_x) * 3;
                    if let Some(rgb) = pixels.get(source_index..source_index + 3) {
                        destination[surface.row_start(y, x)] =
                            rgb888_to_rgb565(rgb[0], rgb[1], rgb[2]);
                    }
                }
            }
        }
    }
    Some(if clear_screen { screen } else { rect })
}

fn clear_rect(
    destination: &mut [Rgb565Pixel],
    rect: DirtyRect,
    frame_width: usize,
    frame_height: usize,
    surface: PreviewSurface,
) {
    for y in rect.y0..rect.y1.min(frame_height) {
        for x in rect.x0..rect.x1.min(frame_width) {
            destination[surface.row_start(y, x)] = Rgb565Pixel(0);
        }
    }
}

fn rgb888_to_rgb565(red: u8, green: u8, blue: u8) -> Rgb565Pixel {
    Rgb565Pixel(
        ((u16::from(red) & 0xf8) << 8) | ((u16::from(green) & 0xfc) << 3) | (u16::from(blue) >> 3),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_frame_scales_rgb565_to_requested_rect() {
        let source = [
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x07e0),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0xffff),
        ];
        let mut destination = vec![Rgb565Pixel(0); 16];
        let rect = DirtyRect {
            x0: 0,
            y0: 0,
            x1: 4,
            y1: 4,
        };
        compose_preview_frame(
            &mut destination,
            4,
            4,
            rect,
            PreviewFrame {
                pixels: PreviewPixels::Rgb565 {
                    pixels: &source,
                    stride_pixels: 2,
                },
                source_width: 2,
                source_height: 2,
                display_width: 4,
                display_height: 4,
            },
            true,
            PreviewSurface::full(4),
        );
        assert_eq!(destination[0], source[0]);
        assert_eq!(destination[3], source[1]);
        assert_eq!(destination[12], source[2]);
        assert_eq!(destination[15], source[3]);
    }
}
