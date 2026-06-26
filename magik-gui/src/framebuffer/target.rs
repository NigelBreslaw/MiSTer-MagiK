use crate::framebuffer::format::production_label;
use crate::framebuffer::mapped::MappedRgb565Framebuffer;
use slint::platform::software_renderer::{PhysicalRegion, Rgb565Pixel, SoftwareRenderer};
use std::sync::OnceLock;

const DEFAULT_DIRTY_RECT_BROAD_PCT: usize = 85;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtyRect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl DirtyRect {
    pub fn rows(self) -> u32 {
        (self.y1 - self.y0) as u32
    }

    pub fn width(self) -> usize {
        self.x1 - self.x0
    }

    pub fn is_full_width(self, render_w: usize) -> bool {
        self.x0 == 0 && self.x1 >= render_w
    }

    pub fn contains(self, other: DirtyRect) -> bool {
        self.x0 <= other.x0 && self.y0 <= other.y0 && self.x1 >= other.x1 && self.y1 >= other.y1
    }

    pub fn intersection(self, other: DirtyRect) -> Option<DirtyRect> {
        let x0 = self.x0.max(other.x0);
        let y0 = self.y0.max(other.y0);
        let x1 = self.x1.min(other.x1);
        let y1 = self.y1.min(other.y1);
        if x1 > x0 && y1 > y0 {
            Some(DirtyRect { x0, y0, x1, y1 })
        } else {
            None
        }
    }

    #[cfg(test)]
    fn area(self) -> usize {
        self.width() * (self.y1 - self.y0)
    }

    #[cfg_attr(not(feature = "video"), allow(dead_code))]
    pub fn union(self, other: DirtyRect) -> DirtyRect {
        DirtyRect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }
}

// Current launcher composition is one Slint base layer, up to two cached custom
// overlays, and one direct arcade-list overlay. Subtracting those rectangles can
// produce more than 16 fragments in adversarial layouts, so keep enough stack
// space for the known worst case without heap allocation.
const DIRTY_RECT_LIST_CAP: usize = 32;
const EMPTY_DIRTY_RECT: DirtyRect = DirtyRect {
    x0: 0,
    y0: 0,
    x1: 0,
    y1: 0,
};

#[derive(Clone, Copy)]
pub struct DirtyRectList {
    rects: [DirtyRect; DIRTY_RECT_LIST_CAP],
    len: usize,
}

impl DirtyRectList {
    pub fn new() -> Self {
        Self {
            rects: [EMPTY_DIRTY_RECT; DIRTY_RECT_LIST_CAP],
            len: 0,
        }
    }

    pub fn from_one(rect: DirtyRect) -> Self {
        let mut list = Self::new();
        list.push(rect);
        list
    }

    pub fn push_if_some(&mut self, rect: Option<DirtyRect>) {
        if let Some(rect) = rect {
            self.push(rect);
        }
    }

    pub fn extend_from(&mut self, other: &Self) {
        for rect in other.iter() {
            self.push(rect);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = DirtyRect> + '_ {
        self.rects[..self.len].iter().copied()
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn push(&mut self, rect: DirtyRect) {
        if self.len < DIRTY_RECT_LIST_CAP {
            self.rects[self.len] = rect;
            self.len += 1;
        } else {
            debug_assert!(false, "dirty rect list capacity exceeded");
            let last = DIRTY_RECT_LIST_CAP - 1;
            self.rects[last] = self.rects[last].union(rect);
        }
    }

    #[cfg(test)]
    fn to_vec(self) -> Vec<DirtyRect> {
        self.iter().collect()
    }
}

impl Default for DirtyRectList {
    fn default() -> Self {
        Self::new()
    }
}

fn subtract_rect_into(rect: DirtyRect, cut: DirtyRect, out: &mut DirtyRectList) {
    let Some(overlap) = rect.intersection(cut) else {
        out.push(rect);
        return;
    };
    if rect.y0 < overlap.y0 {
        out.push(DirtyRect {
            x0: rect.x0,
            y0: rect.y0,
            x1: rect.x1,
            y1: overlap.y0,
        });
    }
    if overlap.y1 < rect.y1 {
        out.push(DirtyRect {
            x0: rect.x0,
            y0: overlap.y1,
            x1: rect.x1,
            y1: rect.y1,
        });
    }
    if rect.x0 < overlap.x0 {
        out.push(DirtyRect {
            x0: rect.x0,
            y0: overlap.y0,
            x1: overlap.x0,
            y1: overlap.y1,
        });
    }
    if overlap.x1 < rect.x1 {
        out.push(DirtyRect {
            x0: overlap.x1,
            y0: overlap.y0,
            x1: rect.x1,
            y1: overlap.y1,
        });
    }
}

fn subtract_rects(rects: DirtyRectList, cuts: &DirtyRectList) -> DirtyRectList {
    let mut current = rects;
    let mut next = DirtyRectList::new();
    for cut in cuts.iter() {
        next.clear();
        for rect in current.iter() {
            subtract_rect_into(rect, cut, &mut next);
        }
        std::mem::swap(&mut current, &mut next);
        if current.is_empty() {
            break;
        }
    }
    current
}

pub fn build_launcher_present_plan(
    base: Option<DirtyRect>,
    cached_overlays: &DirtyRectList,
    direct_overlays: &DirtyRectList,
) -> DirtyRectList {
    let mut layers = DirtyRectList::new();
    layers.push_if_some(base);
    layers.extend_from(cached_overlays);

    let mut plan = DirtyRectList::new();
    let layer_count = layers.len;
    for idx in 0..layer_count {
        let mut cuts = DirtyRectList::new();
        for later_idx in (idx + 1)..layer_count {
            cuts.push(layers.rects[later_idx]);
        }
        cuts.extend_from(direct_overlays);
        plan.extend_from(&subtract_rects(
            DirtyRectList::from_one(layers.rects[idx]),
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

#[cold]
#[inline(never)]
fn log_present_error(context: &str, err: &dyn std::fmt::Display) {
    eprintln!("framebuffer present {context} failed: {err}");
}

pub fn copy_cached_rows_565(
    disp: &mut MappedRgb565Framebuffer,
    cached: &[Rgb565Pixel],
    y0: usize,
    y1: usize,
) {
    if let Err(e) = disp.present_rows_565(cached, y0, y1) {
        log_present_error("rows", &e);
    }
}

pub fn copy_cached_rect_565(
    disp: &mut MappedRgb565Framebuffer,
    geometry: FramebufferTargetGeometry,
    cached: &[Rgb565Pixel],
    rect: DirtyRect,
) {
    if rect.is_full_width(geometry.render_w()) || dirty_rect_is_broad(rect, geometry.render_w()) {
        copy_cached_rows_565(disp, cached, rect.y0, rect.y1);
        return;
    }
    if let Err(e) = disp.present_rect_565_strided(
        rect.x0,
        rect.y0,
        rect.x1 - rect.x0,
        rect.y1 - rect.y0,
        cached,
        geometry.render_w(),
        rect.x0,
        rect.y0,
    ) {
        log_present_error("rect", &e);
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

pub struct UiFrameTarget {
    cached: Vec<Rgb565Pixel>,
}

impl UiFrameTarget {
    pub fn cached(geometry: FramebufferTargetGeometry) -> Self {
        Self {
            cached: vec![Rgb565Pixel(0); geometry.render_w() * geometry.render_h()],
        }
    }

    pub fn open(geometry: FramebufferTargetGeometry) -> Self {
        println!(
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
        renderer.render(&mut self.cached, geometry.render_w())
    }

    #[cfg(mister_bench_scenes)]
    pub fn label(&self) -> &'static str {
        "cached-565"
    }

    pub fn present_rect(
        &mut self,
        disp: &mut MappedRgb565Framebuffer,
        geometry: FramebufferTargetGeometry,
        rect: DirtyRect,
    ) -> u32 {
        copy_cached_rect_565(disp, geometry, &self.cached, rect);
        rect.rows()
    }

    pub fn present_rect_565(
        &mut self,
        disp: &mut MappedRgb565Framebuffer,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Rgb565Pixel],
    ) {
        if let Err(e) = disp.present_rect_565(x, y, w, h, src) {
            log_present_error("dense rect", &e);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn present_rect_565_strided(
        &mut self,
        disp: &mut MappedRgb565Framebuffer,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Rgb565Pixel],
        src_stride: usize,
        src_x: usize,
        src_y: usize,
    ) {
        if let Err(e) = disp.present_rect_565_strided(x, y, w, h, src, src_stride, src_x, src_y) {
            log_present_error("strided rect", &e);
        }
    }

    pub fn present_rows(
        &mut self,
        disp: &mut MappedRgb565Framebuffer,
        y0: usize,
        y1: usize,
    ) -> u32 {
        copy_cached_rows_565(disp, &self.cached, y0, y1);
        y1.saturating_sub(y0) as u32
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
}
