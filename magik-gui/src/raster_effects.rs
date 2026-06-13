//! Host-testable classic arcade raster and palette effects.

use std::time::Instant;

pub use crate::camera_effects::pixel_to_rgb888;
use crate::camera_effects::{color, synthetic_images, CameraImage, CameraPixel};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RasterEffectKind {
    PaletteCyclingLavaWaterNeon,
    PaletteGradientSky,
    PerScanlineColorBars,
    RainbowRasterBands,
    CopperBarHorizontalGlow,
    ScreenFlashAction,
    FadeThroughIndexedPalettes,
    DayNightPaletteSwap,
    LimitedColorPosterizeTransition,
    DitherDissolve,
    OrderedCheckerDissolve,
    CrtPhosphorFadeTrail,
    ScanlineBrightnessPulse,
    PaletteSplitWarningTint,
    WaterReflectionFlippedWavyRows,
}

impl RasterEffectKind {
    pub const ALL: [Self; 15] = [
        Self::PaletteCyclingLavaWaterNeon,
        Self::PaletteGradientSky,
        Self::PerScanlineColorBars,
        Self::RainbowRasterBands,
        Self::CopperBarHorizontalGlow,
        Self::ScreenFlashAction,
        Self::FadeThroughIndexedPalettes,
        Self::DayNightPaletteSwap,
        Self::LimitedColorPosterizeTransition,
        Self::DitherDissolve,
        Self::OrderedCheckerDissolve,
        Self::CrtPhosphorFadeTrail,
        Self::ScanlineBrightnessPulse,
        Self::PaletteSplitWarningTint,
        Self::WaterReflectionFlippedWavyRows,
    ];

    pub fn all() -> &'static [Self] {
        &Self::ALL
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PaletteCyclingLavaWaterNeon => "palette-cycling-lava-water-neon",
            Self::PaletteGradientSky => "palette-gradient-sky",
            Self::PerScanlineColorBars => "per-scanline-color-bars",
            Self::RainbowRasterBands => "rainbow-raster-bands",
            Self::CopperBarHorizontalGlow => "copper-bar-horizontal-glow",
            Self::ScreenFlashAction => "screen-flash-action",
            Self::FadeThroughIndexedPalettes => "fade-through-indexed-palettes",
            Self::DayNightPaletteSwap => "day-night-palette-swap",
            Self::LimitedColorPosterizeTransition => "limited-color-posterize-transition",
            Self::DitherDissolve => "dither-dissolve",
            Self::OrderedCheckerDissolve => "ordered-checker-dissolve",
            Self::CrtPhosphorFadeTrail => "crt-phosphor-fade-trail",
            Self::ScanlineBrightnessPulse => "scanline-brightness-pulse",
            Self::PaletteSplitWarningTint => "palette-split-warning-tint",
            Self::WaterReflectionFlippedWavyRows => "water-reflection-flipped-wavy-rows",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.to_ascii_lowercase().replace('_', "-");
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.label() == normalized)
    }

    pub fn labels() -> String {
        Self::ALL
            .iter()
            .map(|kind| kind.label())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RasterEffectFrameStats {
    pub clear_us: u64,
    pub background_us: u64,
    pub projection_us: u64,
    pub image_blit_us: u64,
    pub sprite_us: u64,
    pub post_us: u64,
    pub hud_us: u64,
    pub palette_step_count: u64,
    pub lut_lookup_count: u64,
    pub row_op_count: u64,
    pub dither_pixel_count: u64,
    pub flash_pixel_count: u64,
    pub trail_pixel_count: u64,
    pub indexed_pixel_count: u64,
    pub reflection_row_count: u64,
}

impl RasterEffectFrameStats {
    pub fn draw_us(self) -> u64 {
        self.clear_us
            + self.background_us
            + self.projection_us
            + self.image_blit_us
            + self.sprite_us
            + self.post_us
            + self.hud_us
    }
}

#[derive(Default)]
struct RasterCounters {
    palette_step_count: u64,
    lut_lookup_count: u64,
    row_op_count: u64,
    dither_pixel_count: u64,
    flash_pixel_count: u64,
    trail_pixel_count: u64,
    indexed_pixel_count: u64,
    reflection_row_count: u64,
}

impl From<RasterCounters> for RasterEffectFrameStats {
    fn from(c: RasterCounters) -> Self {
        Self {
            palette_step_count: c.palette_step_count,
            lut_lookup_count: c.lut_lookup_count,
            row_op_count: c.row_op_count,
            dither_pixel_count: c.dither_pixel_count,
            flash_pixel_count: c.flash_pixel_count,
            trail_pixel_count: c.trail_pixel_count,
            indexed_pixel_count: c.indexed_pixel_count,
            reflection_row_count: c.reflection_row_count,
            ..Self::default()
        }
    }
}

pub struct RasterEffectRenderState {
    indexed: Vec<u8>,
    trail: Vec<CameraPixel>,
    scratch: Vec<CameraPixel>,
    w: usize,
    h: usize,
}

impl RasterEffectRenderState {
    pub fn new(w: usize, h: usize) -> Self {
        let mut state = Self {
            indexed: vec![0; w * h],
            trail: vec![CameraPixel(0); w * h],
            scratch: vec![CameraPixel(0); w * h],
            w,
            h,
        };
        state.rebuild_indexed();
        state
    }

    fn resize(&mut self, w: usize, h: usize) {
        if self.w == w && self.h == h {
            return;
        }
        self.indexed.resize(w * h, 0);
        self.trail.resize(w * h, CameraPixel(0));
        self.scratch.resize(w * h, CameraPixel(0));
        self.w = w;
        self.h = h;
        self.rebuild_indexed();
    }

    fn rebuild_indexed(&mut self) {
        for y in 0..self.h {
            for x in 0..self.w {
                let wave = ((x / 10) ^ (y / 7) ^ ((x + y) / 19)) as u8;
                let radial = (((x as isize - self.w as isize / 2).abs()
                    + (y as isize - self.h as isize / 2).abs())
                    / 9) as u8;
                self.indexed[y * self.w + x] = wave.wrapping_add(radial) & 31;
            }
        }
        self.trail.fill(CameraPixel(0));
    }
}

pub fn render_raster_effect_frame(
    dst: &mut [CameraPixel],
    state: &mut RasterEffectRenderState,
    w: usize,
    h: usize,
    images: &[CameraImage],
    kind: RasterEffectKind,
    frame: u64,
    hud: Option<&str>,
) -> RasterEffectFrameStats {
    assert_eq!(dst.len(), w * h);
    state.resize(w, h);
    let mut stats = RasterEffectFrameStats::default();
    let mut counters = RasterCounters::default();

    let t = Instant::now();
    clear(dst, color(0, 0, 0));
    stats.clear_us = elapsed_us(t);

    match kind {
        RasterEffectKind::PaletteCyclingLavaWaterNeon => {
            time(&mut stats.background_us, || {
                let pal = lava_water_neon_palette(frame);
                draw_indexed_palette(dst, &state.indexed, &pal, frame as usize, &mut counters);
            });
            time(&mut stats.sprite_us, || {
                draw_raster_grid(dst, w, h, frame, &mut counters)
            });
        }
        RasterEffectKind::PaletteGradientSky => {
            time(&mut stats.background_us, || {
                draw_palette_gradient_sky(dst, w, h, frame, &mut counters)
            });
        }
        RasterEffectKind::PerScanlineColorBars => {
            time(&mut stats.background_us, || {
                draw_scanline_color_bars(dst, w, h, frame, &mut counters)
            });
        }
        RasterEffectKind::RainbowRasterBands => {
            time(&mut stats.background_us, || {
                draw_rainbow_bands(dst, w, h, frame, 9, &mut counters)
            });
        }
        RasterEffectKind::CopperBarHorizontalGlow => {
            time(&mut stats.background_us, || clear(dst, color(2, 1, 10)));
            time(&mut stats.projection_us, || {
                draw_copper_glow(dst, w, h, frame, &mut counters)
            });
        }
        RasterEffectKind::ScreenFlashAction => {
            time(&mut stats.background_us, || {
                if screen_flash_active(frame) {
                    draw_action_flash_scene(dst, w, h, frame, &mut counters);
                } else {
                    draw_arcade_scene(dst, w, h, images, frame);
                }
            });
            time(&mut stats.post_us, || {
                draw_screen_flash(dst, frame, &mut counters)
            });
        }
        RasterEffectKind::FadeThroughIndexedPalettes => {
            time(&mut stats.background_us, || {
                let pal = fade_palette(frame, &mut counters);
                draw_indexed_palette(dst, &state.indexed, &pal, frame as usize / 3, &mut counters);
            });
        }
        RasterEffectKind::DayNightPaletteSwap => {
            time(&mut stats.background_us, || {
                let pal = day_night_palette(frame, &mut counters);
                draw_indexed_palette(dst, &state.indexed, &pal, 0, &mut counters);
            });
            time(&mut stats.sprite_us, || {
                draw_skyline(dst, w, h, frame, &mut counters)
            });
        }
        RasterEffectKind::LimitedColorPosterizeTransition => {
            time(&mut stats.image_blit_us, || {
                draw_posterize_transition(dst, w, h, images, frame, &mut counters)
            });
        }
        RasterEffectKind::DitherDissolve => {
            time(&mut stats.image_blit_us, || {
                draw_dither_dissolve(dst, w, h, images, frame, &mut counters)
            });
        }
        RasterEffectKind::OrderedCheckerDissolve => {
            time(&mut stats.image_blit_us, || {
                draw_checker_dissolve(dst, w, h, images, frame, &mut counters)
            });
        }
        RasterEffectKind::CrtPhosphorFadeTrail => {
            time(&mut stats.background_us, || {
                draw_crt_phosphor(dst, &mut state.trail, w, h, frame, &mut counters)
            });
            time(&mut stats.post_us, || {
                draw_scanline_overlay(dst, w, h, frame, &mut counters)
            });
        }
        RasterEffectKind::ScanlineBrightnessPulse => {
            time(&mut stats.background_us, || {
                draw_scanline_color_bars(dst, w, h, frame / 2, &mut counters)
            });
            time(&mut stats.post_us, || {
                draw_scanline_brightness_pulse(dst, w, h, frame, &mut counters)
            });
        }
        RasterEffectKind::PaletteSplitWarningTint => {
            time(&mut stats.background_us, || {
                draw_palette_gradient_sky(dst, w, h, frame, &mut counters)
            });
            time(&mut stats.post_us, || {
                draw_warning_split_tint(dst, w, h, frame, &mut counters)
            });
        }
        RasterEffectKind::WaterReflectionFlippedWavyRows => {
            time(&mut stats.background_us, || {
                draw_palette_gradient_sky(dst, w, h, frame / 2, &mut counters);
                draw_skyline(dst, w, h, frame / 2, &mut counters);
            });
            time(&mut stats.projection_us, || {
                state.scratch.copy_from_slice(dst);
                draw_water_reflection(dst, &state.scratch, w, h, frame, &mut counters);
            });
        }
    }

    let counter_stats: RasterEffectFrameStats = counters.into();
    stats.palette_step_count = counter_stats.palette_step_count;
    stats.lut_lookup_count = counter_stats.lut_lookup_count;
    stats.row_op_count = counter_stats.row_op_count;
    stats.dither_pixel_count = counter_stats.dither_pixel_count;
    stats.flash_pixel_count = counter_stats.flash_pixel_count;
    stats.trail_pixel_count = counter_stats.trail_pixel_count;
    stats.indexed_pixel_count = counter_stats.indexed_pixel_count;
    stats.reflection_row_count = counter_stats.reflection_row_count;

    if let Some(text) = hud {
        time(&mut stats.hud_us, || draw_label(dst, w, h, text));
    }

    stats
}

pub fn synthetic_raster_images(count: usize) -> Vec<CameraImage> {
    synthetic_images(count)
}

fn time(out: &mut u64, f: impl FnOnce()) {
    let t = Instant::now();
    f();
    *out += elapsed_us(t);
}

fn elapsed_us(t: Instant) -> u64 {
    t.elapsed().as_micros() as u64
}

fn clear(dst: &mut [CameraPixel], c: CameraPixel) {
    dst.fill(c);
}

fn draw_indexed_palette(
    dst: &mut [CameraPixel],
    indexed: &[u8],
    pal: &[CameraPixel; 32],
    offset: usize,
    counters: &mut RasterCounters,
) {
    for (i, out) in dst.iter_mut().enumerate() {
        let idx = indexed[i].wrapping_add((offset / 2) as u8) as usize & 31;
        *out = pal[idx];
    }
    counters.palette_step_count += 32;
    counters.lut_lookup_count += dst.len() as u64;
    counters.indexed_pixel_count += dst.len() as u64;
}

fn lava_water_neon_palette(frame: u64) -> [CameraPixel; 32] {
    let mut pal = [CameraPixel(0); 32];
    for (i, out) in pal.iter_mut().enumerate() {
        let phase = ((i * 8 + frame as usize * 3) & 255) as u8;
        *out = match i / 8 {
            0 => color(phase.saturating_add(30), phase / 4, 0),
            1 => color(30, 50 + phase / 3, 130 + phase / 3),
            2 => color(phase / 2, 0, 160 + phase / 3),
            _ => color(20 + phase / 4, 180 + phase / 4, 80 + phase / 3),
        };
    }
    pal
}

fn draw_palette_gradient_sky(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut RasterCounters,
) {
    let sun_y = (h as i32 / 3) + tri_i32(frame as i32, 160, h as i32 / 7);
    for y in 0..h {
        let sky = y as u32 * 255 / h.max(1) as u32;
        let warm = (80i32 - (y as i32 - sun_y).abs()).max(0) as u8;
        let c = color(
            (20 + warm).min(255),
            (28 + sky / 4 + warm as u32 / 3).min(255) as u8,
            (90 + sky / 2).min(255) as u8,
        );
        row(dst, w, y).fill(c);
        counters.row_op_count += 1;
    }
    let sun_x = w as i32 / 2 + tri_i32(frame as i32 + 50, 220, w as i32 / 4);
    fill_disc(
        dst,
        w,
        h,
        sun_x,
        sun_y,
        h.min(w) as i32 / 12,
        color(255, 210, 90),
    );
    counters.palette_step_count += h as u64;
}

fn draw_scanline_color_bars(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut RasterCounters,
) {
    let pal = [
        color(250, 40, 30),
        color(250, 190, 50),
        color(70, 220, 80),
        color(40, 210, 210),
        color(70, 80, 240),
        color(200, 60, 230),
    ];
    for y in 0..h {
        let band = ((y as u64 + frame * 2) / 11 % pal.len() as u64) as usize;
        row(dst, w, y).fill(pal[band]);
        counters.row_op_count += 1;
    }
    counters.palette_step_count += pal.len() as u64;
}

fn draw_rainbow_bands(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    thickness: usize,
    counters: &mut RasterCounters,
) {
    for y in 0..h {
        let p = ((y / thickness + frame as usize / 2) & 31) as u8;
        let c = rainbow(p);
        row(dst, w, y).fill(c);
        counters.row_op_count += 1;
    }
    counters.palette_step_count += 32;
}

fn draw_copper_glow(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut RasterCounters,
) {
    let centers = [
        h as i32 / 5 + tri_i32(frame as i32, 130, h as i32 / 8),
        h as i32 / 2 + tri_i32(frame as i32 + 37, 160, h as i32 / 7),
        h as i32 * 4 / 5 + tri_i32(frame as i32 + 91, 190, h as i32 / 9),
    ];
    for y in 0..h {
        let mut r = 4u8;
        let mut g = 2u8;
        let mut b = 20u8;
        for (i, center) in centers.iter().enumerate() {
            let d = (y as i32 - *center).abs();
            let glow = (42 - d).max(0) as u8;
            r = r.saturating_add(glow.saturating_mul(if i == 0 { 5 } else { 3 }));
            g = g.saturating_add(glow.saturating_mul(if i == 1 { 4 } else { 2 }));
            b = b.saturating_add(glow.saturating_mul(if i == 2 { 4 } else { 1 }));
        }
        row(dst, w, y).fill(color(r, g, b));
        counters.row_op_count += 1;
    }
}

fn draw_screen_flash(dst: &mut [CameraPixel], frame: u64, counters: &mut RasterCounters) {
    let cycle = frame % 96;
    if cycle > 22 {
        return;
    }
    let amount = (255 - cycle as u8 * 10).max(35);
    let flash = if cycle < 8 {
        color(255, 255, 255)
    } else {
        color(amount, 20, 20)
    };
    dst.fill(flash);
    counters.flash_pixel_count += dst.len() as u64;
}

fn screen_flash_active(frame: u64) -> bool {
    frame % 96 <= 22
}

fn draw_action_flash_scene(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut RasterCounters,
) {
    draw_palette_gradient_sky(dst, w, h, frame, counters);
    let cx = w as i32 / 2;
    let cy = h as i32 / 2;
    let r = h.min(w) as i32 / 8;
    for i in 0..12 {
        let a = i * 30 + frame as i32 * 5;
        let ex = cx + tri_i32(a, 180, r * 3);
        let ey = cy + tri_i32(a + 45, 160, r * 2);
        draw_line(dst, w, h, cx, cy, ex, ey, color(255, 240, 120));
    }
    fill_disc(dst, w, h, cx, cy, r, color(255, 80, 40));
}

fn fade_palette(frame: u64, counters: &mut RasterCounters) -> [CameraPixel; 32] {
    let fade = ((frame % 180) as i32 - 90).unsigned_abs() as u8;
    let mut pal = [CameraPixel(0); 32];
    for (idx, out) in pal.iter_mut().enumerate() {
        let base = rainbow(idx as u8);
        let target = color((idx * 7) as u8, (idx * 3 + 70) as u8, (idx * 5 + 30) as u8);
        *out = blend(base, target, fade.saturating_mul(2));
    }
    counters.palette_step_count += 64;
    pal
}

fn day_night_palette(frame: u64, counters: &mut RasterCounters) -> [CameraPixel; 32] {
    let mix = ((frame % 240) as i32 - 120).unsigned_abs() as u8;
    let mut pal = [CameraPixel(0); 32];
    for (idx, out) in pal.iter_mut().enumerate() {
        let day = color(20 + (idx * 5) as u8, 115 + (idx * 4) as u8, 180);
        let night = color(2 + (idx / 3) as u8, 8 + idx as u8, 35 + (idx * 3) as u8);
        *out = blend(day, night, 255 - mix.saturating_mul(2));
    }
    counters.palette_step_count += 64;
    pal
}

fn draw_posterize_transition(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    images: &[CameraImage],
    frame: u64,
    counters: &mut RasterCounters,
) {
    let source = images.first();
    let levels = [2u8, 3, 4, 6, 8, 12, 16, 24][(frame as usize / 18) & 7];
    for y in (0..h).step_by(8) {
        for x in (0..w).step_by(8) {
            let src = sample_image_or_pattern(source, x + frame as usize, y, w, h);
            let q = quantize(src, levels);
            fill_block(dst, w, h, x, y, 8, 8, q);
            counters.lut_lookup_count += 1;
            counters.indexed_pixel_count += 64;
        }
    }
}

fn draw_dither_dissolve(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    images: &[CameraImage],
    frame: u64,
    counters: &mut RasterCounters,
) {
    let a = images.first();
    let b = images.get(1);
    let threshold = (frame % 96) as u8;
    for y in (0..h).step_by(4) {
        for x in (0..w).step_by(4) {
            let d = BAYER_8[(y & 7) * 8 + (x & 7)] * 6;
            let px = if d <= threshold {
                sample_image_or_pattern(b, x + frame as usize / 2, y, w, h)
            } else {
                sample_image_or_pattern(a, x, y + frame as usize / 3, w, h)
            };
            fill_block(dst, w, h, x, y, 4, 4, px);
            counters.dither_pixel_count += 16;
        }
    }
}

fn draw_checker_dissolve(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    images: &[CameraImage],
    frame: u64,
    counters: &mut RasterCounters,
) {
    let a = images.first();
    let b = images.get(1);
    let gate = (frame / 3) as usize % 32;
    for y in (0..h).step_by(4) {
        for x in (0..w).step_by(4) {
            let rank = ((x / 4) + (y / 4) * 3 + ((x ^ y) / 16)) & 31;
            let px = if rank <= gate {
                sample_image_or_pattern(b, x, y + frame as usize, w, h)
            } else {
                sample_image_or_pattern(a, x + frame as usize, y, w, h)
            };
            fill_block(dst, w, h, x, y, 4, 4, px);
            counters.dither_pixel_count += 16;
        }
    }
}

fn draw_crt_phosphor(
    dst: &mut [CameraPixel],
    trail: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut RasterCounters,
) {
    for px in trail.iter_mut() {
        *px = fade_pixel_fast(*px, 214);
    }
    let cx = w as i32 / 2 + tri_i32(frame as i32 * 2, 210, w as i32 / 3);
    let cy = h as i32 / 2 + tri_i32(frame as i32 + 30, 150, h as i32 / 4);
    fill_disc(
        trail,
        w,
        h,
        cx,
        cy,
        h.min(w) as i32 / 15,
        color(80, 255, 140),
    );
    draw_line(
        trail,
        w,
        h,
        w as i32 / 2,
        h as i32 / 2,
        cx,
        cy,
        color(20, 180, 80),
    );
    dst.copy_from_slice(trail);
    counters.trail_pixel_count += trail.len() as u64;
}

fn draw_scanline_overlay(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut RasterCounters,
) {
    for y in (0..h).step_by(3) {
        let amount = 45 + ((frame as usize + y) & 31) as u8;
        for px in row(dst, w, y).iter_mut() {
            *px = fade_pixel_fast(*px, 255 - amount);
        }
        counters.row_op_count += 1;
    }
}

fn draw_scanline_brightness_pulse(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut RasterCounters,
) {
    for y in 0..h {
        let amount = 180 + (((y as u64 * 3 + frame * 5) & 63) as u8);
        row(dst, w, y).fill(color(amount / 3, amount / 2, amount));
        counters.row_op_count += 1;
    }
}

fn draw_warning_split_tint(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut RasterCounters,
) {
    let split = ((frame as usize * 7) % (w + 80)).saturating_sub(40);
    for y in 0..h {
        let row = row(dst, w, y);
        let left = split.min(w);
        row[..left].fill(color(230, 30, 28));
        row[left..].fill(color(255, 210, 40));
        counters.row_op_count += 1;
    }
    counters.palette_step_count += 2;
}

fn draw_water_reflection(
    dst: &mut [CameraPixel],
    src: &[CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut RasterCounters,
) {
    let horizon = h / 2;
    if horizon >= h {
        return;
    }
    for y in (horizon..h).step_by(2) {
        let reflected_y = horizon.saturating_sub(y - horizon + 1);
        let wave = tri_i32(frame as i32 + y as i32 * 3, 64, 10);
        for x in (0..w).step_by(4) {
            let sx = (x as i32 + wave).rem_euclid(w as i32) as usize;
            let src_px = src[reflected_y * w + sx];
            fill_block(dst, w, h, x, y, 4, 2, blue_reflection(src_px));
        }
        counters.reflection_row_count += 2;
    }
    for y in (horizon..h).step_by(4) {
        row(dst, w, y).fill(color(40, 130, 180));
        counters.row_op_count += 1;
    }
}

fn draw_arcade_scene(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    images: &[CameraImage],
    frame: u64,
) {
    let source = images.get((frame as usize / 90) % images.len().max(1));
    for y in (0..h).step_by(4) {
        for x in (0..w).step_by(4) {
            let px = sample_image_or_pattern(source, x + frame as usize, y, w, h);
            fill_block(dst, w, h, x, y, 4, 4, px);
        }
    }
    draw_skyline_plain(dst, w, h, frame);
}

fn draw_skyline(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut RasterCounters,
) {
    draw_skyline_plain(dst, w, h, frame);
    counters.row_op_count += 24;
}

fn draw_skyline_plain(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    let base = h as isize * 3 / 4;
    for i in 0..18 {
        let bw = (w / 30 + (i * 7) % (w.max(30) / 18).max(1)).max(6);
        let bh = h / 7 + (i * 17) % (h.max(20) / 3).max(1);
        let x = ((i * w / 16 + frame as usize / 3) % (w + bw)) as isize - bw as isize;
        fill_rect(dst, w, h, x, base - bh as isize, bw, bh, color(4, 5, 15));
        for wy in (base - bh as isize + 7..base).step_by(13) {
            for wx in (x + 5..x + bw as isize - 4).step_by(14) {
                if ((wx + wy + frame as isize) & 3) == 0 {
                    fill_rect(dst, w, h, wx, wy, 3, 3, color(255, 220, 90));
                }
            }
        }
    }
}

fn draw_raster_grid(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut RasterCounters,
) {
    let grid = color(0, 255, 210);
    for y in (h / 2..h).step_by(18) {
        let wobble = tri_i32(frame as i32 + y as i32, 48, 8);
        let yy = (y as i32 + wobble).clamp(0, h.saturating_sub(1) as i32) as usize;
        row(dst, w, yy).fill(grid);
        counters.row_op_count += 1;
    }
    for x in (0..w).step_by(64) {
        draw_line(
            dst,
            w,
            h,
            w as i32 / 2,
            h as i32 / 2,
            x as i32,
            h as i32 - 1,
            grid,
        );
    }
}

fn sample_image_or_pattern(
    image: Option<&CameraImage>,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) -> CameraPixel {
    if let Some(image) = image {
        let sx = x * image.w / w.max(1);
        let sy = y * image.h / h.max(1);
        return image.pixels[(sy % image.h) * image.stride + (sx % image.w)];
    }
    let r = ((x * 3 + y + w) & 255) as u8;
    let g = ((y * 4 + x / 2 + h) & 255) as u8;
    let b = (((x ^ y) * 2) & 255) as u8;
    color(r, g, b)
}

fn quantize(px: CameraPixel, levels: u8) -> CameraPixel {
    let rgb = pixel_to_rgb888(px);
    let step = (256 / levels.max(2) as u32).max(1);
    let q = |v: u32| ((v / step) * step).min(255) as u8;
    color(q((rgb >> 16) & 255), q((rgb >> 8) & 255), q(rgb & 255))
}

fn blend(a: CameraPixel, b: CameraPixel, amount: u8) -> CameraPixel {
    let ar = ((a.0 >> 11) & 31) as u32 * 255 / 31;
    let ag = ((a.0 >> 5) & 63) as u32 * 255 / 63;
    let ab = (a.0 & 31) as u32 * 255 / 31;
    let br = ((b.0 >> 11) & 31) as u32 * 255 / 31;
    let bg = ((b.0 >> 5) & 63) as u32 * 255 / 63;
    let bb = (b.0 & 31) as u32 * 255 / 31;
    let inv = 255 - amount as u32;
    color(
        ((ar * inv + br * amount as u32) / 255) as u8,
        ((ag * inv + bg * amount as u32) / 255) as u8,
        ((ab * inv + bb * amount as u32) / 255) as u8,
    )
}

fn fade_pixel_fast(px: CameraPixel, amount: u8) -> CameraPixel {
    let r = (((px.0 >> 11) & 31) as u32 * amount as u32 / 255) as u16;
    let g = (((px.0 >> 5) & 63) as u32 * amount as u32 / 255) as u16;
    let b = ((px.0 & 31) as u32 * amount as u32 / 255) as u16;
    CameraPixel((r << 11) | (g << 5) | b)
}

fn blue_reflection(px: CameraPixel) -> CameraPixel {
    let r = ((px.0 >> 11) & 31) / 3;
    let g = ((px.0 >> 5) & 63) / 2 + 8;
    let b = (px.0 & 31).saturating_add(10).min(31);
    CameraPixel((r << 11) | (g.min(63) << 5) | b)
}

fn rainbow(p: u8) -> CameraPixel {
    let p = p & 31;
    match p / 6 {
        0 => color(255, p * 38, 30),
        1 => color(255 - (p - 6) * 30, 255, 30),
        2 => color(20, 255, (p - 12) * 38),
        3 => color(20, 255 - (p - 18) * 36, 255),
        4 => color((p - 24) * 38, 30, 255),
        _ => color(255, 30, 180),
    }
}

fn tri_i32(t: i32, period: i32, amp: i32) -> i32 {
    if period <= 0 || amp <= 0 {
        return 0;
    }
    let half = period / 2;
    let v = t.rem_euclid(period);
    let slope = if v < half { v } else { period - v };
    slope * amp * 2 / half.max(1) - amp
}

fn row(dst: &mut [CameraPixel], w: usize, y: usize) -> &mut [CameraPixel] {
    &mut dst[y * w..(y + 1) * w]
}

fn fill_block(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    bw: usize,
    bh: usize,
    c: CameraPixel,
) {
    for yy in y..(y + bh).min(h) {
        let start = yy * w + x.min(w);
        let end = yy * w + (x + bw).min(w);
        if end > start {
            dst[start..end].fill(c);
        }
    }
}

fn fill_rect(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    x: isize,
    y: isize,
    rw: usize,
    rh: usize,
    c: CameraPixel,
) {
    let x0 = x.clamp(0, w as isize) as usize;
    let y0 = y.clamp(0, h as isize) as usize;
    let x1 = (x + rw as isize).clamp(0, w as isize) as usize;
    let y1 = (y + rh as isize).clamp(0, h as isize) as usize;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    for yy in y0..y1 {
        dst[yy * w + x0..yy * w + x1].fill(c);
    }
}

fn fill_disc(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    cx: i32,
    cy: i32,
    r: i32,
    c: CameraPixel,
) {
    let r2 = r * r;
    for y in (cy - r).max(0)..=(cy + r).min(h as i32 - 1) {
        for x in (cx - r).max(0)..=(cx + r).min(w as i32 - 1) {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r2 {
                dst[y as usize * w + x as usize] = c;
            }
        }
    }
}

fn draw_line(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    c: CameraPixel,
) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        if x0 >= 0 && y0 >= 0 && x0 < w as i32 && y0 < h as i32 {
            dst[y0 as usize * w + x0 as usize] = c;
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn draw_label(dst: &mut [CameraPixel], w: usize, h: usize, text: &str) {
    let bg = color(0, 0, 0);
    let fg = color(255, 245, 170);
    let max_chars = ((w.saturating_sub(16)) / 8).max(1);
    let label = if text.len() > max_chars {
        &text[..max_chars]
    } else {
        text
    };
    fill_rect(dst, w, h, 8, 8, label.len() * 8 + 10, 18, bg);
    for (i, ch) in label.bytes().enumerate() {
        draw_char(dst, w, h, 13 + i * 8, 13, ch, fg);
    }
}

fn draw_char(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    ch: u8,
    c: CameraPixel,
) {
    let seed = ch.wrapping_mul(37);
    for yy in 0..7 {
        for xx in 0..5 {
            let on = if ch == b' ' {
                false
            } else {
                ((seed.rotate_left(yy as u32) ^ (xx as u8 * 19) ^ (yy as u8 * 11)) & 0x18) != 0
                    || xx == 0
                    || yy == 0
            };
            if on && x + xx < w && y + yy < h {
                dst[(y + yy) * w + x + xx] = c;
            }
        }
    }
}

const BAYER_8: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46, 6, 38, 60,
    28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27, 49, 17, 57, 25, 15,
    47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(pixels: &[CameraPixel]) -> u64 {
        let mut h = 0xcbf29ce484222325u64;
        for px in pixels {
            h ^= px.0 as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    fn nonblank(pixels: &[CameraPixel]) -> bool {
        pixels.iter().any(|px| px.0 != pixels[0].0)
    }

    #[test]
    fn labels_parse_and_order() {
        assert_eq!(RasterEffectKind::all().len(), 15);
        assert_eq!(
            RasterEffectKind::all()[0].label(),
            "palette-cycling-lava-water-neon"
        );
        assert_eq!(
            RasterEffectKind::all()[14].label(),
            "water-reflection-flipped-wavy-rows"
        );
        for kind in RasterEffectKind::all() {
            assert_eq!(RasterEffectKind::parse(kind.label()), Some(*kind));
            assert_eq!(
                RasterEffectKind::parse(&kind.label().replace('-', "_")),
                Some(*kind)
            );
        }
        assert!(RasterEffectKind::parse("bogus").is_none());
    }

    #[test]
    fn every_effect_renders_nonblank_and_deterministic() {
        let (w, h) = (96, 54);
        let images = synthetic_raster_images(3);
        for &kind in RasterEffectKind::all() {
            let mut a = vec![CameraPixel(0); w * h];
            let mut b = vec![CameraPixel(0); w * h];
            let mut sa = RasterEffectRenderState::new(w, h);
            let mut sb = RasterEffectRenderState::new(w, h);
            let stats = render_raster_effect_frame(&mut a, &mut sa, w, h, &images, kind, 42, None);
            render_raster_effect_frame(&mut b, &mut sb, w, h, &images, kind, 42, None);
            assert!(nonblank(&a), "{} was blank", kind.label());
            assert_eq!(hash(&a), hash(&b), "{} was not deterministic", kind.label());
            assert_eq!(
                stats.draw_us(),
                stats.clear_us
                    + stats.background_us
                    + stats.projection_us
                    + stats.image_blit_us
                    + stats.sprite_us
                    + stats.post_us
                    + stats.hud_us
            );
        }
    }

    #[test]
    fn every_effect_moves_between_frames() {
        let (w, h) = (96, 54);
        let images = synthetic_raster_images(3);
        for &kind in RasterEffectKind::all() {
            let mut a = vec![CameraPixel(0); w * h];
            let mut b = vec![CameraPixel(0); w * h];
            let mut state = RasterEffectRenderState::new(w, h);
            render_raster_effect_frame(&mut a, &mut state, w, h, &images, kind, 0, None);
            render_raster_effect_frame(&mut b, &mut state, w, h, &images, kind, 60, None);
            assert_ne!(hash(&a), hash(&b), "{} did not visibly move", kind.label());
        }
    }

    #[test]
    fn counters_match_effect_families() {
        let (w, h) = (80, 45);
        let images = synthetic_raster_images(2);
        for &kind in RasterEffectKind::all() {
            let mut frame = vec![CameraPixel(0); w * h];
            let mut state = RasterEffectRenderState::new(w, h);
            let stats =
                render_raster_effect_frame(&mut frame, &mut state, w, h, &images, kind, 7, None);
            let total = stats.palette_step_count
                + stats.lut_lookup_count
                + stats.row_op_count
                + stats.dither_pixel_count
                + stats.flash_pixel_count
                + stats.trail_pixel_count
                + stats.indexed_pixel_count
                + stats.reflection_row_count;
            assert!(total > 0, "{} had no raster counters", kind.label());
        }
    }

    #[test]
    fn trail_retains_previous_light() {
        let (w, h) = (96, 54);
        let images = synthetic_raster_images(1);
        let mut frame = vec![CameraPixel(0); w * h];
        let mut state = RasterEffectRenderState::new(w, h);
        render_raster_effect_frame(
            &mut frame,
            &mut state,
            w,
            h,
            &images,
            RasterEffectKind::CrtPhosphorFadeTrail,
            0,
            None,
        );
        let h0 = hash(&frame);
        render_raster_effect_frame(
            &mut frame,
            &mut state,
            w,
            h,
            &images,
            RasterEffectKind::CrtPhosphorFadeTrail,
            1,
            None,
        );
        assert_ne!(h0, hash(&frame));
        assert!(frame.iter().filter(|px| px.0 != 0).count() > 20);
    }

    #[test]
    fn small_sizes_do_not_panic() {
        let images = synthetic_raster_images(1);
        for &kind in RasterEffectKind::all() {
            for &(w, h) in &[(1, 1), (7, 5), (16, 9)] {
                let mut frame = vec![CameraPixel(0); w * h];
                let mut state = RasterEffectRenderState::new(w, h);
                render_raster_effect_frame(
                    &mut frame,
                    &mut state,
                    w,
                    h,
                    &images,
                    kind,
                    3,
                    Some("x"),
                );
            }
        }
    }
}
