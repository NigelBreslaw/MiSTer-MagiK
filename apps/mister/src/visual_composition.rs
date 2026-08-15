// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral composition helpers shared by the device and visual preview.

use crate::arcade_catalog::ArcadeGameView;
use crate::arcade_list_renderer::{ArcadeListGeometry, ArcadeListRenderer, ArcadeListUpdate};
use crate::framebuffer::target::{DirtyRect, UiFrameTarget, blend_565};
use crate::ui_display::{CrtUiMetrics, ScreenOrientation, UiDisplay, UiLayoutGeometry};
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::Duration;

pub struct ArcadeVisualLayer {
    renderer: ArcadeListRenderer,
}

impl ArcadeVisualLayer {
    pub fn new(frame_width: usize, frame_height: usize) -> Self {
        let mut renderer = ArcadeListRenderer::new();
        let mut geometry = Self::geometry(frame_width, frame_height, false);
        geometry.width = geometry.width.min(frame_width.saturating_sub(geometry.x));
        renderer.set_geometry_for_render_h(geometry, frame_height);
        Self { renderer }
    }

    pub fn configure(&mut self, frame_width: usize, frame_height: usize, search: bool) {
        let mut geometry = Self::geometry(frame_width, frame_height, search);
        geometry.width = geometry.width.min(frame_width.saturating_sub(geometry.x));
        let render_height = if frame_height > frame_width && search {
            geometry.y + frame_height * 34 / 100 + 16
        } else {
            frame_height
        };
        self.renderer
            .set_geometry_for_render_h(geometry, render_height);
    }

    pub fn configure_for_display(
        &mut self,
        display: &UiDisplay,
        orientation: ScreenOrientation,
        search: bool,
    ) {
        if !display.direct_video() {
            let layout = UiLayoutGeometry::for_display(display, orientation);
            self.configure(layout.logical_w(), layout.logical_h(), search);
            return;
        }

        let layout = UiLayoutGeometry::for_display(display, orientation);
        let metrics = CrtUiMetrics::for_display(display);
        self.renderer = ArcadeListRenderer::new_for_crt_display(metrics, display);
        let geometry = ArcadeListGeometry::crt_for_content(layout.content_rect(), metrics, search);
        self.renderer
            .set_geometry_for_render_h(geometry, layout.content_rect().bottom());
    }

    fn geometry(frame_width: usize, frame_height: usize, search: bool) -> ArcadeListGeometry {
        if frame_height > frame_width {
            ArcadeListGeometry::portrait(frame_width, frame_height, search)
        } else if search {
            ArcadeListGeometry::search_for_render_w(frame_width)
        } else {
            ArcadeListGeometry::NORMAL
        }
    }

    pub fn compose(
        &mut self,
        target: &mut UiFrameTarget,
        games: ArcadeGameView<'_>,
        selected: usize,
        visual_index: f32,
        force: bool,
    ) -> Option<DirtyRect> {
        let update = self.renderer.draw(games, selected, visual_index, force)?;
        let rect = match update {
            ArcadeListUpdate::Full(rect) | ArcadeListUpdate::Scroll { rect, .. } => rect,
        };
        self.renderer.compose_layer_to_cached(target, true);
        Some(rect)
    }

    pub fn compose_over_backdrop(
        &mut self,
        target: &mut UiFrameTarget,
        backdrop: &[Rgb565Pixel],
        output_layout: mister_magik_framebuffer_scenes::Rgb565OutputLayout,
        games: ArcadeGameView<'_>,
        selected: usize,
        visual_index: f32,
        force: bool,
    ) -> Option<DirtyRect> {
        let update = self.renderer.draw(games, selected, visual_index, force)?;
        let rect = match update {
            ArcadeListUpdate::Full(rect) | ArcadeListUpdate::Scroll { rect, .. } => rect,
        };
        self.renderer
            .compose_layer_over_backdrop_to_oriented_cached(target, backdrop, output_layout, true)
            .then_some(rect)
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

    pub(crate) fn row_start(self, y: usize, x: usize) -> usize {
        (y - self.y) * self.stride + (x - self.x)
    }
}

pub fn hdmi_preview_rect(frame_width: usize, frame_height: usize) -> DirtyRect {
    const PREVIEW_WIDTH: usize = 320;
    const PREVIEW_HEIGHT: usize = 320;

    if frame_height > frame_width {
        let margin = 16.min(frame_width / 4);
        let y0 = 64.min(frame_height);
        let height = (frame_height * 38 / 100).min(frame_height.saturating_sub(y0));
        return DirtyRect {
            x0: margin,
            y0,
            x1: frame_width.saturating_sub(margin),
            y1: y0 + height,
        };
    }

    let list = ArcadeListGeometry::NORMAL;
    // The list may extend into the nominal right pane. Center in the black
    // pixels that remain visible between the list, header, and footer.
    let visible_x0 = (frame_width / 2)
        .max(list.x.saturating_add(list.width))
        .min(frame_width);
    let visible_y0 = list.y.min(frame_height);
    let visible_width = frame_width.saturating_sub(visible_x0);
    let visible_height = list.visible_height(frame_height);
    let preview_width = PREVIEW_WIDTH.min(visible_width);
    let preview_height = PREVIEW_HEIGHT.min(visible_height);
    let x0 = visible_x0 + visible_width.saturating_sub(preview_width) / 2;
    let y0 = visible_y0 + visible_height.saturating_sub(preview_height) / 2;

    DirtyRect {
        x0,
        y0,
        x1: x0 + preview_width,
        y1: y0 + preview_height,
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
    let source_pixels = frame.source_width.checked_mul(frame.source_height)?;
    match frame.pixels {
        PreviewPixels::Empty => {}
        PreviewPixels::Rgb565 {
            pixels,
            stride_pixels,
        } => {
            if stride_pixels < frame.source_width
                || frame
                    .source_height
                    .checked_sub(1)?
                    .checked_mul(stride_pixels)?
                    .checked_add(frame.source_width)?
                    > pixels.len()
            {
                return None;
            }
        }
        PreviewPixels::Rgb8(pixels) => {
            if source_pixels.checked_mul(3)? > pixels.len() {
                return None;
            }
        }
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

#[derive(Clone)]
pub struct ScreenshotTileImage {
    pub pixels: Vec<Rgb565Pixel>,
    pub w: usize,
    pub h: usize,
    pub stride: usize,
}

pub struct ScreenshotTileWall {
    base: Vec<Rgb565Pixel>,
    next: Vec<Rgb565Pixel>,
    page: usize,
    valid: bool,
}

impl ScreenshotTileWall {
    pub fn new(frame_width: usize, frame_height: usize) -> Self {
        Self {
            base: vec![Rgb565Pixel(0); frame_width.saturating_mul(frame_height)],
            next: vec![Rgb565Pixel(0); frame_width.saturating_mul(frame_height)],
            page: usize::MAX,
            valid: false,
        }
    }

    pub fn invalidate(&mut self) {
        self.valid = false;
    }

    pub fn render(
        &mut self,
        destination: &mut [Rgb565Pixel],
        frame_width: usize,
        frame_height: usize,
        images: &[ScreenshotTileImage],
        elapsed: Duration,
    ) {
        const SLOT_WIDTH: usize = 320;
        const SLOT_HEIGHT: usize = 224;
        const SECOND_US: u128 = 1_000_000;

        let gutter_y = frame_height.saturating_sub(SLOT_HEIGHT * 2) / 3;
        let elapsed_us = elapsed.as_micros();
        let page = (elapsed_us / (6 * SECOND_US)) as usize;
        let active = ((elapsed_us / SECOND_US) as usize) % 6;
        let reveal = ((elapsed_us % SECOND_US) as usize * SLOT_WIDTH) / SECOND_US as usize;
        if !self.valid || self.page != page || self.base.len() != destination.len() {
            self.base.resize(destination.len(), Rgb565Pixel(0));
            self.next.resize(destination.len(), Rgb565Pixel(0));
            self.base.fill(rgb888_to_rgb565(2, 4, 10));
            self.next.fill(rgb888_to_rgb565(2, 4, 10));
            for slot in 0..6 {
                let column = slot % 3;
                let row = slot / 3;
                let x = column * SLOT_WIDTH;
                let y = gutter_y + row * (SLOT_HEIGHT + gutter_y);
                fill_rect(
                    &mut self.base,
                    frame_width,
                    frame_height,
                    DirtyRect {
                        x0: x,
                        y0: y,
                        x1: x + SLOT_WIDTH,
                        y1: y + SLOT_HEIGHT,
                    },
                    Rgb565Pixel(0),
                );
                if let Some(image) = tile_image_at(images, page * 6 + slot) {
                    blit_tile_scaled(
                        &mut self.base,
                        frame_width,
                        frame_height,
                        image,
                        x,
                        y,
                        SLOT_WIDTH,
                        SLOT_HEIGHT,
                        230,
                    );
                }
                if let Some(image) = tile_image_at(images, (page + 1) * 6 + slot) {
                    blit_tile_scaled(
                        &mut self.next,
                        frame_width,
                        frame_height,
                        image,
                        x,
                        y,
                        SLOT_WIDTH,
                        SLOT_HEIGHT,
                        255,
                    );
                }
                stroke_rect(
                    &mut self.base,
                    frame_width,
                    frame_height,
                    x,
                    y,
                    SLOT_WIDTH,
                    SLOT_HEIGHT,
                    rgb888_to_rgb565(70, 255, 210),
                );
            }
            self.page = page;
            self.valid = true;
        }

        destination.copy_from_slice(&self.base);
        if reveal == 0 {
            return;
        }
        let column = active % 3;
        let row = active / 3;
        let x = column * SLOT_WIDTH;
        let y = gutter_y + row * (SLOT_HEIGHT + gutter_y);
        copy_rect(
            destination,
            &self.next,
            frame_width,
            frame_height,
            DirtyRect {
                x0: x,
                y0: y,
                x1: x + reveal,
                y1: y + SLOT_HEIGHT,
            },
        );
        stroke_rect(
            destination,
            frame_width,
            frame_height,
            x,
            y,
            SLOT_WIDTH,
            SLOT_HEIGHT,
            rgb888_to_rgb565(70, 255, 210),
        );
    }
}

fn tile_image_at(images: &[ScreenshotTileImage], index: usize) -> Option<&ScreenshotTileImage> {
    (!images.is_empty()).then(|| &images[index % images.len()])
}

#[allow(clippy::too_many_arguments)]
fn blit_tile_scaled(
    destination: &mut [Rgb565Pixel],
    frame_width: usize,
    frame_height: usize,
    image: &ScreenshotTileImage,
    x: usize,
    y: usize,
    output_width: usize,
    output_height: usize,
    tint: u8,
) {
    if output_width == 0 || output_height == 0 || image.w == 0 || image.h == 0 {
        return;
    }
    let x1 = (x + output_width).min(frame_width);
    let y1 = (y + output_height).min(frame_height);
    let step_x = ((image.w << 16) / output_width).max(1);
    let step_y = ((image.h << 16) / output_height).max(1);
    let dark = rgb888_to_rgb565(0, 0, 18);
    let mut source_y_fixed = 0usize;
    for destination_y in y..y1 {
        let source_y = (source_y_fixed >> 16).min(image.h - 1);
        let mut source_x_fixed = 0usize;
        for destination_x in x..x1 {
            let source_x = (source_x_fixed >> 16).min(image.w - 1);
            let source = image.pixels[source_y * image.stride + source_x];
            destination[destination_y * frame_width + destination_x] = if tint == 255 {
                source
            } else {
                blend_565(dark, source, tint)
            };
            source_x_fixed = source_x_fixed.saturating_add(step_x);
        }
        source_y_fixed = source_y_fixed.saturating_add(step_y);
    }
}

fn fill_rect(
    destination: &mut [Rgb565Pixel],
    frame_width: usize,
    frame_height: usize,
    rect: DirtyRect,
    color: Rgb565Pixel,
) {
    for y in rect.y0..rect.y1.min(frame_height) {
        let start = y * frame_width + rect.x0.min(frame_width);
        let end = y * frame_width + rect.x1.min(frame_width);
        destination[start..end].fill(color);
    }
}

fn copy_rect(
    destination: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    frame_width: usize,
    frame_height: usize,
    rect: DirtyRect,
) {
    for y in rect.y0..rect.y1.min(frame_height) {
        let start = y * frame_width + rect.x0.min(frame_width);
        let end = y * frame_width + rect.x1.min(frame_width);
        destination[start..end].copy_from_slice(&source[start..end]);
    }
}

#[allow(clippy::too_many_arguments)]
fn stroke_rect(
    destination: &mut [Rgb565Pixel],
    frame_width: usize,
    frame_height: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: Rgb565Pixel,
) {
    if width == 0 || height == 0 {
        return;
    }
    fill_rect(
        destination,
        frame_width,
        frame_height,
        DirtyRect {
            x0: x,
            y0: y,
            x1: x + width,
            y1: y + 2,
        },
        color,
    );
    fill_rect(
        destination,
        frame_width,
        frame_height,
        DirtyRect {
            x0: x,
            y0: y + height.saturating_sub(2),
            x1: x + width,
            y1: y + height,
        },
        color,
    );
    fill_rect(
        destination,
        frame_width,
        frame_height,
        DirtyRect {
            x0: x,
            y0: y,
            x1: x + 2,
            y1: y + height,
        },
        color,
    );
    fill_rect(
        destination,
        frame_width,
        frame_height,
        DirtyRect {
            x0: x + width.saturating_sub(2),
            y0: y,
            x1: x + width,
            y1: y + height,
        },
        color,
    );
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
    fn hdmi_preview_is_centered_in_visible_black_area() {
        for (frame_width, frame_height, x0, y0, x1, y1) in [
            (683, 384, 518, 56, 683, 352),
            (960, 540, 579, 122, 899, 442),
            (960, 600, 579, 136, 899, 456),
            (1024, 768, 611, 136, 931, 456),
            (1280, 720, 800, 136, 1120, 456),
        ] {
            assert_eq!(
                hdmi_preview_rect(frame_width, frame_height),
                DirtyRect { x0, y0, x1, y1 }
            );
        }
    }

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

    #[test]
    fn screenshot_tile_wall_is_deterministic_for_fixture_images() {
        let images = (0..12)
            .map(|index| ScreenshotTileImage {
                pixels: vec![Rgb565Pixel(index as u16 + 1); 4],
                w: 2,
                h: 2,
                stride: 2,
            })
            .collect::<Vec<_>>();
        let mut first = vec![Rgb565Pixel(0); 960 * 540];
        let mut second = vec![Rgb565Pixel(0); 960 * 540];
        ScreenshotTileWall::new(960, 540).render(
            &mut first,
            960,
            540,
            &images,
            Duration::from_millis(1_500),
        );
        ScreenshotTileWall::new(960, 540).render(
            &mut second,
            960,
            540,
            &images,
            Duration::from_millis(1_500),
        );
        assert_eq!(first, second);
        assert!(first.iter().any(|pixel| pixel.0 != 0));
    }
}
