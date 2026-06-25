use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DirtyRect {
    pub(crate) x0: usize,
    pub(crate) y0: usize,
    pub(crate) x1: usize,
    pub(crate) y1: usize,
}

impl DirtyRect {
    pub(crate) fn rows(self) -> u32 {
        (self.y1 - self.y0) as u32
    }

    pub(super) fn width(self) -> usize {
        self.x1 - self.x0
    }

    pub(super) fn is_full_width(self, render_w: usize) -> bool {
        self.x0 == 0 && self.x1 >= render_w
    }

    pub(super) fn contains(self, other: DirtyRect) -> bool {
        self.x0 <= other.x0 && self.y0 <= other.y0 && self.x1 >= other.x1 && self.y1 >= other.y1
    }

    pub(crate) fn intersection(self, other: DirtyRect) -> Option<DirtyRect> {
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
    pub(super) fn union(self, other: DirtyRect) -> DirtyRect {
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
pub(super) struct DirtyRectList {
    rects: [DirtyRect; DIRTY_RECT_LIST_CAP],
    len: usize,
}

impl DirtyRectList {
    pub(super) fn new() -> Self {
        Self {
            rects: [EMPTY_DIRTY_RECT; DIRTY_RECT_LIST_CAP],
            len: 0,
        }
    }

    pub(super) fn from_one(rect: DirtyRect) -> Self {
        let mut list = Self::new();
        list.push(rect);
        list
    }

    pub(super) fn push_if_some(&mut self, rect: Option<DirtyRect>) {
        if let Some(rect) = rect {
            self.push(rect);
        }
    }

    pub(super) fn extend_from(&mut self, other: &Self) {
        for rect in other.iter() {
            self.push(rect);
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = DirtyRect> + '_ {
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

pub(super) fn build_launcher_present_plan(
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

pub(super) fn dirty_rect_broad_pct() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("MISTER_DIRTY_RECT_BROAD_PCT")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .map(|value| value.clamp(1, 100))
            .unwrap_or(DEFAULT_DIRTY_RECT_BROAD_PCT)
    })
}

pub(super) fn dirty_rect_is_broad(rect: DirtyRect, render_w: usize) -> bool {
    rect.width() * 100 >= render_w * dirty_rect_broad_pct()
}

pub(super) fn launcher_dirty_opt_enabled() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| {
        !matches!(
            std::env::var("MISTER_LAUNCHER_DIRTY_OPT").as_deref(),
            Ok("0") | Ok("off") | Ok("false") | Ok("no")
        )
    })
}

pub(super) fn preview_run_label() -> String {
    std::env::var("MISTER_PREVIEW_RUN_LABEL").unwrap_or_default()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CatalogRefreshPolicy {
    Default,
    Force,
    Off,
}

impl CatalogRefreshPolicy {
    pub(super) fn force_requested(self) -> bool {
        self == Self::Force
    }

    pub(super) fn worker_enabled(self) -> bool {
        self != Self::Off
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Force => "force",
            Self::Off => "off",
        }
    }
}

pub(super) fn catalog_refresh_policy() -> CatalogRefreshPolicy {
    static VALUE: OnceLock<CatalogRefreshPolicy> = OnceLock::new();
    *VALUE.get_or_init(|| {
        catalog_refresh_policy_from_value(std::env::var("MISTER_CATALOG_REFRESH").ok().as_deref())
    })
}

fn catalog_refresh_policy_from_value(value: Option<&str>) -> CatalogRefreshPolicy {
    match value {
        Some("1") | Some("on") | Some("true") | Some("yes") | Some("force") => {
            CatalogRefreshPolicy::Force
        }
        Some("0") | Some("off") | Some("false") | Some("no") | Some("load-only") => {
            CatalogRefreshPolicy::Off
        }
        _ => CatalogRefreshPolicy::Default,
    }
}

pub(super) fn forced_arcade_selected_index() -> Option<usize> {
    std::env::var("MISTER_ARCADE_SELECTED_INDEX")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
}

pub(super) fn apply_forced_arcade_selected(nav: &mut LauncherNav, catalog: &ArcadeCatalog) {
    let Some(index) = forced_arcade_selected_index() else {
        return;
    };
    let count = active_system_game_slice(catalog, nav).len();
    if count == 0 {
        return;
    }
    nav.screen = Screen::Arcade;
    nav.arcade.selected = index.min(count - 1);
    nav.arcade.snap_to_selected();
    keep_bench_arcade_visible(&mut nav.arcade.scroll_y, nav.arcade.selected, count);
}

pub(super) fn format_dirty_rect(rect: Option<DirtyRect>) -> String {
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
pub(super) fn dirty_rect(
    region: &PhysicalRegion,
    render_w: usize,
    render_h: usize,
) -> Option<DirtyRect> {
    let o = region.bounding_box_origin();
    let s = region.bounding_box_size();
    dirty_rect_from_bounds(o.x, o.y, s.width, s.height, render_w, render_h)
}

pub(super) fn dirty_rect_from_bounds(
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

pub(super) fn copy_cached_rows(
    disp: &mut Display,
    ui: &UiDisplay,
    cached: &[Pixel],
    y0: usize,
    y1: usize,
) {
    debug_assert_eq!(ui.fb_scale(), 1);
    disp.copy_rows(cached, y0, y1);
}

pub(super) fn copy_cached_rect(
    disp: &mut Display,
    ui: &UiDisplay,
    cached: &[Pixel],
    rect: DirtyRect,
) {
    if rect.is_full_width(ui.render_w()) || dirty_rect_is_broad(rect, ui.render_w()) {
        copy_cached_rows(disp, ui, cached, rect.y0, rect.y1);
        return;
    }
    debug_assert_eq!(ui.fb_scale(), 1);
    disp.copy_rect(cached, ui.render_w(), rect.x0, rect.y0, rect.x1, rect.y1);
}

pub(super) fn copy_cached_rows_565(
    disp: &mut Display,
    ui: &UiDisplay,
    cached: &[Rgb565Pixel],
    y0: usize,
    y1: usize,
) {
    debug_assert_eq!(ui.fb_scale(), 1);
    disp.copy_rows_565(cached, y0, y1);
}

pub(super) fn copy_cached_rect_565(
    disp: &mut Display,
    ui: &UiDisplay,
    cached: &[Rgb565Pixel],
    rect: DirtyRect,
) {
    if rect.is_full_width(ui.render_w()) || dirty_rect_is_broad(rect, ui.render_w()) {
        copy_cached_rows_565(disp, ui, cached, rect.y0, rect.y1);
        return;
    }
    debug_assert_eq!(ui.fb_scale(), 1);
    disp.copy_rect_565(cached, ui.render_w(), rect.x0, rect.y0, rect.x1, rect.y1);
}

pub(super) fn pixel_to_rgb(pixel: Pixel) -> (u8, u8, u8) {
    (
        ((pixel.0 >> 16) & 0xff) as u8,
        ((pixel.0 >> 8) & 0xff) as u8,
        (pixel.0 & 0xff) as u8,
    )
}

pub(super) fn blend_565(from: Rgb565Pixel, to: Rgb565Pixel, alpha: u8) -> Rgb565Pixel {
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

pub(super) fn brighten_565(pixel: Rgb565Pixel) -> Rgb565Pixel {
    let v = pixel.0 as u32;
    let r = ((v >> 11) & 0x1f).saturating_add(8).min(0x1f);
    let g = ((v >> 5) & 0x3f).saturating_add(16).min(0x3f);
    let b = (v & 0x1f).saturating_add(8).min(0x1f);
    Rgb565Pixel(((r << 11) | (g << 5) | b) as u16)
}

pub(super) struct PresentProbe {
    pixels: Vec<Pixel>,
}

impl PresentProbe {
    const X: usize = 12;
    const Y: usize = 12;
    const W: usize = 208;
    const H: usize = 72;

    pub(super) fn from_env() -> Option<Self> {
        matches!(
            std::env::var("MISTER_PRESENT_PROBE").as_deref(),
            Ok("1") | Ok("on") | Ok("true") | Ok("yes")
        )
        .then(|| Self {
            pixels: vec![Pixel(0); Self::W * Self::H],
        })
    }

    pub(super) fn present(&mut self, disp: &mut Display, frame: u64) -> u32 {
        self.draw(frame);
        disp.copy_rect_from(Self::X, Self::Y, Self::W, Self::H, &self.pixels);
        Self::H as u32
    }

    pub(super) fn draw(&mut self, frame: u64) {
        self.pixels.fill(Pixel::from_rgb(0, 0, 0));
        let edge = if frame & 1 == 0 {
            Pixel::from_rgb(0, 255, 255)
        } else {
            Pixel::from_rgb(255, 0, 255)
        };
        self.fill_rect(0, 0, Self::W, Self::H, Pixel::from_rgb(8, 8, 14));
        self.stroke_rect(0, 0, Self::W, Self::H, edge);

        let flash = if frame & 1 == 0 {
            Pixel::from_rgb(255, 255, 255)
        } else {
            Pixel::from_rgb(0, 0, 0)
        };
        self.fill_rect(6, 6, 36, 36, flash);
        self.stroke_rect(6, 6, 36, 36, edge);

        let marker_x = 48 + (frame as usize % 150);
        self.fill_rect(marker_x, 4, 3, Self::H - 8, Pixel::from_rgb(255, 40, 40));

        let mut value = (frame % 10_000) as u16;
        let mut digits = [0u8; 4];
        for digit in digits.iter_mut().rev() {
            *digit = (value % 10) as u8;
            value /= 10;
        }
        for (i, digit) in digits.into_iter().enumerate() {
            self.draw_digit(58 + i * 28, 9, digit, Pixel::from_rgb(255, 242, 96));
        }

        for bit in 0..8 {
            let on = ((frame >> (7 - bit)) & 1) != 0;
            let color = if on {
                Pixel::from_rgb(64, 255, 96)
            } else {
                Pixel::from_rgb(32, 44, 40)
            };
            self.fill_rect(8 + bit * 24, 52, 18, 12, color);
            self.stroke_rect(8 + bit * 24, 52, 18, 12, Pixel::from_rgb(160, 160, 160));
        }
    }

    pub(super) fn draw_digit(&mut self, x: usize, y: usize, digit: u8, color: Pixel) {
        const SEGMENTS: [u8; 10] = [
            0b1111110, 0b0110000, 0b1101101, 0b1111001, 0b0110011, 0b1011011, 0b1011111, 0b1110000,
            0b1111111, 0b1111011,
        ];
        let mask = SEGMENTS[digit as usize];
        let seg = |this: &mut Self, bit: u8, rx: usize, ry: usize, rw: usize, rh: usize| {
            if (mask & (1 << bit)) != 0 {
                this.fill_rect(x + rx, y + ry, rw, rh, color);
            }
        };
        seg(self, 6, 3, 0, 18, 4);
        seg(self, 5, 20, 3, 4, 14);
        seg(self, 4, 20, 21, 4, 14);
        seg(self, 3, 3, 36, 18, 4);
        seg(self, 2, 0, 21, 4, 14);
        seg(self, 1, 0, 3, 4, 14);
        seg(self, 0, 3, 18, 18, 4);
    }

    pub(super) fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Pixel) {
        let x1 = (x + w).min(Self::W);
        let y1 = (y + h).min(Self::H);
        for yy in y..y1 {
            let row = yy * Self::W;
            for xx in x..x1 {
                self.pixels[row + xx] = color;
            }
        }
    }

    pub(super) fn stroke_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Pixel) {
        if w == 0 || h == 0 {
            return;
        }
        self.fill_rect(x, y, w, 1, color);
        self.fill_rect(x, y + h - 1, w, 1, color);
        self.fill_rect(x, y, 1, h, color);
        self.fill_rect(x + w - 1, y, 1, h, color);
    }
}

pub(super) struct EffectLabelOverlay {
    font: ConsoleFont,
    pixels: Vec<Pixel>,
}

impl EffectLabelOverlay {
    const X: usize = 10;
    const Y: usize = 10;
    const W: usize = 260;
    const H: usize = 26;

    pub(super) fn new() -> Self {
        Self {
            font: ConsoleFont::new(10.0),
            pixels: vec![Pixel(0); Self::W * Self::H],
        }
    }

    pub(super) fn draw(
        &mut self,
        target: &mut UiFrameTarget,
        ui: &UiDisplay,
        effect: &str,
    ) -> DirtyRect {
        self.pixels.fill(Pixel::from_rgb(0, 0, 0));
        self.fill_rect(1, 1, Self::W - 2, Self::H - 2, Pixel::from_rgb(10, 14, 20));
        self.stroke_rect(0, 0, Self::W, Self::H, Pixel::from_rgb(69, 229, 255));
        self.font.draw_text_clipped(
            &mut self.pixels,
            Self::W,
            Self::W,
            0,
            Self::H,
            8,
            18,
            &format!("EFFECT: {}", effect.to_ascii_uppercase()),
            Pixel::from_rgb(255, 244, 126),
        );
        target.blit_pixel_rect(ui, Self::X, Self::Y, Self::W, Self::H, &self.pixels)
    }

    pub(super) fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Pixel) {
        let x1 = (x + w).min(Self::W);
        let y1 = (y + h).min(Self::H);
        for yy in y..y1 {
            let row = yy * Self::W;
            for xx in x..x1 {
                self.pixels[row + xx] = color;
            }
        }
    }

    pub(super) fn stroke_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: Pixel) {
        if w == 0 || h == 0 {
            return;
        }
        self.fill_rect(x, y, w, 1, color);
        self.fill_rect(x, y + h - 1, w, 1, color);
        self.fill_rect(x, y, 1, h, color);
        self.fill_rect(x + w - 1, y, 1, h, color);
    }
}

pub(crate) enum UiFrameTarget {
    Rgb565 { cached: Vec<Rgb565Pixel> },
}

impl UiFrameTarget {
    pub(super) fn cached(ui: &UiDisplay) -> Self {
        Self::Rgb565 {
            cached: vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()],
        }
    }

    pub(super) fn open(ui: &UiDisplay) -> Self {
        println!(
            "slint-render-target=cached fb-format={}",
            FramebufferFormat::production().label()
        );
        Self::cached(ui)
    }

    pub(super) fn render(&mut self, renderer: &SoftwareRenderer, ui: &UiDisplay) -> PhysicalRegion {
        let Self::Rgb565 { cached } = self;
        renderer.render(cached, ui.render_w())
    }

    #[cfg(mister_bench_scenes)]
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Rgb565 { .. } => "cached-565",
        }
    }

    pub(super) fn present_rect(
        &mut self,
        f: &mut Fpga,
        disp: &mut Display,
        ui: &UiDisplay,
        rect: DirtyRect,
    ) -> u32 {
        let _ = f;
        let Self::Rgb565 { cached } = self;
        copy_cached_rect_565(disp, ui, cached, rect);
        rect.rows()
    }

    pub(crate) fn copy_rect_from_565(
        &mut self,
        disp: &mut Display,
        ui: &UiDisplay,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Rgb565Pixel],
    ) {
        let _ = ui;
        let Self::Rgb565 { .. } = self;
        disp.copy_rect_from_565(x, y, w, h, src);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn copy_rect_from_565_strided(
        &mut self,
        disp: &mut Display,
        ui: &UiDisplay,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Rgb565Pixel],
        src_stride: usize,
        src_x: usize,
        src_y: usize,
    ) {
        let _ = ui;
        let Self::Rgb565 { .. } = self;
        disp.copy_rect_from_565_strided(x, y, w, h, src, src_stride, src_x, src_y);
    }

    pub(super) fn blit_raw_preview(
        &mut self,
        ui: &UiDisplay,
        frame: &PreviewRawFrame<'_>,
        clear_screen: bool,
    ) -> Option<DirtyRect> {
        let Self::Rgb565 { cached } = self;
        Raw565PreviewRenderer::compose_frame(cached, ui, frame, clear_screen)
    }

    pub(super) fn blit_raw_preview_transition(
        &mut self,
        ui: &UiDisplay,
        frame: &PreviewRawTransitionFrame<'_>,
        effect: PreviewTransitionEffect,
        progress: f32,
    ) -> DirtyRect {
        let Self::Rgb565 { cached } = self;
        Raw565PreviewRenderer::compose_transition(cached, ui, frame, effect, progress)
    }

    pub(super) fn blit_pixel_rect(
        &mut self,
        ui: &UiDisplay,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Pixel],
    ) -> DirtyRect {
        let w = w.min(ui.render_w().saturating_sub(x));
        let h = h.min(ui.render_h().saturating_sub(y));
        let Self::Rgb565 { cached } = self;
        for yy in 0..h {
            let dst = (y + yy) * ui.render_w() + x;
            let src_idx = yy * w;
            for xx in 0..w {
                let rgb = pixel_to_rgb(src[src_idx + xx]);
                cached[dst + xx] =
                    <Rgb565Pixel as TargetPixel>::from_rgb(rgb.0, rgb.1, rgb.2);
            }
        }
        DirtyRect {
            x0: x,
            y0: y,
            x1: x + w,
            y1: y + h,
        }
    }

    pub(super) fn present_rows(
        &mut self,
        f: &mut Fpga,
        disp: &mut Display,
        ui: &UiDisplay,
        y0: usize,
        y1: usize,
    ) -> u32 {
        let _ = f;
        let Self::Rgb565 { cached } = self;
        copy_cached_rows_565(disp, ui, cached, y0, y1);
        y1.saturating_sub(y0) as u32
    }
}

pub(super) fn blit_raw_preview_if_needed(
    target: &mut UiFrameTarget,
    ui: &UiDisplay,
    preview: &mut PreviewState,
    transition: &mut PreviewTransitionDemo,
    elapsed: Duration,
    slint_dirty: Option<DirtyRect>,
) -> (Option<DirtyRect>, PreviewTransitionTrace) {
    let raw_dirty = preview.take_raw_dirty();
    let slint_touched_preview = slint_dirty
        .and_then(|rect| rect.intersection(preview_screen_rect(ui)))
        .is_some();
    let transition_frame = preview.raw_transition_frame();
    let trace = transition.update(transition_frame.as_ref(), elapsed);
    if !raw_dirty && !slint_touched_preview && !trace.active {
        preview.finish_raw_empty_transition_if_idle();
        return (None, trace);
    }
    let Some(transition_frame) = transition_frame else {
        return (None, trace);
    };
    let raw_rect = if trace.active {
        target.blit_raw_preview_transition(ui, &transition_frame, trace.effect, trace.progress)
    } else {
        let Some(raw_rect) = target.blit_raw_preview(ui, &transition_frame.current, raw_dirty)
        else {
            return (None, trace);
        };
        raw_rect
    };
    if slint_dirty.is_some_and(|rect| rect.contains(raw_rect)) {
        (None, trace)
    } else {
        (Some(raw_rect), trace)
    }
}

pub(super) fn copy_arcade_list_update(
    target: &mut UiFrameTarget,
    disp: &mut Display,
    ui: &UiDisplay,
    renderer: &mut ArcadeListRenderer,
    update: ArcadeListUpdate,
) -> u32 {
    match update {
        ArcadeListUpdate::Full(rect) => {
            renderer.copy_layer_to_target(target, disp, ui, true);
            rect.rows()
        }
        ArcadeListUpdate::Scroll { .. } => {
            // `Scroll` means the renderer reused its cached RAM surface. A
            // prior live-framebuffer scroll-present path was visually correct
            // but roughly doubled present cost because `/dev/fb0` reads are
            // expensive on the MiSTer write-combined framebuffer.
            renderer.copy_layer_to_target(target, disp, ui, false);
            ArcadeListRenderer::dirty_rect().rows()
        }
    }
}

pub(super) fn arcade_update_dirty_rect(update: &ArcadeListUpdate) -> DirtyRect {
    match update {
        ArcadeListUpdate::Full(rect) => *rect,
        ArcadeListUpdate::Scroll { .. } => ArcadeListRenderer::dirty_rect(),
    }
}

pub(super) fn arcade_list_needs_forced_redraw(
    slint_dirty: Option<DirtyRect>,
    full_frame_present: bool,
) -> bool {
    full_frame_present
        || slint_dirty.is_some_and(|rect| {
            rect.intersection(ArcadeListRenderer::dirty_rect())
                .is_some()
        })
}

#[cfg(mister_bench_scenes)]
pub(super) fn frame_rect(rect: DirtyRect) -> FrameRect {
    FrameRect {
        x0: rect.x0 as u32,
        y0: rect.y0 as u32,
        x1: rect.x1 as u32,
        y1: rect.y1 as u32,
    }
}

pub(super) fn configure_window(ui: &UiDisplay, window: &Rc<MinimalSoftwareWindow>) {
    window.set_size(PhysicalSize::new(
        ui.render_w() as u32,
        ui.render_h() as u32,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_display::{UI_FB_H, UI_FB_W};

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
    fn catalog_refresh_policy_parses_force_off_and_default() {
        assert_eq!(
            catalog_refresh_policy_from_value(None),
            CatalogRefreshPolicy::Default
        );
        assert_eq!(
            catalog_refresh_policy_from_value(Some("on")),
            CatalogRefreshPolicy::Force
        );
        assert_eq!(
            catalog_refresh_policy_from_value(Some("force")),
            CatalogRefreshPolicy::Force
        );
        assert_eq!(
            catalog_refresh_policy_from_value(Some("off")),
            CatalogRefreshPolicy::Off
        );
        assert_eq!(
            catalog_refresh_policy_from_value(Some("load-only")),
            CatalogRefreshPolicy::Off
        );
        assert_eq!(
            catalog_refresh_policy_from_value(Some("later")),
            CatalogRefreshPolicy::Default
        );
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
    fn arcade_list_overlay_redraws_when_full_frame_present_overwrites_stationary_text() {
        assert!(arcade_list_needs_forced_redraw(None, true));
    }

    #[test]
    fn arcade_list_overlay_redraws_when_slint_dirty_touches_list() {
        let rect = ArcadeListRenderer::dirty_rect();

        assert!(arcade_list_needs_forced_redraw(Some(rect), false));
    }

    #[test]
    fn arcade_list_overlay_stays_idle_for_unrelated_slint_dirty_rect() {
        let rect = DirtyRect {
            x0: ARCADE_LIST_X + ARCADE_LIST_W + 1,
            y0: ARCADE_LIST_Y,
            x1: ARCADE_LIST_X + ARCADE_LIST_W + 20,
            y1: ARCADE_LIST_Y + 20,
        };

        assert!(!arcade_list_needs_forced_redraw(Some(rect), false));
    }

    #[test]
    fn empty_raw_preview_blit_clears_preview_screen() {
        let ui = UiDisplay::for_framebuffer(UI_FB_W, UI_FB_H);
        let mut target = UiFrameTarget::cached(&ui);
        let frame = PreviewRawFrame {
            pixels: PreviewRawPixels::Empty,
            source_w: 1,
            source_h: 1,
            display_w: ARCADE_PREVIEW_BOX_W,
            display_h: ARCADE_PREVIEW_BOX_H,
        };

        let UiFrameTarget::Rgb565 { cached } = &mut target;
        cached.fill(<Rgb565Pixel as TargetPixel>::from_rgb(0, 255, 0));

        let rect = target
            .blit_raw_preview(&ui, &frame, true)
            .expect("empty preview rect");
        let screen = preview_screen_rect(&ui);

        assert_eq!(rect.x0, screen.x0);
        assert_eq!(rect.y0, screen.y0);
        assert_eq!(rect.x1, screen.x1);
        assert_eq!(rect.y1, screen.y1);
        let UiFrameTarget::Rgb565 { cached } = target;
        let center = ((screen.y0 + screen.y1) / 2) * ui.render_w() + (screen.x0 + screen.x1) / 2;
        assert_eq!(cached[center].0, 0);
    }
}
