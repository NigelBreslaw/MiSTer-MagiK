// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::framebuffer::mapped::MappedRgb565Framebuffer;
use crate::framebuffer::scanout_slots::ScanoutSlotsRgb565Framebuffer;
use crate::framebuffer::stream;
use crate::framebuffer::target::{
    CachedFrameView, DirtyRect, PhysicalLayerView, StridedFrameRegion, dirty_rect_is_broad,
};
use crate::framebuffer::vertical_scale::Rgb565FrameView;
use crate::framebuffer::vertical_scale::VerticalRect;
use mister_magik_framebuffer_stream::{FrameGeometry, FrameRect};
use slint::platform::software_renderer::Rgb565Pixel;

fn stream_geometry(disp: &MappedRgb565Framebuffer) -> FrameGeometry {
    FrameGeometry {
        width: disp.width() as u32,
        height: disp.height() as u32,
        stride_pixels: disp.width() as u32,
    }
}

fn stream_rect(rect: DirtyRect) -> FrameRect {
    FrameRect {
        x: rect.x0 as u32,
        y: rect.y0 as u32,
        width: rect.width() as u32,
        height: rect.rows(),
    }
}

fn target_rect(rect: VerticalRect) -> DirtyRect {
    DirtyRect {
        x0: rect.x0,
        y0: rect.y0,
        x1: rect.x1,
        y1: rect.y1,
    }
}

#[cold]
#[inline(never)]
fn log_copy_error(context: &str, err: &dyn std::fmt::Display) {
    crate::ui_errln!("framebuffer present {context} failed: {err}");
}

fn cached_copy_rect(view: CachedFrameView<'_>, rect: DirtyRect) -> DirtyRect {
    if rect.is_full_width(view.width()) || dirty_rect_is_broad(rect, view.width()) {
        DirtyRect {
            x0: 0,
            y0: rect.y0,
            x1: view.width(),
            y1: rect.y1,
        }
    } else {
        rect
    }
}

fn cached_geometry_compatible(view: CachedFrameView<'_>, framebuffer_width: usize) -> bool {
    view.width() == framebuffer_width && view.height() != 0
}

fn copy_cached_rows_to_fb0(
    disp: &mut MappedRgb565Framebuffer,
    view: CachedFrameView<'_>,
    y0: usize,
    y1: usize,
) -> Option<crate::framebuffer::vertical_scale::VerticalCopyStats> {
    if !cached_geometry_compatible(view, disp.width()) || y1 > view.height() || y0 > y1 {
        log_copy_error(
            "rows",
            &format_args!(
                "geometry mismatch view={}x{} fb={}x{} rows={y0}..{y1}",
                view.width(),
                view.height(),
                disp.width(),
                disp.height(),
            ),
        );
        return None;
    }
    disp.present_vertical_rect_565(
        Rgb565FrameView {
            pixels: view.pixels(),
            width: view.width(),
            height: view.height(),
            stride_pixels: view.stride(),
        },
        DirtyRect {
            x0: 0,
            y0,
            x1: view.width(),
            y1,
        },
    )
    .map_err(|error| log_copy_error("rows", &error))
    .ok()
    .flatten()
}

pub fn copy_cached_rows_565(
    disp: &mut MappedRgb565Framebuffer,
    view: CachedFrameView<'_>,
    y0: usize,
    y1: usize,
) -> u32 {
    let Some(stats) = copy_cached_rows_to_fb0(disp, view, y0, y1) else {
        return 0;
    };
    let destination = disp.frame_view_565();
    stream::publish_strided_rect(
        stream_geometry(disp),
        stream_rect(target_rect(stats.destination_rect)),
        destination.pixels,
        destination.stride_pixels,
        stats.destination_rect.x0,
        stats.destination_rect.y0,
    );
    stats.destination_rect.rows() as u32
}

pub fn copy_cached_rect_565(
    disp: &mut MappedRgb565Framebuffer,
    view: CachedFrameView<'_>,
    rect: DirtyRect,
) -> Option<DirtyRect> {
    if !cached_geometry_compatible(view, disp.width()) {
        log_copy_error(
            "rect",
            &format_args!(
                "geometry mismatch view={}x{} fb={}x{}",
                view.width(),
                view.height(),
                disp.width(),
                disp.height(),
            ),
        );
        return None;
    }
    let copied_rect = cached_copy_rect(view, rect);
    let stats = match disp.present_vertical_rect_565(
        Rgb565FrameView {
            pixels: view.pixels(),
            width: view.width(),
            height: view.height(),
            stride_pixels: view.stride(),
        },
        copied_rect,
    ) {
        Ok(Some(stats)) => stats,
        Ok(None) => return None,
        Err(e) => {
            log_copy_error("rect", &e);
            return None;
        }
    };
    let destination = disp.frame_view_565();
    stream::publish_strided_rect(
        stream_geometry(disp),
        stream_rect(target_rect(stats.destination_rect)),
        destination.pixels,
        destination.stride_pixels,
        stats.destination_rect.x0,
        stats.destination_rect.y0,
    );
    Some(target_rect(stats.destination_rect))
}

pub fn copy_dense_rect_565(
    disp: &mut MappedRgb565Framebuffer,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    src: &[Rgb565Pixel],
) {
    if let Err(e) = disp.present_rect_565(x, y, w, h, src) {
        log_copy_error("dense rect", &e);
        return;
    }
    stream::publish_dense_rect(
        stream_geometry(disp),
        FrameRect {
            x: x as u32,
            y: y as u32,
            width: w as u32,
            height: h as u32,
        },
        src,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn copy_strided_rect_565(
    disp: &mut MappedRgb565Framebuffer,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    src: &[Rgb565Pixel],
    src_stride: usize,
    src_x: usize,
    src_y: usize,
) -> bool {
    if let Err(e) = disp.present_rect_565_strided(x, y, w, h, src, src_stride, src_x, src_y) {
        log_copy_error("strided rect", &e);
        return false;
    }
    stream::publish_strided_rect(
        stream_geometry(disp),
        FrameRect {
            x: x as u32,
            y: y as u32,
            width: w as u32,
            height: h as u32,
        },
        src,
        src_stride,
        src_x,
        src_y,
    );
    true
}

pub fn copy_physical_layer_rect_565(
    disp: &mut MappedRgb565Framebuffer,
    view: PhysicalLayerView<'_>,
    rect: DirtyRect,
) -> u32 {
    let geometry = stream_geometry(disp);
    copy_physical_layer_region(
        view,
        rect,
        "physical layer rect",
        |region| {
            disp.present_rect_565_strided(
                rect.x0,
                rect.y0,
                rect.width(),
                rect.rows() as usize,
                region.pixels,
                region.stride,
                region.src_x,
                region.src_y,
            )
        },
        |region| {
            stream::publish_strided_rect(
                geometry,
                stream_rect(rect),
                region.pixels,
                region.stride,
                region.src_x,
                region.src_y,
            );
        },
    )
}

pub fn copy_physical_layer_rect_to_hidden(
    hidden: &mut ScanoutSlotsRgb565Framebuffer,
    view: PhysicalLayerView<'_>,
    rect: DirtyRect,
) -> u32 {
    copy_physical_layer_region(
        view,
        rect,
        "hidden physical layer rect",
        |region| {
            hidden.copy_rect_565_strided(
                rect.x0,
                rect.y0,
                rect.width(),
                rect.rows() as usize,
                region.pixels,
                region.stride,
                region.src_x,
                region.src_y,
            )
        },
        |_| {},
    )
}

fn copy_physical_layer_region<T, E>(
    view: PhysicalLayerView<'_>,
    rect: DirtyRect,
    context: &str,
    copy: impl FnOnce(StridedFrameRegion<'_>) -> Result<T, E>,
    publish: impl FnOnce(StridedFrameRegion<'_>),
) -> u32
where
    E: std::fmt::Display,
{
    let Some(region) = view.region(rect) else {
        return 0;
    };
    if let Err(e) = copy(region) {
        log_copy_error(context, &e);
        return 0;
    }
    publish(region);
    rect.rows()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framebuffer::target::{FramebufferTargetGeometry, UiFrameTarget};
    use std::cell::RefCell;

    fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> DirtyRect {
        DirtyRect { x0, y0, x1, y1 }
    }

    #[test]
    fn cached_copy_rect_promotes_broad_rect_to_full_rows() {
        let pixels = vec![Rgb565Pixel(0); 100 * 20];
        let view = CachedFrameView::new(&pixels, 100, 20);
        assert_eq!(
            cached_copy_rect(view, rect(15, 10, 100, 12)),
            rect(0, 10, 100, 12)
        );
        assert_eq!(
            cached_copy_rect(view, rect(20, 10, 40, 12)),
            rect(20, 10, 40, 12)
        );
    }

    #[test]
    fn cached_geometry_requires_matching_width_and_nonzero_source_height() {
        let pixels = vec![Rgb565Pixel(0); 100 * 20];
        let view = CachedFrameView::new(&pixels, 100, 20);
        assert!(cached_geometry_compatible(view, 100));
        assert!(!cached_geometry_compatible(view, 99));
    }

    #[test]
    fn direct_preview_copy_uses_subrect_offsets_then_publishes() {
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(8, 4));
        let backing = rect(2, 1, 6, 3);
        target
            .direct_preview_565_rect_mut(backing)
            .0
            .copy_from_slice(&[
                Rgb565Pixel(0),
                Rgb565Pixel(1),
                Rgb565Pixel(2),
                Rgb565Pixel(3),
                Rgb565Pixel(4),
                Rgb565Pixel(5),
                Rgb565Pixel(6),
                Rgb565Pixel(7),
            ]);
        let events = RefCell::new(Vec::new());

        let rows = copy_physical_layer_region(
            target.direct_preview_view().expect("preview view"),
            rect(3, 2, 5, 3),
            "test",
            |region| {
                assert_eq!((region.stride, region.src_x, region.src_y), (4, 1, 1));
                assert_eq!(
                    region.pixels[region.src_y * region.stride + region.src_x].0,
                    5
                );
                events.borrow_mut().push("copy");
                Ok::<(), std::io::Error>(())
            },
            |_| events.borrow_mut().push("publish"),
        );

        assert_eq!(rows, 1);
        assert_eq!(events.into_inner(), ["copy", "publish"]);
    }

    #[test]
    fn direct_preview_copy_failure_returns_zero_without_publication() {
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(8, 4));
        target.direct_preview_565_rect_mut(rect(2, 1, 6, 3));
        let events = RefCell::new(Vec::new());

        let rows = copy_physical_layer_region(
            target.direct_preview_view().expect("preview view"),
            rect(3, 1, 5, 2),
            "test",
            |_| {
                events.borrow_mut().push("copy");
                Err::<(), _>(std::io::Error::other("expected failure"))
            },
            |_| events.borrow_mut().push("publish"),
        );

        assert_eq!(rows, 0);
        assert_eq!(events.into_inner(), ["copy"]);
    }

    #[test]
    fn direct_preview_nonoverlap_is_a_noop() {
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(8, 4));
        target.direct_preview_565_rect_mut(rect(2, 1, 6, 3));
        let events = RefCell::new(Vec::new());

        let rows = copy_physical_layer_region(
            target.direct_preview_view().expect("preview view"),
            rect(0, 0, 1, 1),
            "test",
            |_| {
                events.borrow_mut().push("copy");
                Ok::<(), std::io::Error>(())
            },
            |_| events.borrow_mut().push("publish"),
        );

        assert_eq!(rows, 0);
        assert!(events.into_inner().is_empty());
    }
}
