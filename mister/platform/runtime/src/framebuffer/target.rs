// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

pub use crate::framebuffer::damage::{DirtyRect, DirtyRectList, subtract_dirty_rects};
use crate::framebuffer::format::production_label;
use slint::platform::software_renderer::{PhysicalRegion, Rgb565Pixel, SoftwareRenderer};
use std::sync::OnceLock;

const DEFAULT_DIRTY_RECT_BROAD_PCT: usize = 85;

pub fn build_launcher_present_plan(
    base: Option<DirtyRect>,
    cached_overlays: &DirtyRectList,
    direct_overlays: &DirtyRectList,
) -> DirtyRectList {
    let mut layers = DirtyRectList::new();
    layers.push_if_some(base);
    layers.extend_from(cached_overlays);

    build_launcher_present_plan_from_layers(&layers, direct_overlays)
}

pub fn build_launcher_present_plan_from_layers(
    cached_layers: &DirtyRectList,
    direct_overlays: &DirtyRectList,
) -> DirtyRectList {
    let layers = cached_layers;

    let mut plan = DirtyRectList::new();
    let layer_count = layers.len();
    for idx in 0..layer_count {
        let mut cuts = DirtyRectList::new();
        for later_idx in (idx + 1)..layer_count {
            cuts.push(layers.get(later_idx).expect("layer index is in range"));
        }
        cuts.extend_from(direct_overlays);
        plan.extend_from(&subtract_dirty_rects(
            DirtyRectList::from_one(layers.get(idx).expect("layer index is in range")),
            &cuts,
        ));
    }
    plan
}

pub fn dirty_rect_broad_pct() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("MISTER_DIRTY_RECT_BROAD_PCT")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .map(|value| value.clamp(1, 100))
            .unwrap_or(DEFAULT_DIRTY_RECT_BROAD_PCT)
    })
}

pub fn dirty_rect_is_broad(rect: DirtyRect, render_w: usize) -> bool {
    rect.width() * 100 >= render_w * dirty_rect_broad_pct()
}

pub fn format_dirty_rect(rect: Option<DirtyRect>) -> String {
    match rect {
        Some(rect) => format!(
            "x0={} y0={} x1={} y1={} rows={}",
            rect.x0,
            rect.y0,
            rect.x1,
            rect.y1,
            rect.rows()
        ),
        None => "none".to_string(),
    }
}

pub fn dirty_rect(region: &PhysicalRegion, render_w: usize, render_h: usize) -> Option<DirtyRect> {
    let o = region.bounding_box_origin();
    let s = region.bounding_box_size();
    dirty_rect_from_bounds(o.x, o.y, s.width, s.height, render_w, render_h)
}

pub fn dirty_rects(region: &PhysicalRegion, render_w: usize, render_h: usize) -> DirtyRectList {
    dirty_rects_from_bounds(
        region
            .iter()
            .map(|(origin, size)| (origin.x, origin.y, size.width, size.height)),
        render_w,
        render_h,
    )
}

fn dirty_rects_from_bounds(
    bounds: impl IntoIterator<Item = (i32, i32, u32, u32)>,
    render_w: usize,
    render_h: usize,
) -> DirtyRectList {
    let mut damage = DirtyRectList::new();
    for (x, y, width, height) in bounds {
        damage.push_if_some(dirty_rect_from_bounds(
            x, y, width, height, render_w, render_h,
        ));
    }
    damage
}

pub fn dirty_rect_from_bounds(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    render_w: usize,
    render_h: usize,
) -> Option<DirtyRect> {
    if width == 0 || height == 0 {
        return None;
    }
    let render_w = render_w as i64;
    let render_h = render_h as i64;
    let x0 = (x as i64).clamp(0, render_w) as usize;
    let x1 = ((x as i64) + width as i64).clamp(0, render_w) as usize;
    let y0 = (y as i64).clamp(0, render_h) as usize;
    let y1 = ((y as i64) + height as i64).clamp(0, render_h) as usize;
    if x1 > x0 && y1 > y0 {
        Some(DirtyRect { x0, y0, x1, y1 })
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferTargetGeometry {
    render_w: usize,
    render_h: usize,
}

impl FramebufferTargetGeometry {
    pub fn new(render_w: usize, render_h: usize) -> Self {
        Self { render_w, render_h }
    }

    pub fn render_w(self) -> usize {
        self.render_w
    }

    pub fn render_h(self) -> usize {
        self.render_h
    }

    pub fn full_rect(self) -> DirtyRect {
        DirtyRect {
            x0: 0,
            y0: 0,
            x1: self.render_w,
            y1: self.render_h,
        }
    }
}

#[derive(Clone, Copy)]
pub struct CachedFrameView<'a> {
    pixels: &'a [Rgb565Pixel],
    stride: usize,
    height: usize,
}

impl<'a> CachedFrameView<'a> {
    pub fn new(pixels: &'a [Rgb565Pixel], stride: usize, height: usize) -> Self {
        debug_assert!(pixels.len() >= stride.saturating_mul(height));
        Self {
            pixels,
            stride,
            height,
        }
    }

    pub fn pixels(self) -> &'a [Rgb565Pixel] {
        self.pixels
    }

    pub fn stride(self) -> usize {
        self.stride
    }

    pub fn width(self) -> usize {
        self.stride
    }

    pub fn height(self) -> usize {
        self.height
    }
}

#[derive(Clone, Copy)]
pub struct DirectPreviewView<'a> {
    pixels: &'a [Rgb565Pixel],
    rect: DirtyRect,
}

#[derive(Clone, Copy)]
pub(crate) struct StridedFrameRegion<'a> {
    pub(crate) pixels: &'a [Rgb565Pixel],
    pub(crate) stride: usize,
    pub(crate) src_x: usize,
    pub(crate) src_y: usize,
}

impl<'a> DirectPreviewView<'a> {
    pub fn pixels(self) -> &'a [Rgb565Pixel] {
        self.pixels
    }

    pub fn rect(self) -> DirtyRect {
        self.rect
    }

    pub fn stride(self) -> usize {
        self.rect.width()
    }

    pub(crate) fn region(self, rect: DirtyRect) -> Option<StridedFrameRegion<'a>> {
        if !self.rect.contains(rect) {
            return None;
        }
        Some(StridedFrameRegion {
            pixels: self.pixels,
            stride: self.stride(),
            src_x: rect.x0 - self.rect.x0,
            src_y: rect.y0 - self.rect.y0,
        })
    }
}

pub fn blend_565(from: Rgb565Pixel, to: Rgb565Pixel, alpha: u8) -> Rgb565Pixel {
    let f = from.0 as u32;
    let t = to.0 as u32;
    let a = ((alpha as u32 + 4) >> 3).min(32);
    if a == 0 {
        return from;
    }
    if a >= 32 {
        return to;
    }
    let ia = 32 - a;
    let rb = (((f & 0xf81f) * ia + (t & 0xf81f) * a) >> 5) & 0xf81f;
    let g = (((f & 0x07e0) * ia + (t & 0x07e0) * a) >> 5) & 0x07e0;
    Rgb565Pixel((rb | g) as u16)
}

pub fn brighten_565(pixel: Rgb565Pixel) -> Rgb565Pixel {
    let v = pixel.0 as u32;
    let r = ((v >> 11) & 0x1f).saturating_add(8).min(0x1f);
    let g = ((v >> 5) & 0x3f).saturating_add(16).min(0x3f);
    let b = (v & 0x1f).saturating_add(8).min(0x1f);
    Rgb565Pixel(((r << 11) | (g << 5) | b) as u16)
}

#[allow(clippy::too_many_arguments)]
fn compose_rect_565_strided_to_cached(
    cached: &mut [Rgb565Pixel],
    cached_stride: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    src: &[Rgb565Pixel],
    src_stride: usize,
    src_x: usize,
    src_y: usize,
) {
    if w == 0 || h == 0 || cached_stride == 0 || src_stride == 0 {
        return;
    }
    if x.saturating_add(w) > cached_stride {
        return;
    }
    for row in 0..h {
        let src_start = (src_y + row)
            .saturating_mul(src_stride)
            .saturating_add(src_x);
        let src_end = src_start.saturating_add(w);
        let dst_start = (y + row).saturating_mul(cached_stride).saturating_add(x);
        let dst_end = dst_start.saturating_add(w);
        if src_end > src.len() || dst_end > cached.len() {
            return;
        }
        cached[dst_start..dst_end].copy_from_slice(&src[src_start..src_end]);
    }
}

pub struct UiFrameTarget {
    cached: Vec<Rgb565Pixel>,
    cached_stride: usize,
    direct_preview: Vec<Rgb565Pixel>,
    direct_preview_rect: Option<DirtyRect>,
}

impl UiFrameTarget {
    pub fn cached(geometry: FramebufferTargetGeometry) -> Self {
        Self {
            cached: vec![Rgb565Pixel(0); geometry.render_w() * geometry.render_h()],
            cached_stride: geometry.render_w(),
            direct_preview: Vec::new(),
            direct_preview_rect: None,
        }
    }

    pub fn open(geometry: FramebufferTargetGeometry) -> Self {
        crate::ui_logln!(
            "slint-render-target=cached fb-format={}",
            production_label()
        );
        Self::cached(geometry)
    }

    pub fn render(
        &mut self,
        renderer: &SoftwareRenderer,
        geometry: FramebufferTargetGeometry,
    ) -> PhysicalRegion {
        self.cached_stride = geometry.render_w();
        renderer.render(&mut self.cached, geometry.render_w())
    }

    #[cfg(feature = "bench-scenes")]
    pub fn label(&self) -> &'static str {
        "cached-565"
    }

    pub fn compose_rect_565(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Rgb565Pixel],
    ) {
        self.compose_rect_565_strided(x, y, w, h, src, w, 0, 0);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn compose_rect_565_strided(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Rgb565Pixel],
        src_stride: usize,
        src_x: usize,
        src_y: usize,
    ) {
        if w == 0 || h == 0 || src_stride == 0 {
            return;
        }
        compose_rect_565_strided_to_cached(
            &mut self.cached,
            self.cached_stride,
            x,
            y,
            w,
            h,
            src,
            src_stride,
            src_x,
            src_y,
        );
    }

    pub fn direct_preview_565_rect_mut(&mut self, rect: DirtyRect) -> (&mut [Rgb565Pixel], usize) {
        let stride = rect.width();
        let len = stride * (rect.y1 - rect.y0);
        if self.direct_preview.len() != len {
            self.direct_preview.resize(len, Rgb565Pixel(0));
        }
        self.direct_preview_rect = Some(rect);
        (&mut self.direct_preview, stride)
    }

    pub fn compose_direct_preview_rect(&mut self, rect: DirtyRect) -> u32 {
        let Some(backing_rect) = self.direct_preview_rect else {
            return 0;
        };
        if !backing_rect.contains(rect) {
            return 0;
        }
        compose_rect_565_strided_to_cached(
            &mut self.cached,
            self.cached_stride,
            rect.x0,
            rect.y0,
            rect.x1 - rect.x0,
            rect.y1 - rect.y0,
            &self.direct_preview,
            backing_rect.width(),
            rect.x0 - backing_rect.x0,
            rect.y0 - backing_rect.y0,
        );
        rect.rows()
    }

    pub fn cached_frame_view(&self) -> CachedFrameView<'_> {
        let height = if self.cached_stride == 0 {
            0
        } else {
            self.cached.len() / self.cached_stride
        };
        CachedFrameView::new(&self.cached, self.cached_stride, height)
    }

    pub fn swap_cached_565(
        &mut self,
        replacement: &mut Vec<Rgb565Pixel>,
        stride_pixels: usize,
    ) -> bool {
        if stride_pixels != self.cached_stride || replacement.len() != self.cached.len() {
            return false;
        }
        std::mem::swap(&mut self.cached, replacement);
        true
    }

    pub fn direct_preview_view(&self) -> Option<DirectPreviewView<'_>> {
        self.direct_preview_rect.map(|rect| DirectPreviewView {
            pixels: &self.direct_preview,
            rect,
        })
    }

    pub fn cached_565_mut(&mut self) -> &mut [Rgb565Pixel] {
        &mut self.cached
    }

    pub fn cached_565(&self) -> &[Rgb565Pixel] {
        &self.cached
    }

    pub fn into_cached_565(self) -> Vec<Rgb565Pixel> {
        self.cached
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> DirtyRect {
        DirtyRect { x0, y0, x1, y1 }
    }

    fn assert_no_intersections(rects: &[DirtyRect]) {
        for (idx, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(idx + 1) {
                assert_eq!(a.intersection(*b), None, "{a:?} intersects {b:?}");
            }
        }
    }

    fn covered_area(rects: &[DirtyRect]) -> usize {
        rects.iter().map(|rect| rect.area()).sum()
    }

    #[test]
    fn present_plan_splits_full_frame_around_cached_and_direct_overlays() {
        let base = rect(0, 0, 10, 10);
        let cached_overlay = rect(2, 2, 7, 7);
        let direct_overlay = rect(4, 4, 9, 9);

        let mut cached_overlays = DirtyRectList::new();
        cached_overlays.push(cached_overlay);
        let mut direct_overlays = DirtyRectList::new();
        direct_overlays.push(direct_overlay);
        let plan =
            build_launcher_present_plan(Some(base), &cached_overlays, &direct_overlays).to_vec();
        let mut all_copies = plan.clone();
        all_copies.push(direct_overlay);

        assert_no_intersections(&all_copies);
        assert_eq!(covered_area(&all_copies), base.area());
    }

    #[test]
    fn present_plan_keeps_cached_overlay_when_base_is_clean() {
        let cached_overlay = rect(20, 30, 40, 50);

        let mut cached_overlays = DirtyRectList::new();
        cached_overlays.push(cached_overlay);
        let direct_overlays = DirtyRectList::new();
        let plan = build_launcher_present_plan(None, &cached_overlays, &direct_overlays).to_vec();

        assert_eq!(plan, vec![cached_overlay]);
    }

    #[test]
    fn subtraction_falls_back_to_original_rects_when_fragments_exceed_capacity() {
        let source = rect(0, 0, 10, 10);
        let mut rects = DirtyRectList::new();
        for _ in 0..DIRTY_RECT_LIST_CAP {
            rects.push(source);
        }
        let cuts = DirtyRectList::from_one(rect(2, 2, 8, 8));

        let result = subtract_dirty_rects(rects, &cuts);

        assert_eq!(result, rects);
    }

    #[test]
    fn direct_preview_can_be_composed_into_cached_frame() {
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(6, 4));
        target.cached_565_mut().fill(Rgb565Pixel(0x0001));
        let preview_rect = rect(2, 1, 5, 3);
        let (preview, stride) = target.direct_preview_565_rect_mut(preview_rect);
        assert_eq!(stride, 3);
        preview.copy_from_slice(&[
            Rgb565Pixel(0x1000),
            Rgb565Pixel(0x1001),
            Rgb565Pixel(0x1002),
            Rgb565Pixel(0x1003),
            Rgb565Pixel(0x1004),
            Rgb565Pixel(0x1005),
        ]);

        assert_eq!(target.compose_direct_preview_rect(rect(3, 1, 5, 3)), 2);

        let cached = target.cached_565();
        assert_eq!(cached[1 * 6 + 2], Rgb565Pixel(0x0001));
        assert_eq!(cached[1 * 6 + 3], Rgb565Pixel(0x1001));
        assert_eq!(cached[1 * 6 + 4], Rgb565Pixel(0x1002));
        assert_eq!(cached[2 * 6 + 3], Rgb565Pixel(0x1004));
        assert_eq!(cached[2 * 6 + 4], Rgb565Pixel(0x1005));
        assert_eq!(cached[2 * 6 + 5], Rgb565Pixel(0x0001));
    }

    #[test]
    fn frame_views_expose_surface_geometry_without_output_ownership() {
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(8, 4));
        let preview_rect = rect(2, 1, 6, 3);
        target
            .direct_preview_565_rect_mut(preview_rect)
            .0
            .fill(Rgb565Pixel(7));

        let cached = target.cached_frame_view();
        assert_eq!(
            (cached.width(), cached.height(), cached.stride()),
            (8, 4, 8)
        );
        assert_eq!(cached.pixels().len(), 32);

        let preview = target.direct_preview_view().expect("preview view");
        assert_eq!(preview.rect(), preview_rect);
        assert_eq!(preview.stride(), 4);
        assert_eq!(preview.pixels(), &[Rgb565Pixel(7); 8]);
        assert!(preview.region(rect(3, 1, 5, 2)).is_some());
        assert!(preview.region(rect(1, 1, 5, 2)).is_none());
    }

    #[test]
    fn cached_buffer_swap_preserves_allocations_and_rejects_wrong_geometry() {
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(6, 4));
        target.cached_565_mut().fill(Rgb565Pixel(0x1111));
        let original_ptr = target.cached_565().as_ptr();
        let mut replacement = vec![Rgb565Pixel(0x2222); 24];
        let replacement_ptr = replacement.as_ptr();

        assert!(target.swap_cached_565(&mut replacement, 6));
        assert_eq!(target.cached_565().as_ptr(), replacement_ptr);
        assert_eq!(replacement.as_ptr(), original_ptr);
        assert!(target.cached_565().iter().all(|pixel| pixel.0 == 0x2222));
        assert!(replacement.iter().all(|pixel| pixel.0 == 0x1111));

        let cached_ptr = target.cached_565().as_ptr();
        let mut wrong_len = vec![Rgb565Pixel(0); 23];
        assert!(!target.swap_cached_565(&mut wrong_len, 6));
        assert_eq!(target.cached_565().as_ptr(), cached_ptr);
        let mut wrong_stride = vec![Rgb565Pixel(0); 24];
        assert!(!target.swap_cached_565(&mut wrong_stride, 5));
        assert_eq!(target.cached_565().as_ptr(), cached_ptr);
    }

    #[test]
    fn dirty_rect_ignores_fully_negative_bounds() {
        assert_eq!(dirty_rect_from_bounds(-40, 10, 20, 20, 960, 540), None);
        assert_eq!(dirty_rect_from_bounds(10, -40, 20, 20, 960, 540), None);
    }

    #[test]
    fn dirty_rect_clips_partially_negative_bounds() {
        assert_eq!(
            dirty_rect_from_bounds(-10, -5, 30, 20, 960, 540),
            Some(DirtyRect {
                x0: 0,
                y0: 0,
                x1: 20,
                y1: 15
            })
        );
    }

    #[test]
    fn dirty_rect_ignores_zero_area_bounds() {
        assert_eq!(dirty_rect_from_bounds(10, 10, 0, 20, 960, 540), None);
        assert_eq!(dirty_rect_from_bounds(10, 10, 20, 0, 960, 540), None);
    }

    #[test]
    fn dirty_rect_keeps_in_bounds_rect() {
        assert_eq!(
            dirty_rect_from_bounds(10, 20, 30, 40, 960, 540),
            Some(DirtyRect {
                x0: 10,
                y0: 20,
                x1: 40,
                y1: 60
            })
        );
    }

    #[test]
    fn dirty_rect_list_preserves_separated_regions_and_clips_each_one() {
        let damage = dirty_rects_from_bounds(
            [
                (10, 20, 30, 40),
                (500, 300, 20, 10),
                (-5, 530, 20, 20),
                (1_000, 1_000, 10, 10),
            ],
            960,
            540,
        );
        assert_eq!(
            damage.to_vec(),
            vec![
                DirtyRect {
                    x0: 10,
                    y0: 20,
                    x1: 40,
                    y1: 60,
                },
                DirtyRect {
                    x0: 500,
                    y0: 300,
                    x1: 520,
                    y1: 310,
                },
                DirtyRect {
                    x0: 0,
                    y0: 530,
                    x1: 15,
                    y1: 540,
                },
            ]
        );
    }
}
