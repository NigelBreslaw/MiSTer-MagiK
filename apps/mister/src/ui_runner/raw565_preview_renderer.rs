// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use mister_magik_fb::framebuffer::target::DirtyRect;
#[cfg(any(test, mister_experiments))]
use mister_magik_fb::framebuffer::target::blend_565;
#[cfg(mister_experiments)]
use mister_magik_fb::framebuffer::target::brighten_565;
use mister_magik_fb::preview_transition::{
    blend_rgb565_bucket as blend_565_bucket,
    blend_rgb565_row_with_black as blend_565_row_with_black,
    blend_rgb565_rows_bucketed as blend_565_row_bucketed,
};
use std::sync::OnceLock;
use std::time::Instant;

pub(super) struct Raw565PreviewRenderer;

#[derive(Clone, Copy)]
pub(super) struct PreviewSurface {
    pub(super) x0: usize,
    pub(super) y0: usize,
    pub(super) stride: usize,
}

impl PreviewSurface {
    fn full(ui: &UiDisplay) -> Self {
        Self {
            x0: 0,
            y0: 0,
            stride: ui.render_w(),
        }
    }

    fn row_start(self, y: usize, x: usize) -> usize {
        (y - self.y0) * self.stride + (x - self.x0)
    }

    fn row_range(self, y: usize, x0: usize, x1: usize) -> std::ops::Range<usize> {
        let start = self.row_start(y, x0);
        start..start + (x1 - x0)
    }
}

pub(super) fn preview_screen_rect(ui: &UiDisplay) -> DirtyRect {
    mister_magik_fb::visual_composition::hdmi_preview_rect(ui.render_w(), ui.render_h())
}

fn centered_preview_origin(
    screen: DirtyRect,
    image_width: isize,
    image_height: isize,
) -> (isize, isize) {
    (
        screen.x0 as isize + (screen.width() as isize - image_width) / 2,
        screen.y0 as isize + (screen.rows() as isize - image_height) / 2,
    )
}

fn raw_preview_scaled_rect(ui: &UiDisplay, frame: &PreviewRawFrame<'_>) -> Option<DirtyRect> {
    match frame.status() {
        PreviewRawFrameStatus::Empty => return Some(preview_screen_rect(ui)),
        PreviewRawFrameStatus::Invalid => return None,
        PreviewRawFrameStatus::Ready => {}
    }

    let screen = preview_screen_rect(ui);
    let (image_x, image_y) =
        centered_preview_origin(screen, frame.display_w as isize, frame.display_h as isize);
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

#[cfg(mister_experiments)]
fn rgb565_to_rgb(pixel: Rgb565Pixel) -> (u8, u8, u8) {
    let v = pixel.0;
    let r5 = (v >> 11) & 0x1f;
    let g6 = (v >> 5) & 0x3f;
    let b5 = v & 0x1f;
    (
        ((r5 << 3) | (r5 >> 2)) as u8,
        ((g6 << 2) | (g6 >> 4)) as u8,
        ((b5 << 3) | (b5 >> 2)) as u8,
    )
}

#[cfg(mister_experiments)]
fn sample_preview_rgb(
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
        let (image_x, image_y) =
            centered_preview_origin(screen, frame.source_w as isize, frame.source_h as isize);
        let image_x = image_x + offset_x;
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
    let (image_x, image_y) = centered_preview_origin(screen, scaled_w, scaled_h);
    let image_x = image_x + offset_x;
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

#[cfg(mister_experiments)]
fn blend_rgb(from: (u8, u8, u8), to: (u8, u8, u8), alpha: u8) -> (u8, u8, u8) {
    let a = alpha as u16;
    let ia = 255u16.saturating_sub(a);
    (
        ((from.0 as u16 * ia + to.0 as u16 * a) / 255) as u8,
        ((from.1 as u16 * ia + to.1 as u16 * a) / 255) as u8,
        ((from.2 as u16 * ia + to.2 as u16 * a) / 255) as u8,
    )
}

#[cfg(mister_experiments)]
fn brighten_rgb(rgb: (u8, u8, u8), add: u8) -> (u8, u8, u8) {
    (
        rgb.0.saturating_add(add),
        rgb.1.saturating_add(add),
        rgb.2.saturating_add(add),
    )
}

#[cfg(mister_bench_scenes)]
pub(super) fn hash2_u8(x: usize, y: usize) -> u8 {
    let mut v = (x as u32).wrapping_mul(0x45d9f3b) ^ (y as u32).wrapping_mul(0x119de1f3);
    v ^= v >> 16;
    v = v.wrapping_mul(0x45d9f3b);
    (v >> 24) as u8
}

struct Raw565View<'a> {
    pixels: &'a [Rgb565Pixel],
    stride_pixels: usize,
    source_w: usize,
    source_h: usize,
    display_w: usize,
    display_h: usize,
    x: isize,
    y: isize,
}

fn raw565_view<'a>(
    frame: &'a PreviewRawFrame<'a>,
    screen: DirtyRect,
    offset_x: isize,
) -> Option<Raw565View<'a>> {
    if frame.status() != PreviewRawFrameStatus::Ready {
        return None;
    }
    let PreviewRawPixels::Rgb565 {
        pixels,
        stride_pixels,
    } = frame.pixels
    else {
        return None;
    };
    let source_w = frame.source_w as usize;
    let source_h = frame.source_h as usize;
    let display_w = frame.display_w as usize;
    let display_h = frame.display_h as usize;
    if source_w == 0 || source_h == 0 || display_w == 0 || display_h == 0 {
        return None;
    }
    let (x, y) = centered_preview_origin(screen, display_w as isize, display_h as isize);
    Some(Raw565View {
        pixels,
        stride_pixels,
        source_w,
        source_h,
        display_w,
        display_h,
        x: x + offset_x,
        y,
    })
}

#[cfg_attr(not(mister_bench_scenes), allow(dead_code))]
fn sample_raw565(view: &Raw565View<'_>, x: usize, y: usize) -> Option<Rgb565Pixel> {
    let sx = x as isize - view.x;
    let sy = y as isize - view.y;
    if sx < 0 || sy < 0 || sx >= view.display_w as isize || sy >= view.display_h as isize {
        None
    } else {
        let src_x = ((sx as usize * view.source_w) / view.display_w).min(view.source_w - 1);
        let src_y = ((sy as usize * view.source_h) / view.display_h).min(view.source_h - 1);
        Some(view.pixels[src_y * view.stride_pixels + src_x])
    }
}

fn raw565_row_for_screen_y<'a>(view: &'a Raw565View<'a>, y: usize) -> Option<&'a [Rgb565Pixel]> {
    if view.display_w != view.source_w || view.display_h != view.source_h {
        return None;
    }
    let sy = y as isize - view.y;
    if sy < 0 || sy >= view.display_h as isize {
        None
    } else {
        let start = sy as usize * view.stride_pixels;
        Some(&view.pixels[start..start + view.source_w])
    }
}

struct Raw565ScreenRow<'a> {
    row: &'a [Rgb565Pixel],
    x0: usize,
    x1: usize,
}

#[derive(Clone, Copy, Debug)]
struct FadeWorkStats {
    path: PreviewFadePath,
    pixels: usize,
    rows: usize,
}

impl FadeWorkStats {
    fn new(path: PreviewFadePath, rect: DirtyRect) -> Self {
        Self {
            path,
            pixels: rect.width().saturating_mul(rect.rows() as usize),
            rows: rect.rows() as usize,
        }
    }

    fn empty(path: PreviewFadePath) -> Self {
        Self {
            path,
            pixels: 0,
            rows: 0,
        }
    }
}

fn raw565_screen_row_for_y<'a>(
    view: &'a Raw565View<'a>,
    y: usize,
    x0: usize,
    x1: usize,
    render_w: usize,
) -> Option<Raw565ScreenRow<'a>> {
    if view.display_w != view.source_w || view.display_h != view.source_h {
        return None;
    }
    let sy = y as isize - view.y;
    if sy < 0 || sy >= view.display_h as isize {
        return None;
    }
    let row_x0 = x0.max(view.x.max(0) as usize);
    let row_x1 = x1
        .min((view.x + view.display_w as isize).max(0) as usize)
        .min(render_w);
    if row_x1 <= row_x0 {
        return None;
    }
    let src_x0 = (row_x0 as isize - view.x) as usize;
    let start = sy as usize * view.stride_pixels + src_x0;
    Some(Raw565ScreenRow {
        row: &view.pixels[start..start + (row_x1 - row_x0)],
        x0: row_x0,
        x1: row_x1,
    })
}

fn raw565_screen_bounds_for_y(
    view: &Raw565View<'_>,
    y: usize,
    x0: usize,
    x1: usize,
    render_w: usize,
) -> Option<(usize, usize)> {
    let sy = y as isize - view.y;
    if sy < 0 || sy >= view.display_h as isize {
        return None;
    }
    let row_x0 = x0.max(view.x.max(0) as usize);
    let row_x1 = x1
        .min((view.x + view.display_w as isize).max(0) as usize)
        .min(render_w);
    (row_x1 > row_x0).then_some((row_x0, row_x1))
}

fn raw565_view_screen_rect(
    view: &Raw565View<'_>,
    ui: &UiDisplay,
    screen: DirtyRect,
) -> Option<DirtyRect> {
    let x0 = screen.x0.max(view.x.max(0) as usize);
    let y0 = screen.y0.max(view.y.max(0) as usize);
    let x1 = screen
        .x1
        .min((view.x + view.display_w as isize).max(0) as usize)
        .min(ui.render_w());
    let y1 = screen
        .y1
        .min((view.y + view.display_h as isize).max(0) as usize)
        .min(ui.render_h());
    (x1 > x0 && y1 > y0).then_some(DirtyRect { x0, y0, x1, y1 })
}

#[cfg(target_os = "linux")]
fn thread_cpu_us() -> Option<u64> {
    let mut ts = std::mem::MaybeUninit::<libc::timespec>::uninit();
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, ts.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let ts = unsafe { ts.assume_init() };
    Some((ts.tv_sec as u64).saturating_mul(1_000_000) + (ts.tv_nsec as u64 / 1_000))
}

#[cfg(not(target_os = "linux"))]
fn thread_cpu_us() -> Option<u64> {
    None
}

fn elapsed_thread_cpu_us(start: Option<u64>) -> u64 {
    start
        .and_then(|start| thread_cpu_us().map(|end| end.saturating_sub(start)))
        .unwrap_or(0)
}

fn blend_565_row(
    dst: &mut [Rgb565Pixel],
    previous: &[Rgb565Pixel],
    current: &[Rgb565Pixel],
    alpha: u8,
) {
    assert!(
        previous.len() >= dst.len() && current.len() >= dst.len(),
        "RGB565 fade rows must cover destination length"
    );
    let a = ((alpha as u16 + 4) >> 3).min(32);
    blend_565_row_bucketed(dst, previous, current, a);
}

fn preview_fade_fast_path_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        preview_fade_fast_path_enabled_value(
            std::env::var("MISTER_PREVIEW_FADE_P02").ok().as_deref(),
        )
    })
}

fn preview_fade_fast_path_enabled_value(value: Option<&str>) -> bool {
    !matches!(value, Some("0" | "off" | "false" | "no" | "legacy"))
}

#[cfg(mister_experiments)]
fn darken_565(pixel: Rgb565Pixel) -> Rgb565Pixel {
    let v = pixel.0 as u32;
    let r = (((v >> 11) & 0x1f) * 5) / 8;
    let g = (((v >> 5) & 0x3f) * 5) / 8;
    let b = ((v & 0x1f) * 5) / 8;
    Rgb565Pixel(((r << 11) | (g << 5) | b) as u16)
}

#[cfg(mister_experiments)]
fn mosaic_block_size(progress: f32) -> usize {
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

#[cfg(mister_experiments)]
fn progress_u8(progress: f32) -> u8 {
    (progress.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(mister_bench_scenes)]
pub(super) fn triangle_wave_u8(x: usize, phase: u8) -> u8 {
    let v = ((x as u32).wrapping_mul(13).wrapping_add(phase as u32)) & 0xff;
    let v = if v < 128 { v } else { 255 - v };
    (v * 2).min(255) as u8
}

#[cfg(mister_bench_scenes)]
pub(super) fn plasma_gate(x: usize, y: usize, phase: u8) -> u8 {
    let a = triangle_wave_u8(x / 3 + y / 7, phase);
    let b = triangle_wave_u8(x / 9 + y / 2, phase.wrapping_mul(3));
    ((a as u16 + b as u16) / 2) as u8
}

#[cfg(mister_experiments)]
fn dist2_from_center(local_x: usize, local_y: usize, w: usize, h: usize) -> u64 {
    let cx = w as i64 / 2;
    let cy = h as i64 / 2;
    let dx = local_x as i64 - cx;
    let dy = local_y as i64 - cy;
    (dx * dx + dy * dy) as u64
}

#[cfg(mister_experiments)]
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

#[cfg(mister_experiments)]
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
            if local_x % 16 < open { 255 } else { 0 }
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
            if local_x + local_y < ((w + h) as f32 * progress).round() as usize
                || local_x % 37 == local_y % 29
            {
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
            let tear = ((local_y / 8 + alpha as usize / 16) & 7) as isize - 3;
            if (local_x as isize + tear * 5).max(0) as usize % w.max(1) < reveal_w {
                alpha
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
                alpha / 2
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
            let crunch = ((local_y as f32 - h as f32 / 2.0).abs() * (1.0 - progress)) as usize;
            if crunch < reveal_h / 2 { alpha } else { 0 }
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
            if d <= r * r { alpha } else { 0 }
        }
        PreviewTransitionEffect::MaskBlit => {
            let mask = ((local_x ^ local_y) + (local_x / 7) + (local_y / 11)) & 255;
            if mask <= alpha as usize { 255 } else { 0 }
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
            let wave = triangle_wave_u8(local_y / 3, alpha);
            if wave as usize + reveal_w / 3 > local_x {
                alpha
            } else {
                0
            }
        }
        PreviewTransitionEffect::Slide => 255,
    }
}

fn blit_preview_frame_565_cut(
    cached: &mut [Rgb565Pixel],
    ui: &UiDisplay,
    screen: DirtyRect,
    surface: PreviewSurface,
    frame: &PreviewRawFrame<'_>,
) -> Option<()> {
    if matches!(frame.pixels, PreviewRawPixels::Empty) {
        clear_preview_screen(cached, ui, screen, surface);
        return Some(());
    }
    let current = raw565_view(frame, screen, 0)?;
    let black = <Rgb565Pixel as TargetPixel>::from_rgb(0, 0, 0);
    let image_rect = raw565_view_screen_rect(&current, ui, screen)?;
    for y in screen.y0..screen.y1.min(ui.render_h()) {
        if y < image_rect.y0 || y >= image_rect.y1 {
            for x in screen.x0..screen.x1.min(ui.render_w()) {
                cached[surface.row_start(y, x)] = black;
            }
            continue;
        }
        for x in screen.x0..image_rect.x0 {
            cached[surface.row_start(y, x)] = black;
        }
        let dst_a = surface.row_start(y, image_rect.x0);
        if let Some(src_row) = raw565_row_for_screen_y(&current, y) {
            let src_x = (image_rect.x0 as isize - current.x) as usize;
            cached[dst_a..dst_a + image_rect.width()]
                .copy_from_slice(&src_row[src_x..src_x + image_rect.width()]);
        } else {
            for x in image_rect.x0..image_rect.x1 {
                cached[surface.row_start(y, x)] = sample_raw565(&current, x, y).unwrap_or(black);
            }
        }
        for x in image_rect.x1..screen.x1.min(ui.render_w()) {
            cached[surface.row_start(y, x)] = black;
        }
    }
    Some(())
}

fn blit_transition_565_fade(
    cached: &mut [Rgb565Pixel],
    ui: &UiDisplay,
    screen: DirtyRect,
    surface: PreviewSurface,
    frame: &PreviewRawTransitionFrame<'_>,
    progress: f32,
) -> PreviewFadeTrace {
    let wall_start = Instant::now();
    let cpu_start = thread_cpu_us();
    let current_empty = matches!(frame.current.pixels, PreviewRawPixels::Empty);
    let current = if current_empty {
        None
    } else {
        raw565_view(&frame.current, screen, 0)
    };
    let previous = frame
        .previous
        .as_ref()
        .and_then(|prev| raw565_view(prev, screen, 0));
    let alpha = (progress.clamp(0.0, 1.0) * 255.0).round() as u8;
    let alpha_bucket = ((alpha as u16 + 4) >> 3).min(32) as u8;
    let finish = |stats: FadeWorkStats| PreviewFadeTrace {
        wall_us: wall_start.elapsed().as_micros().min(u64::MAX as u128) as u64,
        cpu_us: elapsed_thread_cpu_us(cpu_start),
        pixels: stats.pixels.min(u32::MAX as usize) as u32,
        rows: stats.rows.min(u32::MAX as usize) as u32,
        path: stats.path,
        alpha_bucket,
    };
    if previous.is_none() && current.is_none() {
        clear_preview_screen(cached, ui, screen, surface);
        return finish(FadeWorkStats::new(PreviewFadePath::Empty, screen));
    }
    if alpha == 0 {
        if let Some(previous) = frame.previous.as_ref() {
            if blit_preview_frame_565_cut(cached, ui, screen, surface, previous).is_some() {
                let rect = raw_preview_scaled_rect(ui, previous).unwrap_or(screen);
                return finish(FadeWorkStats::new(PreviewFadePath::Cut, rect));
            }
        }
        clear_preview_screen(cached, ui, screen, surface);
        return finish(FadeWorkStats::new(PreviewFadePath::Cut, screen));
    }
    if alpha == 255 {
        if blit_preview_frame_565_cut(cached, ui, screen, surface, &frame.current).is_none() {
            clear_preview_screen(cached, ui, screen, surface);
            return finish(FadeWorkStats::new(PreviewFadePath::Cut, screen));
        }
        let rect = raw_preview_scaled_rect(ui, &frame.current).unwrap_or(screen);
        return finish(FadeWorkStats::new(PreviewFadePath::Cut, rect));
    }
    if preview_fade_fast_path_enabled() {
        if let Some(stats) = blit_transition_565_fade_same_geometry(
            cached,
            ui,
            screen,
            surface,
            previous.as_ref(),
            current.as_ref(),
            alpha,
        ) {
            return finish(stats);
        }
    }
    if preview_fade_fast_path_enabled() {
        if let Some(stats) = blit_transition_565_fade_single_geometry(
            cached,
            ui,
            screen,
            surface,
            previous.as_ref(),
            current.as_ref(),
            alpha,
        ) {
            return finish(stats);
        }
    }
    let stats = blit_transition_565_fade_rows(
        cached,
        ui,
        screen,
        surface,
        previous.as_ref(),
        current.as_ref(),
        alpha,
    );
    finish(stats)
}

fn blit_transition_565_fade_same_geometry(
    cached: &mut [Rgb565Pixel],
    ui: &UiDisplay,
    screen: DirtyRect,
    surface: PreviewSurface,
    previous: Option<&Raw565View<'_>>,
    current: Option<&Raw565View<'_>>,
    alpha: u8,
) -> Option<FadeWorkStats> {
    let previous = previous?;
    let current = current?;
    if previous.x != current.x
        || previous.y != current.y
        || previous.display_w != current.display_w
        || previous.display_h != current.display_h
        || previous.display_w != previous.source_w
        || previous.display_h != previous.source_h
        || current.display_w != current.source_w
        || current.display_h != current.source_h
    {
        return None;
    }
    let rect = raw565_view_screen_rect(previous, ui, screen)?;
    if raw565_view_screen_rect(current, ui, screen) != Some(rect) {
        return None;
    }
    let alpha_bucket = ((alpha as u16 + 4) >> 3).min(32);
    for y in rect.y0..rect.y1.min(ui.render_h()) {
        let previous_row = raw565_screen_row_for_y(previous, y, rect.x0, rect.x1, ui.render_w())?;
        let current_row = raw565_screen_row_for_y(current, y, rect.x0, rect.x1, ui.render_w())?;
        let dst = &mut cached[surface.row_range(y, rect.x0, rect.x1)];
        blend_565_row_bucketed(dst, previous_row.row, current_row.row, alpha_bucket);
    }
    Some(FadeWorkStats::new(PreviewFadePath::SameGeometry, rect))
}

fn blit_transition_565_fade_single_geometry(
    cached: &mut [Rgb565Pixel],
    ui: &UiDisplay,
    screen: DirtyRect,
    surface: PreviewSurface,
    previous: Option<&Raw565View<'_>>,
    current: Option<&Raw565View<'_>>,
    alpha: u8,
) -> Option<FadeWorkStats> {
    let (view, fade_in) = match (previous, current) {
        (None, Some(current)) => (current, true),
        (Some(previous), None) => (previous, false),
        _ => return None,
    };
    if view.display_w != view.source_w || view.display_h != view.source_h {
        return None;
    }
    let rect = raw565_view_screen_rect(view, ui, screen)?;
    let alpha_bucket = ((alpha as u16 + 4) >> 3).min(32);
    for y in rect.y0..rect.y1.min(ui.render_h()) {
        let src_row = raw565_screen_row_for_y(view, y, rect.x0, rect.x1, ui.render_w())?;
        let dst = &mut cached[surface.row_range(y, rect.x0, rect.x1)];
        blend_565_row_with_black(dst, src_row.row, alpha_bucket, fade_in);
    }
    Some(FadeWorkStats::new(PreviewFadePath::SingleBlack, rect))
}

fn blit_transition_565_fade_rows(
    cached: &mut [Rgb565Pixel],
    ui: &UiDisplay,
    screen: DirtyRect,
    surface: PreviewSurface,
    previous: Option<&Raw565View<'_>>,
    current: Option<&Raw565View<'_>>,
    alpha: u8,
) -> FadeWorkStats {
    let mut fade_rect: Option<DirtyRect> = None;
    for view in [previous, current].into_iter().flatten() {
        if let Some(rect) = raw565_view_screen_rect(view, ui, screen) {
            fade_rect = Some(fade_rect.map_or(rect, |existing| existing.union(rect)));
        }
    }
    let fade_rect = fade_rect.unwrap_or(screen);
    let black = <Rgb565Pixel as TargetPixel>::from_rgb(0, 0, 0);
    let alpha_bucket = ((alpha as u16 + 4) >> 3).min(32);
    let x1 = fade_rect.x1.min(ui.render_w());
    let mut stats = FadeWorkStats::new(PreviewFadePath::Rows, fade_rect);
    for y in fade_rect.y0..fade_rect.y1.min(ui.render_h()) {
        let previous_bounds = previous.as_ref().and_then(|view| {
            raw565_screen_bounds_for_y(view, y, fade_rect.x0, fade_rect.x1, ui.render_w())
        });
        let current_bounds = current.as_ref().and_then(|view| {
            raw565_screen_bounds_for_y(view, y, fade_rect.x0, fade_rect.x1, ui.render_w())
        });
        let previous_row = previous.as_ref().and_then(|view| {
            raw565_screen_row_for_y(view, y, fade_rect.x0, fade_rect.x1, ui.render_w())
        });
        let current_row = current.as_ref().and_then(|view| {
            raw565_screen_row_for_y(view, y, fade_rect.x0, fade_rect.x1, ui.render_w())
        });

        let mut bounds = [fade_rect.x0, x1, x1, x1, x1, x1];
        let mut len = 2;
        if let Some((x0, x1)) = previous_bounds {
            push_sorted_unique_bound(&mut bounds, &mut len, x0);
            push_sorted_unique_bound(&mut bounds, &mut len, x1);
        }
        if let Some((x0, x1)) = current_bounds {
            push_sorted_unique_bound(&mut bounds, &mut len, x0);
            push_sorted_unique_bound(&mut bounds, &mut len, x1);
        }

        for idx in 0..len - 1 {
            let seg_x0 = bounds[idx];
            let seg_x1 = bounds[idx + 1];
            if seg_x1 <= seg_x0 {
                continue;
            }
            let previous_segment = previous_row
                .as_ref()
                .filter(|row| seg_x0 >= row.x0 && seg_x1 <= row.x1);
            let current_segment = current_row
                .as_ref()
                .filter(|row| seg_x0 >= row.x0 && seg_x1 <= row.x1);
            let dst = &mut cached[surface.row_range(y, seg_x0, seg_x1)];
            match (previous_segment, current_segment) {
                (Some(previous_row), Some(current_row)) => {
                    let previous_start = seg_x0 - previous_row.x0;
                    let current_start = seg_x0 - current_row.x0;
                    blend_565_row(
                        dst,
                        &previous_row.row[previous_start..previous_start + dst.len()],
                        &current_row.row[current_start..current_start + dst.len()],
                        alpha,
                    );
                }
                (Some(previous_row), None) => {
                    let previous_start = seg_x0 - previous_row.x0;
                    for x in 0..dst.len() {
                        dst[x] = blend_565_bucket(
                            previous_row.row[previous_start + x],
                            black,
                            alpha_bucket,
                        );
                    }
                }
                (None, Some(current_row)) => {
                    let current_start = seg_x0 - current_row.x0;
                    for x in 0..dst.len() {
                        dst[x] = blend_565_bucket(
                            black,
                            current_row.row[current_start + x],
                            alpha_bucket,
                        );
                    }
                }
                (None, None) => {
                    let previous_covers =
                        previous_bounds.is_some_and(|(x0, x1)| seg_x0 >= x0 && seg_x1 <= x1);
                    let current_covers =
                        current_bounds.is_some_and(|(x0, x1)| seg_x0 >= x0 && seg_x1 <= x1);
                    match (previous_covers, current_covers) {
                        (true, true) => {
                            stats.path = PreviewFadePath::ScaledSample;
                            let previous = previous.expect("previous bounds require view");
                            let current = current.expect("current bounds require view");
                            for x in 0..dst.len() {
                                let screen_x = seg_x0 + x;
                                let prev = sample_raw565(previous, screen_x, y).unwrap_or(black);
                                let curr = sample_raw565(current, screen_x, y).unwrap_or(black);
                                dst[x] = blend_565_bucket(prev, curr, alpha_bucket);
                            }
                        }
                        (true, false) => {
                            stats.path = PreviewFadePath::ScaledSample;
                            let previous = previous.expect("previous bounds require view");
                            for x in 0..dst.len() {
                                let screen_x = seg_x0 + x;
                                let prev = sample_raw565(previous, screen_x, y).unwrap_or(black);
                                dst[x] = blend_565_bucket(prev, black, alpha_bucket);
                            }
                        }
                        (false, true) => {
                            stats.path = PreviewFadePath::ScaledSample;
                            let current = current.expect("current bounds require view");
                            for x in 0..dst.len() {
                                let screen_x = seg_x0 + x;
                                let curr = sample_raw565(current, screen_x, y).unwrap_or(black);
                                dst[x] = blend_565_bucket(black, curr, alpha_bucket);
                            }
                        }
                        (false, false) => dst.fill(black),
                    }
                }
            }
        }
    }
    stats
}

fn push_sorted_unique_bound(bounds: &mut [usize; 6], len: &mut usize, bound: usize) {
    if bounds[..*len].contains(&bound) {
        return;
    }
    let mut pos = *len;
    while pos > 0 && bounds[pos - 1] > bound {
        bounds[pos] = bounds[pos - 1];
        pos -= 1;
    }
    bounds[pos] = bound;
    *len += 1;
}

#[cfg(mister_experiments)]
mod transition_experiments {
    use super::*;

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
                        if local_x % 16 < open { curr } else { prev }
                    }
                    PreviewTransitionEffect::Iris => {
                        if dist2_from_center(
                            local_x,
                            local_y,
                            screen.width(),
                            screen.rows() as usize,
                        ) <= iris_r2
                        {
                            curr
                        } else {
                            prev
                        }
                    }
                    PreviewTransitionEffect::ClockWipe => {
                        if angle_byte(local_x, local_y, screen.width(), screen.rows() as usize)
                            <= alpha
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
                        if local_x.saturating_add(wave as usize / 2) < reveal_w + screen.width() / 8
                        {
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
                        if mask <= alpha as usize { curr } else { prev }
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
                        let d = dist2_from_center(
                            local_x,
                            local_y,
                            screen.width(),
                            screen.rows() as usize,
                        );
                        let r = (screen.width().min(screen.rows() as usize) as f32
                            * (0.08 + progress * 0.92)) as u64;
                        if d <= r * r {
                            blend_565(prev, curr, alpha)
                        } else {
                            prev
                        }
                    }
                    PreviewTransitionEffect::StarfieldWarp => {
                        let d = dist2_from_center(
                            local_x,
                            local_y,
                            screen.width(),
                            screen.rows() as usize,
                        );
                        let noise = hash2_u8(local_x / 4, local_y / 4) as u64;
                        let max_r2 = ((screen.width() * screen.width()
                            + screen.rows() as usize * screen.rows() as usize)
                            / 4) as u64;
                        if d.saturating_add(noise * 48)
                            <= (max_r2 as f32 * progress * progress) as u64
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
                        let gate = if ((local_x / 12 + local_y / 12 + alpha as usize / 16) & 3) == 0
                        {
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
                            PreviewTransitionEffect::PaletteCycle
                                if ((local_x + local_y) & 7) == 0 =>
                            {
                                brighten_565(base)
                            }
                            PreviewTransitionEffect::PhosphorDecay if gate < 255 => {
                                darken_565(base)
                            }
                            PreviewTransitionEffect::StarfieldWarp if gate == 192 => {
                                brighten_565(base)
                            }
                            _ => base,
                        }
                    }
                };
            }
        }
        Some(())
    }

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
            PreviewTransitionEffect::Fade => blend_rgb(prev, current, alpha),
            PreviewTransitionEffect::Wipe => {
                let reveal_w = ((screen.width() as f32) * progress).round() as usize;
                if local_x < reveal_w { current } else { prev }
            }
            PreviewTransitionEffect::Slide => {
                let pane_w = screen.width() as isize;
                let offset = ((1.0 - progress) * pane_w as f32).round() as isize;
                let prev_offset = -((progress * pane_w as f32).round() as isize);
                let sliding_current =
                    sample_preview_rgb(&frame.current, screen, x, y, offset, 1024, 1024);
                let sliding_prev = frame.previous.as_ref().and_then(|prev| {
                    sample_preview_rgb(prev, screen, x, y, prev_offset, 1024, 1024)
                });
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
                if gate <= alpha { current } else { prev }
            }
            PreviewTransitionEffect::Dissolve => {
                let gate = hash2_u8(local_x / 2, local_y / 2);
                if gate <= alpha { current } else { prev }
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
}

fn clear_preview_screen(
    cached: &mut [Rgb565Pixel],
    ui: &UiDisplay,
    screen: DirtyRect,
    surface: PreviewSurface,
) {
    let black = <Rgb565Pixel as TargetPixel>::from_rgb(0, 0, 0);
    for y in screen.y0..screen.y1.min(ui.render_h()) {
        for x in screen.x0..screen.x1.min(ui.render_w()) {
            cached[surface.row_start(y, x)] = black;
        }
    }
}

impl Raw565PreviewRenderer {
    pub(super) fn compose_frame(
        cached: &mut [Rgb565Pixel],
        ui: &UiDisplay,
        frame: &PreviewRawFrame<'_>,
        clear_screen: bool,
    ) -> Option<DirtyRect> {
        Self::compose_frame_strided(cached, ui, frame, clear_screen, PreviewSurface::full(ui))
    }

    pub(super) fn compose_frame_strided(
        cached: &mut [Rgb565Pixel],
        ui: &UiDisplay,
        frame: &PreviewRawFrame<'_>,
        clear_screen: bool,
        surface: PreviewSurface,
    ) -> Option<DirtyRect> {
        let screen = preview_screen_rect(ui);
        let pixels = match frame.pixels {
            PreviewRawPixels::Empty => mister_magik_fb::visual_composition::PreviewPixels::Empty,
            PreviewRawPixels::Rgb565 {
                pixels,
                stride_pixels,
            } => mister_magik_fb::visual_composition::PreviewPixels::Rgb565 {
                pixels,
                stride_pixels,
            },
            PreviewRawPixels::Rgb8(rgb) => {
                mister_magik_fb::visual_composition::PreviewPixels::Rgb8(rgb)
            }
        };
        mister_magik_fb::visual_composition::compose_preview_frame(
            cached,
            ui.render_w(),
            ui.render_h(),
            screen,
            mister_magik_fb::visual_composition::PreviewFrame {
                pixels,
                source_width: frame.source_w as usize,
                source_height: frame.source_h as usize,
                display_width: frame.display_w as usize,
                display_height: frame.display_h as usize,
            },
            clear_screen,
            mister_magik_fb::visual_composition::PreviewSurface {
                x: surface.x0,
                y: surface.y0,
                stride: surface.stride,
            },
        )
    }

    pub(super) fn compose_transition(
        cached: &mut [Rgb565Pixel],
        ui: &UiDisplay,
        frame: &PreviewRawTransitionFrame<'_>,
        effect: PreviewTransitionEffect,
        progress: f32,
    ) -> (DirtyRect, PreviewFadeTrace) {
        Self::compose_transition_strided(
            cached,
            ui,
            frame,
            effect,
            progress,
            PreviewSurface::full(ui),
        )
    }

    pub(super) fn compose_transition_strided(
        cached: &mut [Rgb565Pixel],
        ui: &UiDisplay,
        frame: &PreviewRawTransitionFrame<'_>,
        effect: PreviewTransitionEffect,
        progress: f32,
        surface: PreviewSurface,
    ) -> (DirtyRect, PreviewFadeTrace) {
        let screen = preview_screen_rect(ui);
        let fade = match effect {
            PreviewTransitionEffect::Fade => {
                blit_transition_565_fade(cached, ui, screen, surface, frame, progress)
            }
            #[cfg(mister_experiments)]
            _ => {
                if transition_experiments::blit_transition_565_fast(
                    cached, ui, screen, frame, effect, progress,
                )
                .is_some()
                {
                    return (screen, PreviewFadeTrace::default());
                }
                for y in screen.y0..screen.y1.min(ui.render_h()) {
                    let row = y * ui.render_w();
                    for x in screen.x0..screen.x1.min(ui.render_w()) {
                        let rgb = transition_experiments::transition_rgb(
                            frame, screen, effect, progress, x, y,
                        );
                        cached[row + x] =
                            <Rgb565Pixel as TargetPixel>::from_rgb(rgb.0, rgb.1, rgb.2);
                    }
                }
                PreviewFadeTrace::default()
            }
        };
        (screen, fade)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_display::{UI_FB_H, UI_FB_W};

    #[test]
    fn empty_raw_preview_compose_clears_preview_screen() {
        let ui = UiDisplay::for_framebuffer(UI_FB_W, UI_FB_H);
        let frame = PreviewRawFrame {
            pixels: PreviewRawPixels::Empty,
            source_w: 1,
            source_h: 1,
            display_w: ARCADE_PREVIEW_BOX_W,
            display_h: ARCADE_PREVIEW_BOX_H,
        };
        let mut cached =
            vec![<Rgb565Pixel as TargetPixel>::from_rgb(0, 255, 0); ui.render_w() * ui.render_h()];

        let rect = Raw565PreviewRenderer::compose_frame(&mut cached, &ui, &frame, true)
            .expect("empty preview rect");
        let screen = preview_screen_rect(&ui);

        assert_eq!(rect, screen);
        let center = ((screen.y0 + screen.y1) / 2) * ui.render_w() + (screen.x0 + screen.x1) / 2;
        assert_eq!(cached[center].0, 0);
    }

    #[test]
    fn preview_screen_rect_stays_inside_small_framebuffer() {
        let ui = UiDisplay::for_framebuffer(320, 240);
        let rect = preview_screen_rect(&ui);

        assert!(rect.x0 <= rect.x1);
        assert!(rect.y0 <= rect.y1);
        assert!(rect.x1 <= ui.render_w());
        assert!(rect.y1 <= ui.render_h());
    }

    #[test]
    fn raw_preview_image_is_centered_in_dynamic_hdmi_aperture() {
        let frame = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &[Rgb565Pixel(0xffff); 4],
                stride_pixels: 2,
            },
            source_w: 2,
            source_h: 2,
            display_w: 100,
            display_h: 80,
        };

        for (frame_width, frame_height) in
            [(683, 384), (960, 540), (960, 600), (1024, 768), (1280, 720)]
        {
            let ui = UiDisplay::for_framebuffer(frame_width, frame_height);
            let screen = preview_screen_rect(&ui);
            let image = raw_preview_scaled_rect(&ui, &frame).expect("preview image rect");

            assert!(image.x0 >= screen.x0);
            assert!(image.y0 >= screen.y0);
            assert!(image.x1 <= screen.x1);
            assert!(image.y1 <= screen.y1);
            assert!((image.x0 + image.x1).abs_diff(screen.x0 + screen.x1) <= 1);
            assert!((image.y0 + image.y1).abs_diff(screen.y0 + screen.y1) <= 1);
        }
    }

    #[test]
    fn raw_preview_compose_rejects_invalid_frames() {
        let ui = UiDisplay::for_framebuffer(UI_FB_W, UI_FB_H);
        let pixels = [Rgb565Pixel(0xffff); 1];
        let frame = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &pixels,
                stride_pixels: 1,
            },
            source_w: 2,
            source_h: 2,
            display_w: 2,
            display_h: 2,
        };
        let mut cached = vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()];

        assert_eq!(
            Raw565PreviewRenderer::compose_frame(&mut cached, &ui, &frame, false),
            None
        );
    }

    #[test]
    fn raw565_two_x_path_duplicates_each_source_pixel_exactly() {
        let ui = UiDisplay::for_framebuffer(UI_FB_W, UI_FB_H);
        let pixels = [
            Rgb565Pixel(1),
            Rgb565Pixel(2),
            Rgb565Pixel(3),
            Rgb565Pixel(4),
        ];
        let frame = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &pixels,
                stride_pixels: 2,
            },
            source_w: 2,
            source_h: 2,
            display_w: 4,
            display_h: 4,
        };
        let mut cached = vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()];

        let rect = Raw565PreviewRenderer::compose_frame(&mut cached, &ui, &frame, false).unwrap();
        let rows = (rect.y0..rect.y1)
            .map(|y| {
                cached[y * ui.render_w() + rect.x0..y * ui.render_w() + rect.x1]
                    .iter()
                    .map(|p| p.0)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            rows,
            vec![
                vec![1, 1, 2, 2],
                vec![1, 1, 2, 2],
                vec![3, 3, 4, 4],
                vec![3, 3, 4, 4]
            ]
        );
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
            duration_divisor: 1,
        };
        let mut cached = vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()];

        Raw565PreviewRenderer::compose_transition(
            &mut cached,
            &ui,
            &frame,
            PreviewTransitionEffect::Fade,
            0.5,
        );

        let (image_x, image_y) = centered_preview_origin(screen, 2, 2);
        assert_eq!(
            cached[image_y as usize * ui.render_w() + image_x as usize],
            blend_565(red, blue, 128)
        );
    }

    #[test]
    fn rgb565_row_blend_matches_scalar_blend() {
        let previous = [
            Rgb565Pixel(0x0000),
            Rgb565Pixel(0xffff),
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x07e0),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0x8410),
            Rgb565Pixel(0x39e7),
            Rgb565Pixel(0x1234),
            Rgb565Pixel(0xf81f),
            Rgb565Pixel(0x07ff),
            Rgb565Pixel(0xffe0),
            Rgb565Pixel(0x7bef),
        ];
        let current = [
            Rgb565Pixel(0xffff),
            Rgb565Pixel(0x0000),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x07e0),
            Rgb565Pixel(0x7bef),
            Rgb565Pixel(0x1234),
            Rgb565Pixel(0x39e7),
            Rgb565Pixel(0x07ff),
            Rgb565Pixel(0xf81f),
            Rgb565Pixel(0x8410),
            Rgb565Pixel(0xffe0),
        ];
        for alpha in 0..=255 {
            let mut optimized = [Rgb565Pixel(0); 12];
            blend_565_row(&mut optimized, &previous, &current, alpha);
            let expected: Vec<_> = previous
                .iter()
                .zip(current.iter())
                .map(|(&prev, &curr)| blend_565(prev, curr, alpha))
                .collect();

            assert_eq!(optimized.as_slice(), expected.as_slice(), "alpha={alpha}");
        }
    }

    #[test]
    fn preview_fade_fast_path_env_flag_defaults_on_and_accepts_legacy_off() {
        assert!(preview_fade_fast_path_enabled_value(None));
        assert!(preview_fade_fast_path_enabled_value(Some("1")));
        assert!(!preview_fade_fast_path_enabled_value(Some("0")));
        assert!(!preview_fade_fast_path_enabled_value(Some("legacy")));
    }

    #[test]
    fn rgb565_black_row_blend_matches_scalar_blend() {
        let black = <Rgb565Pixel as TargetPixel>::from_rgb(0, 0, 0);
        let pixels = [
            Rgb565Pixel(0x0000),
            Rgb565Pixel(0xffff),
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x07e0),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0x8410),
            Rgb565Pixel(0x39e7),
            Rgb565Pixel(0x1234),
        ];
        for alpha in 0..=255 {
            let bucket = ((alpha as u16 + 4) >> 3).min(32);
            let mut fade_in = [Rgb565Pixel(0); 8];
            blend_565_row_with_black(&mut fade_in, &pixels, bucket, true);
            let expected_in: Vec<_> = pixels
                .iter()
                .map(|&pixel| blend_565(black, pixel, alpha))
                .collect();
            assert_eq!(
                fade_in.as_slice(),
                expected_in.as_slice(),
                "in alpha={alpha}"
            );

            let mut fade_out = [Rgb565Pixel(0); 8];
            blend_565_row_with_black(&mut fade_out, &pixels, bucket, false);
            let expected_out: Vec<_> = pixels
                .iter()
                .map(|&pixel| blend_565(pixel, black, alpha))
                .collect();
            assert_eq!(
                fade_out.as_slice(),
                expected_out.as_slice(),
                "out alpha={alpha}"
            );
        }
    }

    #[test]
    fn rgb565_single_fade_matches_scalar_reference() {
        let ui = UiDisplay::for_framebuffer(UI_FB_W, UI_FB_H);
        let screen = preview_screen_rect(&ui);
        let previous_pixels = [
            Rgb565Pixel(0x0000),
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x07e0),
            Rgb565Pixel(0xffff),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0xffe0),
            Rgb565Pixel(0x1234),
            Rgb565Pixel(0x8410),
            Rgb565Pixel(0x39e7),
            Rgb565Pixel(0x7bef),
            Rgb565Pixel(0xf81f),
            Rgb565Pixel(0x07ff),
        ];
        let current_pixels = [
            Rgb565Pixel(0xffff),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x0000),
            Rgb565Pixel(0x07e0),
            Rgb565Pixel(0x39e7),
            Rgb565Pixel(0x8410),
            Rgb565Pixel(0x1234),
            Rgb565Pixel(0x07ff),
            Rgb565Pixel(0xf81f),
            Rgb565Pixel(0x7bef),
            Rgb565Pixel(0xffe0),
        ];
        let previous = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &previous_pixels,
                stride_pixels: 4,
            },
            source_w: 3,
            source_h: 3,
            display_w: 3,
            display_h: 3,
        };
        let current = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &current_pixels,
                stride_pixels: 4,
            },
            source_w: 3,
            source_h: 3,
            display_w: 3,
            display_h: 3,
        };
        let frame = PreviewRawTransitionFrame {
            previous: Some(previous),
            current,
            transition_id: 1,
            duration_divisor: 1,
        };

        for alpha in [1, 64, 128, 192, 254] {
            let sentinel = Rgb565Pixel(0x4208);
            let mut actual = vec![sentinel; ui.render_w() * ui.render_h()];
            let mut expected = actual.clone();

            Raw565PreviewRenderer::compose_transition(
                &mut actual,
                &ui,
                &frame,
                PreviewTransitionEffect::Fade,
                alpha as f32 / 255.0,
            );
            paint_scalar_fade_reference(&mut expected, &ui, screen, &frame, alpha);

            assert_eq!(actual, expected, "alpha={alpha}");
        }
    }

    #[test]
    fn rgb565_same_geometry_fade_reports_cpu_trace_shape() {
        let ui = UiDisplay::for_framebuffer(UI_FB_W, UI_FB_H);
        let previous_pixels = [Rgb565Pixel(0xf800); 4];
        let current_pixels = [Rgb565Pixel(0x001f); 4];
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
            duration_divisor: 1,
        };
        let mut cached = vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()];

        let (_rect, trace) = Raw565PreviewRenderer::compose_transition(
            &mut cached,
            &ui,
            &frame,
            PreviewTransitionEffect::Fade,
            128.0 / 255.0,
        );

        assert_eq!(trace.path, PreviewFadePath::SameGeometry);
        assert_eq!(trace.alpha_bucket, 16);
        assert_eq!(trace.pixels, 4);
        assert_eq!(trace.rows, 2);
    }

    #[test]
    fn rgb565_legacy_fade_rows_match_scalar_reference() {
        let ui = UiDisplay::for_framebuffer(UI_FB_W, UI_FB_H);
        let screen = preview_screen_rect(&ui);
        let previous_pixels = [
            Rgb565Pixel(0x0000),
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x07e0),
            Rgb565Pixel(0xffff),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0xffe0),
            Rgb565Pixel(0x1234),
            Rgb565Pixel(0x8410),
            Rgb565Pixel(0x39e7),
        ];
        let current_pixels = [
            Rgb565Pixel(0xffff),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x0000),
            Rgb565Pixel(0x07e0),
            Rgb565Pixel(0x39e7),
            Rgb565Pixel(0x8410),
            Rgb565Pixel(0x1234),
            Rgb565Pixel(0x07ff),
        ];
        let previous = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &previous_pixels,
                stride_pixels: 3,
            },
            source_w: 3,
            source_h: 3,
            display_w: 3,
            display_h: 3,
        };
        let current = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &current_pixels,
                stride_pixels: 3,
            },
            source_w: 3,
            source_h: 3,
            display_w: 3,
            display_h: 3,
        };
        let frame = PreviewRawTransitionFrame {
            previous: Some(previous),
            current,
            transition_id: 1,
            duration_divisor: 1,
        };
        let previous_view = raw565_view(frame.previous.as_ref().unwrap(), screen, 0);
        let current_view = raw565_view(&frame.current, screen, 0);

        let sentinel = Rgb565Pixel(0x4208);
        let mut actual = vec![sentinel; ui.render_w() * ui.render_h()];
        let mut expected = actual.clone();
        blit_transition_565_fade_rows(
            &mut actual,
            &ui,
            screen,
            PreviewSurface::full(&ui),
            previous_view.as_ref(),
            current_view.as_ref(),
            128,
        );
        paint_scalar_fade_reference(&mut expected, &ui, screen, &frame, 128);

        assert_eq!(actual, expected);
    }

    #[test]
    fn rgb565_single_frame_fade_from_black_matches_scalar_reference() {
        let ui = UiDisplay::for_framebuffer(UI_FB_W, UI_FB_H);
        let screen = preview_screen_rect(&ui);
        let current_pixels = [
            Rgb565Pixel(0xffff),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x0000),
            Rgb565Pixel(0x07e0),
            Rgb565Pixel(0x39e7),
            Rgb565Pixel(0x8410),
            Rgb565Pixel(0x1234),
            Rgb565Pixel(0x07ff),
        ];
        let current = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &current_pixels,
                stride_pixels: 3,
            },
            source_w: 3,
            source_h: 3,
            display_w: 3,
            display_h: 3,
        };
        let frame = PreviewRawTransitionFrame {
            previous: None,
            current,
            transition_id: 1,
            duration_divisor: 1,
        };

        let sentinel = Rgb565Pixel(0x4208);
        let mut actual = vec![sentinel; ui.render_w() * ui.render_h()];
        let mut expected = actual.clone();
        Raw565PreviewRenderer::compose_transition(
            &mut actual,
            &ui,
            &frame,
            PreviewTransitionEffect::Fade,
            128.0 / 255.0,
        );
        paint_scalar_fade_reference(&mut expected, &ui, screen, &frame, 128);

        assert_eq!(actual, expected);
    }

    #[test]
    fn rgb565_single_fade_handles_scaled_frames() {
        let ui = UiDisplay::for_framebuffer(UI_FB_W, UI_FB_H);
        let screen = preview_screen_rect(&ui);
        let previous_pixels = [
            Rgb565Pixel(0x0000),
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x07e0),
            Rgb565Pixel(0xffff),
        ];
        let current_pixels = [
            Rgb565Pixel(0xffff),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x0000),
        ];
        let previous = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &previous_pixels,
                stride_pixels: 2,
            },
            source_w: 2,
            source_h: 2,
            display_w: 4,
            display_h: 4,
        };
        let current = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &current_pixels,
                stride_pixels: 2,
            },
            source_w: 2,
            source_h: 2,
            display_w: 4,
            display_h: 4,
        };
        let frame = PreviewRawTransitionFrame {
            previous: Some(previous),
            current,
            transition_id: 1,
            duration_divisor: 1,
        };
        let alpha = 128;
        let sentinel = Rgb565Pixel(0x4208);
        let mut actual = vec![sentinel; ui.render_w() * ui.render_h()];
        let mut expected = actual.clone();

        Raw565PreviewRenderer::compose_transition(
            &mut actual,
            &ui,
            &frame,
            PreviewTransitionEffect::Fade,
            alpha as f32 / 255.0,
        );
        paint_scalar_fade_reference(&mut expected, &ui, screen, &frame, alpha);

        assert_eq!(actual, expected);
    }

    #[test]
    fn rgb565_scaled_fade_reports_scaled_sample_path() {
        let ui = UiDisplay::for_framebuffer(UI_FB_W, UI_FB_H);
        let previous_pixels = [
            Rgb565Pixel(0x0000),
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x07e0),
            Rgb565Pixel(0xffff),
        ];
        let current_pixels = [
            Rgb565Pixel(0xffff),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x0000),
        ];
        let previous = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &previous_pixels,
                stride_pixels: 2,
            },
            source_w: 2,
            source_h: 2,
            display_w: 4,
            display_h: 4,
        };
        let current = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &current_pixels,
                stride_pixels: 2,
            },
            source_w: 2,
            source_h: 2,
            display_w: 4,
            display_h: 4,
        };
        let frame = PreviewRawTransitionFrame {
            previous: Some(previous),
            current,
            transition_id: 1,
            duration_divisor: 1,
        };
        let mut cached = vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()];

        let (_rect, trace) = Raw565PreviewRenderer::compose_transition(
            &mut cached,
            &ui,
            &frame,
            PreviewTransitionEffect::Fade,
            128.0 / 255.0,
        );

        assert_eq!(trace.path, PreviewFadePath::ScaledSample);
        assert_eq!(trace.pixels, 16);
        assert_eq!(trace.rows, 4);
    }

    fn paint_scalar_fade_reference(
        cached: &mut [Rgb565Pixel],
        ui: &UiDisplay,
        screen: DirtyRect,
        frame: &PreviewRawTransitionFrame<'_>,
        alpha: u8,
    ) {
        let previous = frame
            .previous
            .as_ref()
            .and_then(|frame| raw565_view(frame, screen, 0));
        let current = raw565_view(&frame.current, screen, 0);
        let mut fade_rect: Option<DirtyRect> = None;
        for view in [previous.as_ref(), current.as_ref()].into_iter().flatten() {
            if let Some(rect) = raw565_view_screen_rect(view, ui, screen) {
                fade_rect = Some(fade_rect.map_or(rect, |existing| existing.union(rect)));
            }
        }
        let fade_rect = fade_rect.unwrap_or(screen);
        let black = <Rgb565Pixel as TargetPixel>::from_rgb(0, 0, 0);
        for y in fade_rect.y0..fade_rect.y1.min(ui.render_h()) {
            let row = y * ui.render_w();
            for x in fade_rect.x0..fade_rect.x1.min(ui.render_w()) {
                let previous = previous
                    .as_ref()
                    .and_then(|view| sample_raw565(view, x, y))
                    .unwrap_or(black);
                let current = current
                    .as_ref()
                    .and_then(|view| sample_raw565(view, x, y))
                    .unwrap_or(black);
                cached[row + x] = blend_565(previous, current, alpha);
            }
        }
    }
}
