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

pub(super) fn catalog_refresh_requested() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| {
        matches!(
            std::env::var("MISTER_CATALOG_REFRESH").as_deref(),
            Ok("1") | Ok("on") | Ok("true") | Ok("yes")
        )
    })
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

pub(super) fn preview_screen_rect(ui: &UiDisplay) -> DirtyRect {
    const CABINET_W: usize = 336;
    const CABINET_H: usize = 520;
    let right_x = ui.render_w() / 2;
    let right_w = ui.render_w().saturating_sub(right_x);
    let cabinet_x = right_x + right_w.saturating_sub(CABINET_W) / 2;
    let cabinet_y = ui.render_h().saturating_sub(CABINET_H) / 2;
    DirtyRect {
        x0: cabinet_x + ARCADE_PREVIEW_BOX_X,
        y0: cabinet_y + ARCADE_PREVIEW_BOX_Y,
        x1: cabinet_x + ARCADE_PREVIEW_BOX_X + ARCADE_PREVIEW_BOX_W as usize,
        y1: cabinet_y + ARCADE_PREVIEW_BOX_Y + ARCADE_PREVIEW_BOX_H as usize,
    }
}

pub(super) fn rgb565_to_pixel(pixel: Rgb565Pixel) -> Pixel {
    let v = pixel.0;
    let r5 = (v >> 11) & 0x1f;
    let g6 = (v >> 5) & 0x3f;
    let b5 = v & 0x1f;
    let r = ((r5 << 3) | (r5 >> 2)) as u32;
    let g = ((g6 << 2) | (g6 >> 4)) as u32;
    let b = ((b5 << 3) | (b5 >> 2)) as u32;
    Pixel((r << 16) | (g << 8) | b)
}

pub(super) fn pixel_to_rgb(pixel: Pixel) -> (u8, u8, u8) {
    (
        ((pixel.0 >> 16) & 0xff) as u8,
        ((pixel.0 >> 8) & 0xff) as u8,
        (pixel.0 & 0xff) as u8,
    )
}

pub(super) fn rgb565_to_rgb(pixel: Rgb565Pixel) -> (u8, u8, u8) {
    pixel_to_rgb(rgb565_to_pixel(pixel))
}

pub(super) fn raw_preview_scaled_rect(
    ui: &UiDisplay,
    frame: &PreviewRawFrame<'_>,
) -> Option<DirtyRect> {
    if matches!(frame.pixels, PreviewRawPixels::Empty) {
        return Some(preview_screen_rect(ui));
    }
    if frame.source_w == 0 || frame.source_h == 0 || frame.display_w == 0 || frame.display_h == 0 {
        return None;
    }
    match frame.pixels {
        PreviewRawPixels::Rgb8(rgb)
            if rgb.len() < frame.source_w as usize * frame.source_h as usize * 3 =>
        {
            return None;
        }
        PreviewRawPixels::Rgb565 {
            pixels,
            stride_pixels,
        } if stride_pixels < frame.source_w as usize
            || pixels.len() < stride_pixels * frame.source_h as usize =>
        {
            return None;
        }
        _ => {}
    }

    let screen = preview_screen_rect(ui);
    let image_x =
        screen.x0 as isize + (ARCADE_PREVIEW_BOX_W as isize - frame.display_w as isize) / 2;
    let image_y =
        screen.y0 as isize + (ARCADE_PREVIEW_BOX_H as isize - frame.display_h as isize) / 2;
    let x0 = screen.x0.max(image_x.max(0) as usize);
    let y0 = screen.y0.max(image_y.max(0) as usize);
    let x1 = screen
        .x1
        .min((image_x + frame.display_w as isize).max(0) as usize)
        .min(ui.render_w());
    let y1 = screen
        .y1
        .min((image_y + frame.display_h as isize).max(0) as usize)
        .min(ui.render_h());

    (x1 > x0 && y1 > y0).then_some(DirtyRect { x0, y0, x1, y1 })
}

pub(super) fn sample_preview_rgb(
    frame: &PreviewRawFrame<'_>,
    screen: DirtyRect,
    x: usize,
    y: usize,
    offset_x: isize,
    scale_num: u32,
    scale_den: u32,
) -> Option<(u8, u8, u8)> {
    if matches!(frame.pixels, PreviewRawPixels::Empty) {
        return Some((0, 0, 0));
    }
    if frame.source_w == 0
        || frame.source_h == 0
        || frame.display_w == 0
        || frame.display_h == 0
        || scale_den == 0
    {
        return None;
    }
    if scale_num == scale_den
        && frame.display_w == frame.source_w
        && frame.display_h == frame.source_h
    {
        let image_x = screen.x0 as isize
            + (ARCADE_PREVIEW_BOX_W as isize - frame.source_w as isize) / 2
            + offset_x;
        let image_y =
            screen.y0 as isize + (ARCADE_PREVIEW_BOX_H as isize - frame.source_h as isize) / 2;
        let src_x = x as isize - image_x;
        let src_y = y as isize - image_y;
        if src_x < 0
            || src_y < 0
            || src_x >= frame.source_w as isize
            || src_y >= frame.source_h as isize
        {
            return None;
        }
        let src_x = src_x as usize;
        let src_y = src_y as usize;
        return match frame.pixels {
            PreviewRawPixels::Empty => Some((0, 0, 0)),
            PreviewRawPixels::Rgb8(rgb) => {
                let si = (src_y * frame.source_w as usize + src_x) * 3;
                (si + 2 < rgb.len()).then(|| (rgb[si], rgb[si + 1], rgb[si + 2]))
            }
            PreviewRawPixels::Rgb565 {
                pixels,
                stride_pixels,
            } => {
                let idx = src_y * stride_pixels + src_x;
                (idx < pixels.len()).then(|| rgb565_to_rgb(pixels[idx]))
            }
        };
    }
    let scaled_w = ((frame.display_w as u64 * scale_num as u64) / scale_den as u64)
        .max(1)
        .min(isize::MAX as u64) as isize;
    let scaled_h = ((frame.display_h as u64 * scale_num as u64) / scale_den as u64)
        .max(1)
        .min(isize::MAX as u64) as isize;
    let center_x = screen.x0 as isize + ARCADE_PREVIEW_BOX_W as isize / 2 + offset_x;
    let center_y = screen.y0 as isize + ARCADE_PREVIEW_BOX_H as isize / 2;
    let image_x = center_x - scaled_w / 2;
    let image_y = center_y - scaled_h / 2;
    let local_x = x as isize - image_x;
    let local_y = y as isize - image_y;
    if local_x < 0 || local_y < 0 || local_x >= scaled_w || local_y >= scaled_h {
        return None;
    }
    let src_w = frame.source_w as usize;
    let src_h = frame.source_h as usize;
    let src_x = ((local_x as u64 * frame.source_w as u64) / scaled_w as u64)
        .min(frame.source_w.saturating_sub(1) as u64) as usize;
    let src_y = ((local_y as u64 * frame.source_h as u64) / scaled_h as u64)
        .min(frame.source_h.saturating_sub(1) as u64) as usize;
    match frame.pixels {
        PreviewRawPixels::Empty => Some((0, 0, 0)),
        PreviewRawPixels::Rgb8(rgb) => {
            let si = (src_y * src_w + src_x) * 3;
            (si + 2 < rgb.len()).then(|| (rgb[si], rgb[si + 1], rgb[si + 2]))
        }
        PreviewRawPixels::Rgb565 {
            pixels,
            stride_pixels,
        } => {
            if src_x >= src_w || src_y >= src_h || src_y * stride_pixels + src_x >= pixels.len() {
                None
            } else {
                Some(rgb565_to_rgb(pixels[src_y * stride_pixels + src_x]))
            }
        }
    }
}

pub(super) fn blend_rgb(from: (u8, u8, u8), to: (u8, u8, u8), alpha: u8) -> (u8, u8, u8) {
    let a = alpha as u16;
    let ia = 255u16.saturating_sub(a);
    (
        ((from.0 as u16 * ia + to.0 as u16 * a) / 255) as u8,
        ((from.1 as u16 * ia + to.1 as u16 * a) / 255) as u8,
        ((from.2 as u16 * ia + to.2 as u16 * a) / 255) as u8,
    )
}

#[cfg(mister_bench_scenes)]
pub(super) fn brighten_rgb(rgb: (u8, u8, u8), add: u8) -> (u8, u8, u8) {
    (
        rgb.0.saturating_add(add),
        rgb.1.saturating_add(add),
        rgb.2.saturating_add(add),
    )
}

pub(super) fn hash2_u8(x: usize, y: usize) -> u8 {
    let mut v = (x as u32).wrapping_mul(0x45d9f3b) ^ (y as u32).wrapping_mul(0x119de1f3);
    v ^= v >> 16;
    v = v.wrapping_mul(0x45d9f3b);
    (v >> 24) as u8
}

pub(super) struct Raw565View<'a> {
    pixels: &'a [Rgb565Pixel],
    stride_pixels: usize,
    w: usize,
    h: usize,
    x: isize,
    y: isize,
}

pub(super) fn raw565_view<'a>(
    frame: &'a PreviewRawFrame<'a>,
    screen: DirtyRect,
    offset_x: isize,
) -> Option<Raw565View<'a>> {
    if frame.display_w != frame.source_w || frame.display_h != frame.source_h {
        return None;
    }
    let PreviewRawPixels::Rgb565 {
        pixels,
        stride_pixels,
    } = frame.pixels
    else {
        return None;
    };
    let w = frame.source_w as usize;
    let h = frame.source_h as usize;
    if w == 0 || h == 0 || stride_pixels < w || pixels.len() < stride_pixels * h {
        return None;
    }
    Some(Raw565View {
        pixels,
        stride_pixels,
        w,
        h,
        x: screen.x0 as isize + (ARCADE_PREVIEW_BOX_W as isize - w as isize) / 2 + offset_x,
        y: screen.y0 as isize + (ARCADE_PREVIEW_BOX_H as isize - h as isize) / 2,
    })
}

pub(super) fn sample_raw565(view: &Raw565View<'_>, x: usize, y: usize) -> Option<Rgb565Pixel> {
    let sx = x as isize - view.x;
    let sy = y as isize - view.y;
    if sx < 0 || sy < 0 || sx >= view.w as isize || sy >= view.h as isize {
        None
    } else {
        Some(view.pixels[sy as usize * view.stride_pixels + sx as usize])
    }
}

fn raw565_row_for_screen_y<'a>(view: &'a Raw565View<'a>, y: usize) -> Option<&'a [Rgb565Pixel]> {
    let sy = y as isize - view.y;
    if sy < 0 || sy >= view.h as isize {
        None
    } else {
        let start = sy as usize * view.stride_pixels;
        Some(&view.pixels[start..start + view.w])
    }
}

fn raw565_row_pixel_or(
    row: Option<&[Rgb565Pixel]>,
    view: Option<&Raw565View<'_>>,
    x: usize,
    fallback: Rgb565Pixel,
) -> Rgb565Pixel {
    let Some((row, view)) = row.zip(view) else {
        return fallback;
    };
    let sx = x as isize - view.x;
    if sx < 0 || sx >= view.w as isize {
        fallback
    } else {
        row[sx as usize]
    }
}

pub(super) fn blend_565(from: Rgb565Pixel, to: Rgb565Pixel, alpha: u8) -> Rgb565Pixel {
    let a = alpha as u32;
    let ia = 255u32.saturating_sub(a);
    let f = from.0 as u32;
    let t = to.0 as u32;
    let fr = (f >> 11) & 0x1f;
    let fg = (f >> 5) & 0x3f;
    let fb = f & 0x1f;
    let tr = (t >> 11) & 0x1f;
    let tg = (t >> 5) & 0x3f;
    let tb = t & 0x1f;
    let r = (fr * ia + tr * a) / 255;
    let g = (fg * ia + tg * a) / 255;
    let b = (fb * ia + tb * a) / 255;
    Rgb565Pixel(((r << 11) | (g << 5) | b) as u16)
}

#[cfg(mister_bench_scenes)]
pub(super) fn darken_565(pixel: Rgb565Pixel) -> Rgb565Pixel {
    let v = pixel.0 as u32;
    let r = (((v >> 11) & 0x1f) * 5) / 8;
    let g = (((v >> 5) & 0x3f) * 5) / 8;
    let b = ((v & 0x1f) * 5) / 8;
    Rgb565Pixel(((r << 11) | (g << 5) | b) as u16)
}

pub(super) fn brighten_565(pixel: Rgb565Pixel) -> Rgb565Pixel {
    let v = pixel.0 as u32;
    let r = ((v >> 11) & 0x1f).saturating_add(8).min(0x1f);
    let g = ((v >> 5) & 0x3f).saturating_add(16).min(0x3f);
    let b = (v & 0x1f).saturating_add(8).min(0x1f);
    Rgb565Pixel(((r << 11) | (g << 5) | b) as u16)
}

#[cfg(mister_bench_scenes)]
pub(super) fn mosaic_block_size(progress: f32) -> usize {
    let p = progress.clamp(0.0, 1.0);
    if p >= 0.96 {
        1
    } else if p >= 0.78 {
        2
    } else if p >= 0.58 {
        4
    } else if p >= 0.38 {
        8
    } else if p >= 0.18 {
        16
    } else {
        32
    }
}

#[cfg(mister_bench_scenes)]
fn progress_u8(progress: f32) -> u8 {
    (progress.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub(super) fn triangle_wave_u8(x: usize, phase: u8) -> u8 {
    let v = ((x as u32).wrapping_mul(13).wrapping_add(phase as u32)) & 0xff;
    let v = if v < 128 { v } else { 255 - v };
    (v * 2).min(255) as u8
}

pub(super) fn plasma_gate(x: usize, y: usize, phase: u8) -> u8 {
    let a = triangle_wave_u8(x / 3 + y / 7, phase);
    let b = triangle_wave_u8(x / 9 + y / 2, phase.wrapping_mul(3));
    ((a as u16 + b as u16) / 2) as u8
}

#[cfg(mister_bench_scenes)]
fn dist2_from_center(local_x: usize, local_y: usize, w: usize, h: usize) -> u64 {
    let cx = w as i64 / 2;
    let cy = h as i64 / 2;
    let dx = local_x as i64 - cx;
    let dy = local_y as i64 - cy;
    (dx * dx + dy * dy) as u64
}

#[cfg(mister_bench_scenes)]
fn angle_byte(local_x: usize, local_y: usize, w: usize, h: usize) -> u8 {
    let cx = w as isize / 2;
    let cy = h as isize / 2;
    let dx = local_x as isize - cx;
    let dy = local_y as isize - cy;
    let ax = dx.unsigned_abs();
    let ay = dy.unsigned_abs();
    let denom = (ax + ay).max(1);
    let turn = ((ay * 64) / denom).min(64) as u8;
    match (dx >= 0, dy >= 0) {
        (true, true) => 128u8.saturating_add(turn),
        (true, false) => 128u8.saturating_sub(turn),
        (false, true) => 255u8.saturating_sub(turn),
        (false, false) => turn,
    }
}

#[cfg(mister_bench_scenes)]
fn transition_gate(
    effect: PreviewTransitionEffect,
    progress: f32,
    local_x: usize,
    local_y: usize,
    w: usize,
    h: usize,
) -> u8 {
    let alpha = progress_u8(progress);
    let reveal_w = ((w as f32) * progress).round() as usize;
    let reveal_h = ((h as f32) * progress).round() as usize;
    match effect {
        PreviewTransitionEffect::Cut => 255,
        PreviewTransitionEffect::Fade => alpha,
        PreviewTransitionEffect::Wipe => {
            if local_x < reveal_w {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::Zoom => {
            let cx = w / 2;
            let cy = h / 2;
            if local_x.abs_diff(cx) <= reveal_w / 2 && local_y.abs_diff(cy) <= reveal_h / 2 {
                alpha
            } else {
                0
            }
        }
        PreviewTransitionEffect::Scanline => {
            if local_y < reveal_h {
                alpha
            } else {
                0
            }
        }
        PreviewTransitionEffect::Checker => {
            if hash2_u8(local_x / 16, local_y / 16) <= alpha {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::Dissolve => {
            if hash2_u8(local_x / 2, local_y / 2) <= alpha {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::CrtBeamWipe => {
            let beam_y = (progress * (h as f32 + 4.0)).round() as isize - 2;
            let dy = local_y as isize - beam_y;
            if dy <= 0 {
                255
            } else if dy <= 10 {
                220u8.saturating_sub((dy as u8) * 18)
            } else {
                0
            }
        }
        PreviewTransitionEffect::MosaicResolve => alpha,
        PreviewTransitionEffect::CopperBars => {
            let bar = ((local_y / 10 + progress_u8(progress) as usize / 7) & 7) as u8;
            if local_x < reveal_w || bar <= (alpha >> 5) {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::VenetianBlinds => {
            let open = ((16.0 * progress).round() as usize).min(16);
            if local_x % 16 < open {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::BarnDoor => {
            let half = (w as f32 * progress / 2.0).round() as usize;
            let cx = w / 2;
            if local_x >= cx.saturating_sub(half) && local_x <= (cx + half).min(w) {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::Iris => {
            let max_r2 = ((w * w + h * h) / 4) as u64;
            if dist2_from_center(local_x, local_y, w, h)
                <= (max_r2 as f32 * progress * progress) as u64
            {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::ClockWipe => {
            if angle_byte(local_x, local_y, w, h) <= alpha {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::SpriteStrips => {
            let strip = local_y / 24;
            let skew = (strip * 19) % w.max(1);
            if (local_x + skew) % w.max(1) < reveal_w {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::StarfieldWarp => {
            let d = dist2_from_center(local_x, local_y, w, h);
            let noise = hash2_u8(local_x / 4, local_y / 4) as u64;
            let max_r2 = ((w * w + h * h) / 4) as u64;
            if d.saturating_add(noise * 48) <= (max_r2 as f32 * progress * progress) as u64 {
                255
            } else if noise as u8 > 244u8.saturating_sub(alpha / 8) {
                192
            } else {
                0
            }
        }
        PreviewTransitionEffect::VectorRedraw => {
            let diag = local_x + local_y;
            if diag < ((w + h) as f32 * progress).round() as usize || local_x % 37 == local_y % 29 {
                alpha
            } else {
                0
            }
        }
        PreviewTransitionEffect::PaletteCycle => {
            if ((local_x / 12 + local_y / 12 + alpha as usize / 16) & 3) == 0 {
                alpha / 2
            } else {
                alpha
            }
        }
        PreviewTransitionEffect::RasterTear => {
            let tear = ((local_y * 11 + alpha as usize * 3) & 63) as isize - 32;
            if (local_x as isize + tear).unsigned_abs() % w.max(1) < reveal_w {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::TileLoader => {
            if hash2_u8(local_x / 24, local_y / 24) <= alpha {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::VenetianCopper => {
            let open = ((20.0 * progress).round() as usize).min(20);
            if local_x % 20 < open || ((local_y / 9 + alpha as usize / 18) & 3) == 0 {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::AttributeFlash => {
            if hash2_u8(local_x / 8, local_y / 8) <= alpha {
                255
            } else {
                alpha / 3
            }
        }
        PreviewTransitionEffect::TecTec => {
            let wave = triangle_wave_u8(local_y / 2, alpha);
            if local_x.saturating_add(wave as usize / 2) < reveal_w + w / 8 {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::Linecrunch => {
            let band = ((local_y as f32 - h as f32 / 2.0).abs() * (1.0 - progress)) as usize;
            if band < h / 2 || local_y < reveal_h {
                alpha
            } else {
                0
            }
        }
        PreviewTransitionEffect::RacingBeam => {
            let beam = reveal_w as isize;
            let dx = local_x as isize - beam;
            if dx <= 0 {
                255
            } else if dx < 28 {
                255u8.saturating_sub((dx as u8) * 8)
            } else {
                0
            }
        }
        PreviewTransitionEffect::SpriteMultiplex => {
            if hash2_u8(local_x / 32, (local_y + alpha as usize) / 20) <= alpha {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::RowScrollParallax => {
            let row_phase = ((local_y / 12) * 17) % w.max(1);
            if (local_x + row_phase) % w.max(1) < reveal_w {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::SuperScalerPop => {
            let d = dist2_from_center(local_x, local_y, w, h);
            let r = (w.min(h) as f32 * (0.08 + progress * 0.92)) as u64;
            if d <= r * r {
                alpha
            } else {
                0
            }
        }
        PreviewTransitionEffect::MaskBlit => {
            let mask = ((local_x ^ local_y) + (local_x / 7) + (local_y / 11)) & 255;
            if mask <= alpha as usize {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::PhosphorDecay => {
            if local_y < reveal_h {
                255
            } else {
                alpha / 2
            }
        }
        PreviewTransitionEffect::PlasmaMask => {
            if plasma_gate(local_x, local_y, alpha) <= alpha {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::MoireRings => {
            let ring = ((dist2_from_center(local_x, local_y, w, h) / 96) & 255) as u8;
            if ring <= alpha || ring.abs_diff(alpha) < 12 {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::KefrensCurtain => {
            let wave = triangle_wave_u8(local_y / 3, alpha) as usize;
            if local_x < reveal_w.saturating_add(wave / 3) {
                255
            } else {
                0
            }
        }
        PreviewTransitionEffect::Slide => 255,
    }
}

fn blit_transition_565_cut(
    cached: &mut [Rgb565Pixel],
    ui: &UiDisplay,
    screen: DirtyRect,
    frame: &PreviewRawTransitionFrame<'_>,
) -> Option<()> {
    if matches!(frame.current.pixels, PreviewRawPixels::Empty) {
        let black = <Rgb565Pixel as TargetPixel>::from_rgb(0, 0, 0);
        for y in screen.y0..screen.y1.min(ui.render_h()) {
            let row = y * ui.render_w();
            for x in screen.x0..screen.x1.min(ui.render_w()) {
                cached[row + x] = black;
            }
        }
        return Some(());
    }
    let current = raw565_view(&frame.current, screen, 0)?;
    let black = <Rgb565Pixel as TargetPixel>::from_rgb(0, 0, 0);
    for y in screen.y0..screen.y1.min(ui.render_h()) {
        let row = y * ui.render_w();
        for x in screen.x0..screen.x1.min(ui.render_w()) {
            cached[row + x] = sample_raw565(&current, x, y).unwrap_or(black);
        }
    }
    Some(())
}

fn blit_transition_565_fade(
    cached: &mut [Rgb565Pixel],
    ui: &UiDisplay,
    screen: DirtyRect,
    frame: &PreviewRawTransitionFrame<'_>,
    progress: f32,
) -> Option<()> {
    let current_empty = matches!(frame.current.pixels, PreviewRawPixels::Empty);
    let current = if current_empty {
        None
    } else {
        Some(raw565_view(&frame.current, screen, 0)?)
    };
    let previous = frame
        .previous
        .as_ref()
        .and_then(|prev| raw565_view(prev, screen, 0));
    let alpha = (progress.clamp(0.0, 1.0) * 255.0).round() as u8;
    let black = <Rgb565Pixel as TargetPixel>::from_rgb(0, 0, 0);
    for y in screen.y0..screen.y1.min(ui.render_h()) {
        let row = y * ui.render_w();
        let previous_row = previous
            .as_ref()
            .and_then(|view| raw565_row_for_screen_y(view, y));
        let current_row = current
            .as_ref()
            .and_then(|view| raw565_row_for_screen_y(view, y));
        for x in screen.x0..screen.x1.min(ui.render_w()) {
            let prev = raw565_row_pixel_or(previous_row, previous.as_ref(), x, black);
            let curr = raw565_row_pixel_or(current_row, current.as_ref(), x, black);
            cached[row + x] = blend_565(prev, curr, alpha);
        }
    }
    Some(())
}

fn blit_transition_565_via_rgb(
    cached: &mut [Rgb565Pixel],
    ui: &UiDisplay,
    screen: DirtyRect,
    frame: &PreviewRawTransitionFrame<'_>,
    effect: PreviewTransitionEffect,
    progress: f32,
) {
    let alpha = (progress.clamp(0.0, 1.0) * 255.0).round() as u8;
    for y in screen.y0..screen.y1.min(ui.render_h()) {
        let row = y * ui.render_w();
        for x in screen.x0..screen.x1.min(ui.render_w()) {
            let current = sample_preview_rgb(&frame.current, screen, x, y, 0, 1024, 1024)
                .unwrap_or((0, 0, 0));
            let rgb = match effect {
                PreviewTransitionEffect::Cut => current,
                PreviewTransitionEffect::Fade => {
                    let prev = frame
                        .previous
                        .as_ref()
                        .and_then(|prev| sample_preview_rgb(prev, screen, x, y, 0, 1024, 1024))
                        .unwrap_or((0, 0, 0));
                    blend_rgb(prev, current, alpha)
                }
                #[cfg(mister_bench_scenes)]
                _ => unreachable!("non-production transitions use bench blitters"),
            };
            cached[row + x] = <Rgb565Pixel as TargetPixel>::from_rgb(rgb.0, rgb.1, rgb.2);
        }
    }
}

#[cfg(mister_bench_scenes)]
pub(super) fn blit_transition_565_fast(
    cached: &mut [Rgb565Pixel],
    ui: &UiDisplay,
    screen: DirtyRect,
    frame: &PreviewRawTransitionFrame<'_>,
    effect: PreviewTransitionEffect,
    progress: f32,
) -> Option<()> {
    let current = raw565_view(&frame.current, screen, 0)?;
    let alpha = (progress.clamp(0.0, 1.0) * 255.0).round() as u8;
    let black = <Rgb565Pixel as TargetPixel>::from_rgb(0, 0, 0);
    let prev_offset = -((progress * screen.width() as f32).round() as isize);
    let current_offset = ((1.0 - progress) * screen.width() as f32).round() as isize;
    let previous = frame
        .previous
        .as_ref()
        .and_then(|prev| raw565_view(prev, screen, 0));
    let slide_previous = frame
        .previous
        .as_ref()
        .and_then(|prev| raw565_view(prev, screen, prev_offset));
    let slide_current = raw565_view(&frame.current, screen, current_offset);
    let reveal_w = ((screen.width() as f32) * progress).round() as usize;
    let reveal_h = ((screen.rows() as f32) * progress).round() as usize;
    let cx = screen.width() / 2;
    let cy = screen.rows() as usize / 2;
    let zoom_w = reveal_w / 2;
    let zoom_h = reveal_h / 2;
    let iris_r2 = {
        let max_r2 = ((screen.width() * screen.width()
            + screen.rows() as usize * screen.rows() as usize)
            / 4) as u64;
        (max_r2 as f32 * progress * progress) as u64
    };

    for y in screen.y0..screen.y1.min(ui.render_h()) {
        let row = y * ui.render_w();
        let local_y = y - screen.y0;
        for x in screen.x0..screen.x1.min(ui.render_w()) {
            let local_x = x - screen.x0;
            let prev = previous
                .as_ref()
                .and_then(|view| sample_raw565(view, x, y))
                .unwrap_or(black);
            let curr = sample_raw565(&current, x, y).unwrap_or(black);
            cached[row + x] = match effect {
                PreviewTransitionEffect::Cut => curr,
                PreviewTransitionEffect::Fade => blend_565(prev, curr, alpha),
                PreviewTransitionEffect::Wipe => {
                    if local_x < reveal_w {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::Slide => slide_current
                    .as_ref()
                    .and_then(|view| sample_raw565(view, x, y))
                    .or_else(|| {
                        slide_previous
                            .as_ref()
                            .and_then(|view| sample_raw565(view, x, y))
                    })
                    .unwrap_or(black),
                PreviewTransitionEffect::Zoom => {
                    if local_x.abs_diff(cx) <= zoom_w && local_y.abs_diff(cy) <= zoom_h {
                        blend_565(prev, curr, alpha)
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::Scanline => {
                    if local_y < reveal_h {
                        let blended = blend_565(prev, curr, alpha);
                        if local_y & 3 == 0 {
                            darken_565(blended)
                        } else {
                            blended
                        }
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::BarnDoor => {
                    let half = reveal_w / 2;
                    if local_x >= cx.saturating_sub(half)
                        && local_x <= (cx + half).min(screen.width())
                    {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::VenetianBlinds => {
                    let open = ((16.0 * progress).round() as usize).min(16);
                    if local_x % 16 < open {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::Iris => {
                    if dist2_from_center(local_x, local_y, screen.width(), screen.rows() as usize)
                        <= iris_r2
                    {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::ClockWipe => {
                    if angle_byte(local_x, local_y, screen.width(), screen.rows() as usize) <= alpha
                    {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::SpriteStrips => {
                    let strip = local_y / 24;
                    let skew = (strip * 19) % screen.width().max(1);
                    if (local_x + skew) % screen.width().max(1) < reveal_w {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::RacingBeam => {
                    let beam = reveal_w as isize;
                    let dx = local_x as isize - beam;
                    let gate = if dx <= 0 {
                        255
                    } else if dx < 28 {
                        255u8.saturating_sub((dx as u8) * 8)
                    } else {
                        0
                    };
                    let base = if gate == 255 {
                        curr
                    } else if gate == 0 {
                        prev
                    } else {
                        blend_565(prev, curr, gate)
                    };
                    if gate > 0 && ((local_y + alpha as usize / 8) & 15 == 0 || gate > 220) {
                        brighten_565(base)
                    } else {
                        base
                    }
                }
                PreviewTransitionEffect::TecTec => {
                    let wave = triangle_wave_u8(local_y / 2, alpha);
                    if local_x.saturating_add(wave as usize / 2) < reveal_w + screen.width() / 8 {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::VenetianCopper => {
                    let open = ((20.0 * progress).round() as usize).min(20);
                    let gate =
                        local_x % 20 < open || ((local_y / 9 + alpha as usize / 18) & 3) == 0;
                    let base = if gate { curr } else { prev };
                    if gate && ((local_y + alpha as usize / 8) & 15 == 0 || alpha > 220) {
                        brighten_565(base)
                    } else {
                        base
                    }
                }
                PreviewTransitionEffect::CopperBars => {
                    let bar = ((local_y / 10 + alpha as usize / 7) & 7) as u8;
                    let gate = local_x < reveal_w || bar <= (alpha >> 5);
                    let base = if gate { curr } else { prev };
                    if gate && ((local_y + alpha as usize / 8) & 15 == 0 || alpha > 220) {
                        brighten_565(base)
                    } else {
                        base
                    }
                }
                PreviewTransitionEffect::Checker => {
                    if hash2_u8(local_x / 16, local_y / 16) <= alpha {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::Dissolve => {
                    if hash2_u8(local_x / 2, local_y / 2) <= alpha {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::TileLoader => {
                    if hash2_u8(local_x / 24, local_y / 24) <= alpha {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::MaskBlit => {
                    let mask = ((local_x ^ local_y) + (local_x / 7) + (local_y / 11)) & 255;
                    if mask <= alpha as usize {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::SpriteMultiplex => {
                    if hash2_u8(local_x / 32, (local_y + alpha as usize) / 20) <= alpha {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::RowScrollParallax => {
                    let row_phase = ((local_y / 12) * 17) % screen.width().max(1);
                    if (local_x + row_phase) % screen.width().max(1) < reveal_w {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::SuperScalerPop => {
                    let d =
                        dist2_from_center(local_x, local_y, screen.width(), screen.rows() as usize);
                    let r = (screen.width().min(screen.rows() as usize) as f32
                        * (0.08 + progress * 0.92)) as u64;
                    if d <= r * r {
                        blend_565(prev, curr, alpha)
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::StarfieldWarp => {
                    let d =
                        dist2_from_center(local_x, local_y, screen.width(), screen.rows() as usize);
                    let noise = hash2_u8(local_x / 4, local_y / 4) as u64;
                    let max_r2 = ((screen.width() * screen.width()
                        + screen.rows() as usize * screen.rows() as usize)
                        / 4) as u64;
                    if d.saturating_add(noise * 48) <= (max_r2 as f32 * progress * progress) as u64
                    {
                        curr
                    } else if noise as u8 > 244u8.saturating_sub(alpha / 8) {
                        brighten_565(prev)
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::VectorRedraw => {
                    let diag = local_x + local_y;
                    if diag
                        < ((screen.width() + screen.rows() as usize) as f32 * progress).round()
                            as usize
                        || local_x % 37 == local_y % 29
                    {
                        blend_565(prev, curr, alpha)
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::PaletteCycle => {
                    let gate = if ((local_x / 12 + local_y / 12 + alpha as usize / 16) & 3) == 0 {
                        alpha / 2
                    } else {
                        alpha
                    };
                    let base = blend_565(prev, curr, gate);
                    if ((local_x + local_y) & 7) == 0 {
                        brighten_565(base)
                    } else {
                        base
                    }
                }
                PreviewTransitionEffect::PlasmaMask => {
                    if plasma_gate(local_x, local_y, alpha) <= alpha {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::PhosphorDecay => {
                    if local_y < reveal_h {
                        curr
                    } else {
                        darken_565(blend_565(prev, curr, alpha / 2))
                    }
                }
                PreviewTransitionEffect::MoireRings => {
                    let ring = ((dist2_from_center(
                        local_x,
                        local_y,
                        screen.width(),
                        screen.rows() as usize,
                    ) / 96)
                        & 255) as u8;
                    if ring <= alpha || ring.abs_diff(alpha) < 12 {
                        curr
                    } else {
                        prev
                    }
                }
                PreviewTransitionEffect::CrtBeamWipe => {
                    let beam_y = (progress * (screen.rows() as f32 + 4.0)).round() as isize - 2;
                    let dy = local_y as isize - beam_y;
                    let base = if dy <= 0 {
                        curr
                    } else if dy <= 10 {
                        blend_565(prev, curr, 220u8.saturating_sub((dy as u8) * 18))
                    } else {
                        prev
                    };
                    if dy.abs() <= 2 {
                        brighten_565(base)
                    } else {
                        base
                    }
                }
                PreviewTransitionEffect::MosaicResolve => {
                    let block = mosaic_block_size(progress);
                    let sample_x = (screen.x0 + (local_x / block) * block + block / 2)
                        .min(screen.x1.saturating_sub(1));
                    let sample_y = (screen.y0 + (local_y / block) * block + block / 2)
                        .min(screen.y1.saturating_sub(1));
                    let chunky = sample_raw565(&current, sample_x, sample_y).unwrap_or(curr);
                    blend_565(prev, chunky, alpha)
                }
                other => {
                    let gate = transition_gate(
                        other,
                        progress,
                        local_x,
                        local_y,
                        screen.width(),
                        screen.rows() as usize,
                    );
                    let base = if gate == 255 {
                        curr
                    } else if gate == 0 {
                        prev
                    } else {
                        blend_565(prev, curr, gate)
                    };
                    match other {
                        PreviewTransitionEffect::CopperBars
                        | PreviewTransitionEffect::VenetianCopper
                        | PreviewTransitionEffect::RacingBeam
                            if gate > 0
                                && ((local_y + alpha as usize / 8) & 15 == 0 || gate > 220) =>
                        {
                            brighten_565(base)
                        }
                        PreviewTransitionEffect::PaletteCycle if ((local_x + local_y) & 7) == 0 => {
                            brighten_565(base)
                        }
                        PreviewTransitionEffect::PhosphorDecay if gate < 255 => darken_565(base),
                        PreviewTransitionEffect::StarfieldWarp if gate == 192 => brighten_565(base),
                        _ => base,
                    }
                }
            };
        }
    }
    Some(())
}

#[cfg(mister_bench_scenes)]
pub(super) fn transition_rgb(
    frame: &PreviewRawTransitionFrame<'_>,
    screen: DirtyRect,
    effect: PreviewTransitionEffect,
    progress: f32,
    x: usize,
    y: usize,
) -> (u8, u8, u8) {
    let alpha = (progress.clamp(0.0, 1.0) * 255.0).round() as u8;
    let local_x = x.saturating_sub(screen.x0);
    let local_y = y.saturating_sub(screen.y0);
    let prev = frame
        .previous
        .as_ref()
        .and_then(|prev| sample_preview_rgb(prev, screen, x, y, 0, 1024, 1024))
        .unwrap_or((0, 0, 0));
    let current =
        sample_preview_rgb(&frame.current, screen, x, y, 0, 1024, 1024).unwrap_or((0, 0, 0));

    match effect {
        PreviewTransitionEffect::Cut => current,
        PreviewTransitionEffect::Fade => blend_rgb(prev, current, alpha),
        PreviewTransitionEffect::Wipe => {
            let reveal_w = ((screen.width() as f32) * progress).round() as usize;
            if local_x < reveal_w {
                current
            } else {
                prev
            }
        }
        PreviewTransitionEffect::Slide => {
            let pane_w = screen.width() as isize;
            let offset = ((1.0 - progress) * pane_w as f32).round() as isize;
            let prev_offset = -((progress * pane_w as f32).round() as isize);
            let sliding_current =
                sample_preview_rgb(&frame.current, screen, x, y, offset, 1024, 1024);
            let sliding_prev = frame
                .previous
                .as_ref()
                .and_then(|prev| sample_preview_rgb(prev, screen, x, y, prev_offset, 1024, 1024));
            sliding_current.or(sliding_prev).unwrap_or((0, 0, 0))
        }
        PreviewTransitionEffect::Zoom => {
            let cx = screen.width() / 2;
            let cy = screen.rows() as usize / 2;
            let reveal_w = ((screen.width() as f32) * progress).round() as usize / 2;
            let reveal_h = ((screen.rows() as f32) * progress).round() as usize / 2;
            if local_x.abs_diff(cx) <= reveal_w && local_y.abs_diff(cy) <= reveal_h {
                blend_rgb(prev, current, alpha)
            } else {
                prev
            }
        }
        PreviewTransitionEffect::Scanline => {
            let mut rgb = blend_rgb(prev, current, alpha);
            if local_y & 3 == 0 {
                rgb.0 = ((rgb.0 as u16 * 5) / 8) as u8;
                rgb.1 = ((rgb.1 as u16 * 5) / 8) as u8;
                rgb.2 = ((rgb.2 as u16 * 5) / 8) as u8;
            }
            if local_y < ((screen.rows() as f32) * progress).round() as usize {
                rgb
            } else {
                prev
            }
        }
        PreviewTransitionEffect::Checker => {
            let tile = 16usize;
            let gate = hash2_u8(local_x / tile, local_y / tile);
            if gate <= alpha {
                current
            } else {
                prev
            }
        }
        PreviewTransitionEffect::Dissolve => {
            let gate = hash2_u8(local_x / 2, local_y / 2);
            if gate <= alpha {
                current
            } else {
                prev
            }
        }
        PreviewTransitionEffect::CrtBeamWipe => {
            let beam_y = (progress * (screen.rows() as f32 + 4.0)).round() as isize - 2;
            let dy = local_y as isize - beam_y;
            let base = if dy <= 0 {
                current
            } else if dy <= 10 {
                blend_rgb(prev, current, 220u8.saturating_sub((dy as u8) * 18))
            } else {
                prev
            };
            if dy.abs() <= 2 {
                brighten_rgb(base, 72)
            } else {
                base
            }
        }
        PreviewTransitionEffect::MosaicResolve => {
            let block = mosaic_block_size(progress);
            let sample_x = (screen.x0 + (local_x / block) * block + block / 2)
                .min(screen.x1.saturating_sub(1));
            let sample_y = (screen.y0 + (local_y / block) * block + block / 2)
                .min(screen.y1.saturating_sub(1));
            let chunky =
                sample_preview_rgb(&frame.current, screen, sample_x, sample_y, 0, 1024, 1024)
                    .unwrap_or(current);
            blend_rgb(prev, chunky, alpha)
        }
        other => {
            let gate = transition_gate(
                other,
                progress,
                local_x,
                local_y,
                screen.width(),
                screen.rows() as usize,
            );
            let mut base = if gate == 255 {
                current
            } else if gate == 0 {
                prev
            } else {
                blend_rgb(prev, current, gate)
            };
            match other {
                PreviewTransitionEffect::CopperBars
                | PreviewTransitionEffect::VenetianCopper
                | PreviewTransitionEffect::RacingBeam
                    if gate > 0 && ((local_y + alpha as usize / 8) & 15 == 0 || gate > 220) =>
                {
                    base = brighten_rgb(base, 56);
                }
                PreviewTransitionEffect::PaletteCycle if ((local_x + local_y) & 7) == 0 => {
                    base = brighten_rgb(base, 40);
                }
                PreviewTransitionEffect::PhosphorDecay if gate < 255 => {
                    base.0 = ((base.0 as u16 * 5) / 8) as u8;
                    base.1 = ((base.1 as u16 * 5) / 8) as u8;
                    base.2 = ((base.2 as u16 * 5) / 8) as u8;
                }
                PreviewTransitionEffect::StarfieldWarp if gate == 192 => {
                    base = brighten_rgb(base, 72);
                }
                _ => {}
            }
            base
        }
    }
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
            FramebufferFormat::production_default().label()
        );
        Self::cached(ui)
    }

    pub(super) fn render(&mut self, renderer: &SoftwareRenderer, ui: &UiDisplay) -> PhysicalRegion {
        match self {
            Self::Rgb565 { cached } => renderer.render(cached, ui.render_w()),
        }
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
        match self {
            Self::Rgb565 { cached } => copy_cached_rect_565(disp, ui, cached, rect),
        }
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
        match self {
            Self::Rgb565 { .. } => disp.copy_rect_from_565(x, y, w, h, src),
        }
    }

    pub(super) fn blit_raw_preview(
        &mut self,
        ui: &UiDisplay,
        frame: &PreviewRawFrame<'_>,
        clear_screen: bool,
    ) -> Option<DirtyRect> {
        let rect = raw_preview_scaled_rect(ui, frame)?;
        let screen = preview_screen_rect(ui);
        let image_x =
            screen.x0 as isize + (ARCADE_PREVIEW_BOX_W as isize - frame.display_w as isize) / 2;
        let image_y =
            screen.y0 as isize + (ARCADE_PREVIEW_BOX_H as isize - frame.display_h as isize) / 2;
        let scale_x = (frame.display_w / frame.source_w).max(1) as usize;
        let scale_y = (frame.display_h / frame.source_h).max(1) as usize;
        let src_w = frame.source_w as usize;
        let src_h = frame.source_h as usize;

        match self {
            Self::Rgb565 { cached } => {
                if clear_screen {
                    let black = <Rgb565Pixel as TargetPixel>::from_rgb(0, 0, 0);
                    for y in screen.y0..screen.y1.min(ui.render_h()) {
                        let row = y * ui.render_w();
                        for x in screen.x0..screen.x1.min(ui.render_w()) {
                            cached[row + x] = black;
                        }
                    }
                }
                match frame.pixels {
                    PreviewRawPixels::Empty => {
                        let black = <Rgb565Pixel as TargetPixel>::from_rgb(0, 0, 0);
                        for y in screen.y0..screen.y1.min(ui.render_h()) {
                            let row = y * ui.render_w();
                            for x in screen.x0..screen.x1.min(ui.render_w()) {
                                cached[row + x] = black;
                            }
                        }
                    }
                    PreviewRawPixels::Rgb565 {
                        pixels,
                        stride_pixels,
                    } if frame.display_w == frame.source_w && frame.display_h == frame.source_h => {
                        for y in rect.y0..rect.y1 {
                            let src_y = (y as isize - image_y).max(0) as usize;
                            let src_x = (rect.x0 as isize - image_x).max(0) as usize;
                            let src_a = src_y * stride_pixels + src_x;
                            let dst_a = y * ui.render_w() + rect.x0;
                            cached[dst_a..dst_a + rect.width()]
                                .copy_from_slice(&pixels[src_a..src_a + rect.width()]);
                        }
                    }
                    PreviewRawPixels::Rgb565 {
                        pixels,
                        stride_pixels,
                    } => {
                        for y in rect.y0..rect.y1 {
                            let src_y =
                                ((y as isize - image_y).max(0) as usize / scale_y).min(src_h - 1);
                            let row = y * ui.render_w();
                            for x in rect.x0..rect.x1 {
                                let src_x = ((x as isize - image_x).max(0) as usize / scale_x)
                                    .min(src_w - 1);
                                cached[row + x] = pixels[src_y * stride_pixels + src_x];
                            }
                        }
                    }
                    PreviewRawPixels::Rgb8(rgb) => {
                        for y in rect.y0..rect.y1 {
                            let src_y =
                                ((y as isize - image_y).max(0) as usize / scale_y).min(src_h - 1);
                            let row = y * ui.render_w();
                            for x in rect.x0..rect.x1 {
                                let src_x = ((x as isize - image_x).max(0) as usize / scale_x)
                                    .min(src_w - 1);
                                let si = (src_y * src_w + src_x) * 3;
                                cached[row + x] = <Rgb565Pixel as TargetPixel>::from_rgb(
                                    rgb[si],
                                    rgb[si + 1],
                                    rgb[si + 2],
                                );
                            }
                        }
                    }
                }
            }
        }
        Some(if clear_screen { screen } else { rect })
    }

    pub(super) fn blit_raw_preview_transition(
        &mut self,
        ui: &UiDisplay,
        frame: &PreviewRawTransitionFrame<'_>,
        effect: PreviewTransitionEffect,
        progress: f32,
    ) -> DirtyRect {
        let screen = preview_screen_rect(ui);
        match self {
            Self::Rgb565 { cached } => match effect {
                PreviewTransitionEffect::Cut => {
                    if blit_transition_565_cut(cached, ui, screen, frame).is_none() {
                        blit_transition_565_via_rgb(cached, ui, screen, frame, effect, progress);
                    }
                }
                PreviewTransitionEffect::Fade => {
                    if blit_transition_565_fade(cached, ui, screen, frame, progress).is_none() {
                        blit_transition_565_via_rgb(cached, ui, screen, frame, effect, progress);
                    }
                }
                #[cfg(mister_bench_scenes)]
                _ => {
                    if blit_transition_565_fast(cached, ui, screen, frame, effect, progress)
                        .is_some()
                    {
                        return screen;
                    }
                    for y in screen.y0..screen.y1.min(ui.render_h()) {
                        let row = y * ui.render_w();
                        for x in screen.x0..screen.x1.min(ui.render_w()) {
                            let rgb = transition_rgb(frame, screen, effect, progress, x, y);
                            cached[row + x] =
                                <Rgb565Pixel as TargetPixel>::from_rgb(rgb.0, rgb.1, rgb.2);
                        }
                    }
                }
            },
        }
        screen
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
        match self {
            Self::Rgb565 { cached } => {
                for yy in 0..h {
                    let dst = (y + yy) * ui.render_w() + x;
                    let src_idx = yy * w;
                    for xx in 0..w {
                        let rgb = pixel_to_rgb(src[src_idx + xx]);
                        cached[dst + xx] =
                            <Rgb565Pixel as TargetPixel>::from_rgb(rgb.0, rgb.1, rgb.2);
                    }
                }
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
        match self {
            Self::Rgb565 { cached } => copy_cached_rows_565(disp, ui, cached, y0, y1),
        }
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
            renderer.copy_layer_to_target(target, disp, ui);
            rect.rows()
        }
        ArcadeListUpdate::Scroll { .. } => {
            // `Scroll` means the renderer reused its cached RAM surface. A
            // prior live-framebuffer scroll-present path was visually correct
            // but roughly doubled present cost because `/dev/fb0` reads are
            // expensive on the MiSTer write-combined framebuffer.
            renderer.copy_layer_to_target(target, disp, ui);
            ArcadeListRenderer::dirty_rect().rows()
        }
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

    #[test]
    fn rgb565_fade_blends_centered_preview_pixels() {
        let ui = UiDisplay::for_framebuffer(UI_FB_W, UI_FB_H);
        let screen = preview_screen_rect(&ui);
        let red = <Rgb565Pixel as TargetPixel>::from_rgb(255, 0, 0);
        let blue = <Rgb565Pixel as TargetPixel>::from_rgb(0, 0, 255);
        let previous_pixels = [red; 4];
        let current_pixels = [blue; 4];
        let previous = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &previous_pixels,
                stride_pixels: 2,
            },
            source_w: 2,
            source_h: 2,
            display_w: 2,
            display_h: 2,
        };
        let current = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &current_pixels,
                stride_pixels: 2,
            },
            source_w: 2,
            source_h: 2,
            display_w: 2,
            display_h: 2,
        };
        let frame = PreviewRawTransitionFrame {
            previous: Some(previous),
            current,
            transition_id: 1,
        };
        let mut cached = vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()];

        blit_transition_565_fade(&mut cached, &ui, screen, &frame, 0.5).expect("fade blit");

        let image_x = screen.x0 + (ARCADE_PREVIEW_BOX_W as usize - 2) / 2;
        let image_y = screen.y0 + (ARCADE_PREVIEW_BOX_H as usize - 2) / 2;
        assert_eq!(
            cached[image_y * ui.render_w() + image_x],
            blend_565(red, blue, 128)
        );
    }
}
