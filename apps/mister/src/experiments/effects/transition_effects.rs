// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-testable classic arcade screen transition effects.
#![allow(clippy::too_many_arguments)]

use std::time::Instant;

use super::camera_effects::{CameraImage, CameraPixel, color, synthetic_images};
use super::render_helpers::{clear, elapsed_us, time};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionEffectKind {
    VenetianBlindsWipe,
    HorizontalShutterClose,
    VerticalArcadeDoorClose,
    IrisCircleOpenClose,
    RadialSpokeWipe,
    MosaicBlockInOut,
    TilemapPageFlip,
    CheckerboardReveal,
    StarfieldWarpTransition,
    TunnelZoomTransition,
    ScreenShakeFlash,
    CrtPowerOffVerticalCollapse,
    CrtPowerOffFastSnap,
    CrtPowerOffHotLine,
    CrtPowerOffCenterDot,
    CrtPowerOffPhosphorBloom,
    CrtPowerOffWobbleCollapse,
    BurnInGhostCrossfade,
    GlitchySpritePriorityReveal,
    CabinetMarqueeLightSweep,
}

impl TransitionEffectKind {
    pub const ALL: [Self; 20] = [
        Self::VenetianBlindsWipe,
        Self::HorizontalShutterClose,
        Self::VerticalArcadeDoorClose,
        Self::IrisCircleOpenClose,
        Self::RadialSpokeWipe,
        Self::MosaicBlockInOut,
        Self::TilemapPageFlip,
        Self::CheckerboardReveal,
        Self::StarfieldWarpTransition,
        Self::TunnelZoomTransition,
        Self::ScreenShakeFlash,
        Self::CrtPowerOffVerticalCollapse,
        Self::CrtPowerOffFastSnap,
        Self::CrtPowerOffHotLine,
        Self::CrtPowerOffCenterDot,
        Self::CrtPowerOffPhosphorBloom,
        Self::CrtPowerOffWobbleCollapse,
        Self::BurnInGhostCrossfade,
        Self::GlitchySpritePriorityReveal,
        Self::CabinetMarqueeLightSweep,
    ];

    pub fn all() -> &'static [Self] {
        &Self::ALL
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::VenetianBlindsWipe => "venetian-blinds-wipe",
            Self::HorizontalShutterClose => "horizontal-shutter-close",
            Self::VerticalArcadeDoorClose => "vertical-arcade-door-close",
            Self::IrisCircleOpenClose => "iris-circle-open-close",
            Self::RadialSpokeWipe => "radial-spoke-wipe",
            Self::MosaicBlockInOut => "mosaic-block-in-out",
            Self::TilemapPageFlip => "tilemap-page-flip",
            Self::CheckerboardReveal => "checkerboard-reveal",
            Self::StarfieldWarpTransition => "starfield-warp-transition",
            Self::TunnelZoomTransition => "tunnel-zoom-transition",
            Self::ScreenShakeFlash => "screen-shake-flash",
            Self::CrtPowerOffVerticalCollapse => "crt-power-off-vertical-collapse",
            Self::CrtPowerOffFastSnap => "crt-power-off-fast-snap",
            Self::CrtPowerOffHotLine => "crt-power-off-hot-line",
            Self::CrtPowerOffCenterDot => "crt-power-off-center-dot",
            Self::CrtPowerOffPhosphorBloom => "crt-power-off-phosphor-bloom",
            Self::CrtPowerOffWobbleCollapse => "crt-power-off-wobble-collapse",
            Self::BurnInGhostCrossfade => "burn-in-ghost-crossfade",
            Self::GlitchySpritePriorityReveal => "glitchy-sprite-priority-reveal",
            Self::CabinetMarqueeLightSweep => "cabinet-marquee-light-sweep",
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
pub struct TransitionEffectFrameStats {
    pub clear_us: u64,
    pub background_us: u64,
    pub projection_us: u64,
    pub image_blit_us: u64,
    pub sprite_us: u64,
    pub post_us: u64,
    pub hud_us: u64,
    pub mask_cell_count: u64,
    pub revealed_pixel_count: u64,
    pub hidden_pixel_count: u64,
    pub source_a_pixel_count: u64,
    pub source_b_pixel_count: u64,
    pub shake_offset_px: u64,
    pub flash_pixel_count: u64,
    pub warp_sample_count: u64,
    pub ghost_pixel_count: u64,
    pub glitch_band_count: u64,
}

impl TransitionEffectFrameStats {
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
struct TransitionCounters {
    mask_cell_count: u64,
    revealed_pixel_count: u64,
    hidden_pixel_count: u64,
    source_a_pixel_count: u64,
    source_b_pixel_count: u64,
    shake_offset_px: u64,
    flash_pixel_count: u64,
    warp_sample_count: u64,
    ghost_pixel_count: u64,
    glitch_band_count: u64,
}

pub struct TransitionEffectRenderState {
    source_a: Vec<CameraPixel>,
    source_b: Vec<CameraPixel>,
    scratch: Vec<CameraPixel>,
    ghost: Vec<CameraPixel>,
    w: usize,
    h: usize,
}

impl TransitionEffectRenderState {
    pub fn new(w: usize, h: usize) -> Self {
        let mut state = Self {
            source_a: vec![CameraPixel(0); w * h],
            source_b: vec![CameraPixel(0); w * h],
            scratch: vec![CameraPixel(0); w * h],
            ghost: vec![CameraPixel(0); w * h],
            w,
            h,
        };
        state.rebuild_sources(&synthetic_transition_images(2));
        state
    }

    fn resize(&mut self, w: usize, h: usize, images: &[CameraImage]) {
        if self.w == w && self.h == h {
            return;
        }
        self.source_a.resize(w * h, CameraPixel(0));
        self.source_b.resize(w * h, CameraPixel(0));
        self.scratch.resize(w * h, CameraPixel(0));
        self.ghost.resize(w * h, CameraPixel(0));
        self.w = w;
        self.h = h;
        self.rebuild_sources(images);
    }

    fn rebuild_sources(&mut self, images: &[CameraImage]) {
        fill_source(&mut self.source_a, self.w, self.h, images.first(), 0);
        fill_source(&mut self.source_b, self.w, self.h, images.get(1), 1);
        self.ghost.copy_from_slice(&self.source_a);
    }
}

pub fn render_transition_effect_frame(
    dst: &mut [CameraPixel],
    state: &mut TransitionEffectRenderState,
    w: usize,
    h: usize,
    images: &[CameraImage],
    kind: TransitionEffectKind,
    frame: u64,
    hud: Option<&str>,
) -> TransitionEffectFrameStats {
    assert_eq!(dst.len(), w * h);
    state.resize(w, h, images);
    let mut stats = TransitionEffectFrameStats::default();
    let mut counters = TransitionCounters::default();
    let progress = ((frame % 120) as usize * 256 / 119).min(255);

    let t = Instant::now();
    clear(dst, color(0, 0, 0));
    stats.clear_us = elapsed_us(t);

    match kind {
        TransitionEffectKind::VenetianBlindsWipe => time(&mut stats.image_blit_us, || {
            draw_venetian(
                dst,
                &state.source_a,
                &state.source_b,
                w,
                h,
                progress,
                &mut counters,
            )
        }),
        TransitionEffectKind::HorizontalShutterClose => time(&mut stats.image_blit_us, || {
            draw_horizontal_shutter(
                dst,
                &state.source_a,
                &state.source_b,
                w,
                h,
                progress,
                &mut counters,
            )
        }),
        TransitionEffectKind::VerticalArcadeDoorClose => time(&mut stats.image_blit_us, || {
            draw_vertical_doors(
                dst,
                &state.source_a,
                &state.source_b,
                w,
                h,
                progress,
                &mut counters,
            )
        }),
        TransitionEffectKind::IrisCircleOpenClose => time(&mut stats.projection_us, || {
            draw_iris(
                dst,
                &state.source_a,
                &state.source_b,
                w,
                h,
                progress,
                &mut counters,
            )
        }),
        TransitionEffectKind::RadialSpokeWipe => time(&mut stats.projection_us, || {
            draw_radial_spokes(
                dst,
                &state.source_a,
                &state.source_b,
                w,
                h,
                progress,
                &mut counters,
            )
        }),
        TransitionEffectKind::MosaicBlockInOut => time(&mut stats.image_blit_us, || {
            draw_mosaic(
                dst,
                &state.source_a,
                &state.source_b,
                w,
                h,
                progress,
                &mut counters,
            )
        }),
        TransitionEffectKind::TilemapPageFlip => time(&mut stats.projection_us, || {
            draw_page_flip(
                dst,
                &state.source_a,
                &state.source_b,
                w,
                h,
                progress,
                &mut counters,
            )
        }),
        TransitionEffectKind::CheckerboardReveal => time(&mut stats.image_blit_us, || {
            draw_checkerboard(
                dst,
                &state.source_a,
                &state.source_b,
                w,
                h,
                progress,
                &mut counters,
            )
        }),
        TransitionEffectKind::StarfieldWarpTransition => time(&mut stats.projection_us, || {
            draw_starfield_warp(
                dst,
                &state.source_a,
                &state.source_b,
                w,
                h,
                frame,
                &mut counters,
            )
        }),
        TransitionEffectKind::TunnelZoomTransition => time(&mut stats.projection_us, || {
            draw_tunnel_zoom(
                dst,
                &state.source_a,
                &state.source_b,
                w,
                h,
                frame,
                &mut counters,
            )
        }),
        TransitionEffectKind::ScreenShakeFlash => {
            time(&mut stats.image_blit_us, || {
                draw_screen_shake(
                    dst,
                    &state.source_a,
                    &state.source_b,
                    w,
                    h,
                    frame,
                    &mut counters,
                )
            });
            time(&mut stats.post_us, || draw_flash(dst, frame, &mut counters));
        }
        TransitionEffectKind::CrtPowerOffVerticalCollapse => time(&mut stats.projection_us, || {
            draw_crt_collapse(
                dst,
                &state.source_a,
                w,
                h,
                progress,
                frame,
                CrtPowerOffStyle::Balanced,
                &mut counters,
            )
        }),
        TransitionEffectKind::CrtPowerOffFastSnap => time(&mut stats.projection_us, || {
            draw_crt_collapse(
                dst,
                &state.source_a,
                w,
                h,
                progress,
                frame,
                CrtPowerOffStyle::FastSnap,
                &mut counters,
            )
        }),
        TransitionEffectKind::CrtPowerOffHotLine => time(&mut stats.projection_us, || {
            draw_crt_collapse(
                dst,
                &state.source_a,
                w,
                h,
                progress,
                frame,
                CrtPowerOffStyle::HotLine,
                &mut counters,
            )
        }),
        TransitionEffectKind::CrtPowerOffCenterDot => time(&mut stats.projection_us, || {
            draw_crt_collapse(
                dst,
                &state.source_a,
                w,
                h,
                progress,
                frame,
                CrtPowerOffStyle::CenterDot,
                &mut counters,
            )
        }),
        TransitionEffectKind::CrtPowerOffPhosphorBloom => time(&mut stats.projection_us, || {
            draw_crt_collapse(
                dst,
                &state.source_a,
                w,
                h,
                progress,
                frame,
                CrtPowerOffStyle::PhosphorBloom,
                &mut counters,
            )
        }),
        TransitionEffectKind::CrtPowerOffWobbleCollapse => time(&mut stats.projection_us, || {
            draw_crt_collapse(
                dst,
                &state.source_a,
                w,
                h,
                progress,
                frame,
                CrtPowerOffStyle::Wobble,
                &mut counters,
            )
        }),
        TransitionEffectKind::BurnInGhostCrossfade => time(&mut stats.post_us, || {
            draw_burn_in_ghost(
                dst,
                &mut state.ghost,
                &state.source_a,
                &state.source_b,
                w,
                h,
                progress,
                &mut counters,
            )
        }),
        TransitionEffectKind::GlitchySpritePriorityReveal => {
            time(&mut stats.image_blit_us, || {
                draw_glitch_reveal(
                    dst,
                    &state.source_a,
                    &state.source_b,
                    w,
                    h,
                    frame,
                    &mut counters,
                )
            });
            time(&mut stats.sprite_us, || {
                draw_priority_sprites(dst, w, h, frame, &mut counters)
            });
        }
        TransitionEffectKind::CabinetMarqueeLightSweep => {
            time(&mut stats.background_us, || {
                draw_marquee_base(dst, &state.source_a, w, h, &mut counters)
            });
            time(&mut stats.post_us, || {
                draw_marquee_sweep(dst, w, h, frame, &mut counters)
            });
        }
    }

    stats.mask_cell_count = counters.mask_cell_count;
    stats.revealed_pixel_count = counters.revealed_pixel_count;
    stats.hidden_pixel_count = counters.hidden_pixel_count;
    stats.source_a_pixel_count = counters.source_a_pixel_count;
    stats.source_b_pixel_count = counters.source_b_pixel_count;
    stats.shake_offset_px = counters.shake_offset_px;
    stats.flash_pixel_count = counters.flash_pixel_count;
    stats.warp_sample_count = counters.warp_sample_count;
    stats.ghost_pixel_count = counters.ghost_pixel_count;
    stats.glitch_band_count = counters.glitch_band_count;

    if let Some(text) = hud {
        time(&mut stats.hud_us, || draw_label(dst, w, h, text));
    }

    stats
}

pub fn synthetic_transition_images(count: usize) -> Vec<CameraImage> {
    synthetic_images(count)
}

fn fill_source(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    image: Option<&CameraImage>,
    variant: usize,
) {
    for y in (0..h).step_by(4) {
        for x in (0..w).step_by(4) {
            let px = sample_image_or_pattern(image, x + variant * 23, y + variant * 17, w, h);
            fill_block(dst, w, h, x, y, 4, 4, px);
        }
    }
    draw_source_decoration(dst, w, h, variant);
}

fn draw_source_decoration(dst: &mut [CameraPixel], w: usize, h: usize, variant: usize) {
    let c1 = if variant == 0 {
        color(255, 220, 70)
    } else {
        color(70, 230, 255)
    };
    let c2 = if variant == 0 {
        color(180, 40, 40)
    } else {
        color(60, 70, 220)
    };
    let band_h = (h / 12).max(2);
    for i in 0..6 {
        let y = (i * h / 6 + variant * 17) % h.max(1);
        fill_rect(
            dst,
            w,
            h,
            0,
            y as isize,
            w,
            band_h,
            if i & 1 == 0 { c1 } else { c2 },
        );
    }
    for i in 0..12 {
        let x = (i * w / 12 + variant * 31) % w.max(1);
        let rh = h / 4 + (i * 13) % (h / 3).max(1);
        fill_rect(
            dst,
            w,
            h,
            x as isize,
            h as isize - rh as isize,
            (w / 28).max(3),
            rh,
            color(8 + variant as u8 * 18, 8, 28 + variant as u8 * 22),
        );
    }
}

fn draw_venetian(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    h: usize,
    progress: usize,
    counters: &mut TransitionCounters,
) {
    let slat = 18usize;
    let open = slat * progress / 255;
    for y in 0..h {
        let reveal = y % slat < open;
        let src = if reveal { b } else { a };
        copy_row(dst, src, w, y);
        count_row(reveal, w, counters);
        counters.mask_cell_count += (w / slat.max(1)).max(1) as u64;
    }
}

fn draw_horizontal_shutter(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    h: usize,
    progress: usize,
    counters: &mut TransitionCounters,
) {
    dst.copy_from_slice(b);
    counters.source_b_pixel_count += dst.len() as u64;
    let half = h / 2;
    let closed = half * progress / 255;
    for y in 0..closed {
        copy_row(dst, a, w, y);
        copy_row(dst, a, w, h - 1 - y);
        counters.hidden_pixel_count += (w * 2) as u64;
        counters.source_a_pixel_count += (w * 2) as u64;
        counters.mask_cell_count += 2;
    }
}

fn draw_vertical_doors(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    h: usize,
    progress: usize,
    counters: &mut TransitionCounters,
) {
    dst.copy_from_slice(b);
    counters.source_b_pixel_count += dst.len() as u64;
    let door = (w / 2) * progress / 255;
    for y in 0..h {
        let row = &mut dst[y * w..(y + 1) * w];
        let arow = &a[y * w..(y + 1) * w];
        row[..door].copy_from_slice(&arow[..door]);
        row[w - door..].copy_from_slice(&arow[w - door..]);
        counters.hidden_pixel_count += (door * 2) as u64;
        counters.source_a_pixel_count += (door * 2) as u64;
        counters.mask_cell_count += 2;
    }
}

fn draw_iris(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    h: usize,
    progress: usize,
    counters: &mut TransitionCounters,
) {
    let cx = w as i32 / 2;
    let cy = h as i32 / 2;
    let max_r2 = (cx * cx + cy * cy).max(1);
    let r = ((max_r2 as i64 * progress as i64 / 255) as f64).sqrt() as i32;
    let r2 = r * r;
    for y in (0..h).step_by(4) {
        for x in (0..w).step_by(4) {
            let dx = x as i32 + 2 - cx;
            let dy = y as i32 + 2 - cy;
            let reveal = dx * dx + dy * dy <= r2;
            let px = block_pick(a, b, w, x, y, reveal);
            fill_block(dst, w, h, x, y, 4, 4, px);
            count_block(reveal, 16, counters);
            counters.mask_cell_count += 1;
        }
    }
}

fn draw_radial_spokes(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    h: usize,
    progress: usize,
    counters: &mut TransitionCounters,
) {
    let cx = w as i32 / 2;
    let cy = h as i32 / 2;
    let threshold = progress as i32 * 16 / 255;
    for y in (0..h).step_by(4) {
        for x in (0..w).step_by(4) {
            let dx = x as i32 + 2 - cx;
            let dy = y as i32 + 2 - cy;
            let spoke = (dy.abs() * 7 + dx.abs() * 5 + if dx ^ dy < 0 { 8 } else { 0 }) & 15;
            let reveal = spoke <= threshold;
            let px = block_pick(a, b, w, x, y, reveal);
            fill_block(dst, w, h, x, y, 4, 4, px);
            count_block(reveal, 16, counters);
            counters.mask_cell_count += 1;
        }
    }
}

fn draw_mosaic(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    h: usize,
    progress: usize,
    counters: &mut TransitionCounters,
) {
    let block = 4 + ((255 - progress) / 28) * 4;
    let gate = progress * 31 / 255;
    for y in (0..h).step_by(block.max(1)) {
        for x in (0..w).step_by(block.max(1)) {
            let rank = ((x / block + y / block * 3 + (x ^ y) / 29) & 31).min(31);
            let reveal = rank <= gate;
            let px = block_pick(a, b, w, x, y, reveal);
            fill_block(dst, w, h, x, y, block, block, px);
            count_block(reveal, block * block, counters);
            counters.mask_cell_count += 1;
        }
    }
}

fn draw_page_flip(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    h: usize,
    progress: usize,
    counters: &mut TransitionCounters,
) {
    dst.copy_from_slice(a);
    counters.source_a_pixel_count += dst.len() as u64;
    let page_w = w * progress / 255;
    let fold = (w / 12).max(4);
    for y in 0..h {
        let row = &mut dst[y * w..(y + 1) * w];
        let brow = &b[y * w..(y + 1) * w];
        let upto = page_w.min(w);
        row[..upto].copy_from_slice(&brow[..upto]);
        if upto < w {
            let shade_end = (upto + fold).min(w);
            for px in &mut row[upto..shade_end] {
                *px = fade_pixel_fast(*px, 140);
            }
        }
    }
    counters.revealed_pixel_count += (page_w.min(w) * h) as u64;
    counters.source_b_pixel_count += (page_w.min(w) * h) as u64;
    counters.mask_cell_count += h as u64;
}

fn draw_checkerboard(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    h: usize,
    progress: usize,
    counters: &mut TransitionCounters,
) {
    let block = 16usize;
    let gate = progress * 31 / 255;
    for y in (0..h).step_by(block) {
        for x in (0..w).step_by(block) {
            let rank = ((x / block) + (y / block) * 5) & 31;
            let reveal = rank <= gate;
            blit_block(dst, if reveal { b } else { a }, w, h, x, y, block, block);
            count_block(reveal, block * block, counters);
            counters.mask_cell_count += 1;
        }
    }
}

fn draw_starfield_warp(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TransitionCounters,
) {
    blit_blocky_source(dst, b, w, h, 4, counters);
    let cx = w as i32 / 2;
    let cy = h as i32 / 2;
    for i in 0..220 {
        let seed = i * 97 + 13;
        let angle_x = ((seed * 37) % w.max(1)) as i32 - cx;
        let angle_y = ((seed * 53) % h.max(1)) as i32 - cy;
        let z = ((frame as i32 * 7 + seed as i32) & 255) + 24;
        let x = cx + angle_x * 256 / z;
        let y = cy + angle_y * 256 / z;
        let tail = 8 + (255 - z).max(0) / 16;
        draw_line(
            dst,
            w,
            h,
            x,
            y,
            x + angle_x.signum() * tail,
            y + angle_y.signum() * tail,
            color(240, 250, 255),
        );
        counters.warp_sample_count += 1;
    }
    overlay_center_sample(dst, a, w, h, frame as usize / 2, counters);
}

fn draw_tunnel_zoom(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TransitionCounters,
) {
    let cx = w as i32 / 2;
    let cy = h as i32 / 2;
    let zoom = 40 + (frame as usize % 120) * 2;
    for y in (0..h).step_by(8) {
        for x in (0..w).step_by(8) {
            let dx = (x as i32 + 2 - cx).unsigned_abs() as usize;
            let dy = (y as i32 + 2 - cy).unsigned_abs() as usize;
            let dist = dx + dy + 1;
            let sx = (x + zoom * 320 / dist) % w.max(1);
            let sy = (y + zoom * 180 / dist) % h.max(1);
            let source = if ((dist + frame as usize) / 24) & 1 == 0 {
                a
            } else {
                b
            };
            let px = source[sy * w + sx];
            fill_block(dst, w, h, x, y, 8, 8, px);
            counters.warp_sample_count += 1;
        }
    }
}

fn draw_screen_shake(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TransitionCounters,
) {
    let source = if (frame / 30) & 1 == 0 { a } else { b };
    let ox = tri_i32(frame as i32 * 5, 32, 10);
    let oy = tri_i32(frame as i32 * 7, 28, 6);
    for y in (0..h).step_by(2) {
        let sy = (y as i32 + oy).rem_euclid(h as i32) as usize;
        for x in (0..w).step_by(8) {
            let sx = (x as i32 + ox).rem_euclid(w as i32) as usize;
            let px = source[sy * w + sx];
            fill_block(dst, w, h, x, y, 8, 2, px);
        }
    }
    counters.source_a_pixel_count += dst.len() as u64 / 2;
    counters.source_b_pixel_count += dst.len() as u64 / 2;
    counters.shake_offset_px = ox.unsigned_abs() as u64 + oy.unsigned_abs() as u64;
}

fn draw_flash(dst: &mut [CameraPixel], frame: u64, counters: &mut TransitionCounters) {
    if frame % 48 < 8 {
        let flash = if frame % 16 < 8 {
            color(255, 255, 255)
        } else {
            color(255, 40, 30)
        };
        dst.fill(flash);
        counters.flash_pixel_count += dst.len() as u64;
    }
}

fn draw_crt_collapse(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    w: usize,
    h: usize,
    progress: usize,
    frame: u64,
    style: CrtPowerOffStyle,
    counters: &mut TransitionCounters,
) {
    if w == 0 || h == 0 {
        return;
    }
    clear(dst, color(0, 0, 0));

    let params = style.params();
    if params.afterglow && progress >= 88 {
        let glow_phase = phase255(progress, 88, 246);
        draw_crt_phosphor_afterglow(dst, a, w, h, glow_phase, counters);
    }

    let collapse = phase255(progress, params.v_start, params.v_end);
    let pinch = phase255(progress, params.x_start, params.x_end);
    let inv_v = 255usize.saturating_sub(collapse);
    let inv_x = 255usize.saturating_sub(pinch);
    let min_w = (w / params.min_w_div).max(1).min(w);
    let open_h = if progress >= params.image_cutoff {
        1
    } else {
        scale_cubic(h, inv_v).max(2.min(h))
    };
    let open_w = if progress >= params.x_closed_at {
        1
    } else {
        min_w + scale_square(w.saturating_sub(min_w), inv_x)
    }
    .max(1)
    .min(w);

    let cy = h / 2;
    let y0 = cy.saturating_sub(open_h / 2);
    if progress < params.image_cutoff {
        let fade = 250usize
            .saturating_sub(progress * params.fade_loss / 255)
            .clamp(params.min_image_fade, 250) as u8;
        for yy in 0..open_h {
            let dy = (y0 + yy).min(h.saturating_sub(1));
            let sy = (yy * h / open_h.max(1)).min(h.saturating_sub(1));
            let from_center = (yy as isize * 2 + 1 - open_h as isize).unsigned_abs();
            let taper = if open_h > 8 {
                open_w * from_center / open_h.max(1) / params.taper_div
            } else {
                0
            };
            let row_w = open_w.saturating_sub(taper).max(1).min(w.max(1));
            let wobble = if params.wobble {
                tri_i32(
                    frame as i32 * 9 + yy as i32 * 5,
                    32,
                    (w / 82).clamp(2, 14) as i32,
                )
            } else {
                0
            };
            let center_x = ((w / 2) as isize + wobble as isize)
                .clamp(0, w.saturating_sub(1) as isize) as usize;
            let x0 = center_x
                .saturating_sub(row_w / 2)
                .min(w.saturating_sub(row_w));
            let step = if row_w >= 480 {
                4
            } else if row_w >= 120 {
                2
            } else {
                1
            };
            let row_fade = if (dy + progress / 6) & 1 == 0 {
                fade
            } else {
                (fade as usize * 208 / 255) as u8
            };
            for xx in (0..row_w).step_by(step) {
                let sx_base = (xx * w / row_w.max(1)).min(w.saturating_sub(1));
                let sx = if params.wobble {
                    (sx_base as i32 + wobble).rem_euclid(w as i32) as usize
                } else {
                    sx_base
                };
                let px = fade_pixel_fast(a[sy * w + sx], row_fade);
                fill_block(dst, w, h, x0 + xx, dy, step, 1, px);
            }
        }
        counters.source_a_pixel_count += (open_w * open_h) as u64;
        counters.revealed_pixel_count += (open_w * open_h) as u64;
    }

    if progress >= params.line_start && progress < params.line_end {
        let line_decay = phase255(progress, params.line_decay_start, params.line_end);
        let glow_w = (open_w + w * (255usize.saturating_sub(line_decay)) / params.line_extra_div)
            .max((w / params.line_min_div).max(1))
            .min(w);
        let glow_h = (1 + collapse * params.line_glow_max / 255).min(params.line_glow_max);
        draw_crt_hot_line(dst, w, h, glow_w, glow_h, line_decay, counters);
    }

    if progress >= params.dot_start {
        let dot_phase = phase255(progress, params.dot_start, params.dot_end);
        let radius = scale_linear(
            (w.min(h) / params.dot_radius_div).max(1),
            255usize.saturating_sub(dot_phase),
        )
        .max(1);
        draw_crt_center_bloom(dst, w, h, radius, dot_phase, counters);
    }

    let visible = if progress < params.image_cutoff {
        open_w * open_h
    } else {
        0
    };
    counters.hidden_pixel_count += (w * h).saturating_sub(visible) as u64;
    counters.mask_cell_count += open_h as u64;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CrtPowerOffStyle {
    Balanced,
    FastSnap,
    HotLine,
    CenterDot,
    PhosphorBloom,
    Wobble,
}

#[derive(Clone, Copy)]
struct CrtPowerOffParams {
    v_start: usize,
    v_end: usize,
    x_start: usize,
    x_end: usize,
    x_closed_at: usize,
    image_cutoff: usize,
    min_w_div: usize,
    fade_loss: usize,
    min_image_fade: usize,
    taper_div: usize,
    line_start: usize,
    line_decay_start: usize,
    line_end: usize,
    line_extra_div: usize,
    line_min_div: usize,
    line_glow_max: usize,
    dot_start: usize,
    dot_end: usize,
    dot_radius_div: usize,
    afterglow: bool,
    wobble: bool,
}

impl CrtPowerOffStyle {
    fn params(self) -> CrtPowerOffParams {
        match self {
            Self::Balanced => CrtPowerOffParams {
                v_start: 8,
                v_end: 128,
                x_start: 24,
                x_end: 188,
                x_closed_at: 206,
                image_cutoff: 178,
                min_w_div: 28,
                fade_loss: 95,
                min_image_fade: 128,
                taper_div: 9,
                line_start: 24,
                line_decay_start: 112,
                line_end: 224,
                line_extra_div: 5,
                line_min_div: 14,
                line_glow_max: 7,
                dot_start: 138,
                dot_end: 238,
                dot_radius_div: 12,
                afterglow: false,
                wobble: false,
            },
            Self::FastSnap => CrtPowerOffParams {
                v_start: 4,
                v_end: 72,
                x_start: 8,
                x_end: 104,
                x_closed_at: 132,
                image_cutoff: 112,
                min_w_div: 60,
                fade_loss: 130,
                min_image_fade: 96,
                taper_div: 7,
                line_start: 8,
                line_decay_start: 54,
                line_end: 160,
                line_extra_div: 3,
                line_min_div: 22,
                line_glow_max: 4,
                dot_start: 70,
                dot_end: 172,
                dot_radius_div: 18,
                afterglow: false,
                wobble: false,
            },
            Self::HotLine => CrtPowerOffParams {
                v_start: 16,
                v_end: 150,
                x_start: 60,
                x_end: 230,
                x_closed_at: 254,
                image_cutoff: 192,
                min_w_div: 12,
                fade_loss: 80,
                min_image_fade: 144,
                taper_div: 12,
                line_start: 16,
                line_decay_start: 150,
                line_end: 255,
                line_extra_div: 2,
                line_min_div: 4,
                line_glow_max: 10,
                dot_start: 190,
                dot_end: 255,
                dot_radius_div: 24,
                afterglow: false,
                wobble: false,
            },
            Self::CenterDot => CrtPowerOffParams {
                v_start: 6,
                v_end: 112,
                x_start: 12,
                x_end: 112,
                x_closed_at: 142,
                image_cutoff: 146,
                min_w_div: 96,
                fade_loss: 110,
                min_image_fade: 112,
                taper_div: 8,
                line_start: 18,
                line_decay_start: 86,
                line_end: 182,
                line_extra_div: 6,
                line_min_div: 32,
                line_glow_max: 6,
                dot_start: 82,
                dot_end: 244,
                dot_radius_div: 7,
                afterglow: false,
                wobble: false,
            },
            Self::PhosphorBloom => CrtPowerOffParams {
                v_start: 10,
                v_end: 118,
                x_start: 35,
                x_end: 184,
                x_closed_at: 218,
                image_cutoff: 208,
                min_w_div: 18,
                fade_loss: 70,
                min_image_fade: 150,
                taper_div: 10,
                line_start: 18,
                line_decay_start: 132,
                line_end: 250,
                line_extra_div: 4,
                line_min_div: 10,
                line_glow_max: 10,
                dot_start: 126,
                dot_end: 255,
                dot_radius_div: 8,
                afterglow: true,
                wobble: false,
            },
            Self::Wobble => CrtPowerOffParams {
                v_start: 14,
                v_end: 150,
                x_start: 18,
                x_end: 170,
                x_closed_at: 204,
                image_cutoff: 190,
                min_w_div: 22,
                fade_loss: 105,
                min_image_fade: 118,
                taper_div: 6,
                line_start: 14,
                line_decay_start: 120,
                line_end: 226,
                line_extra_div: 5,
                line_min_div: 12,
                line_glow_max: 8,
                dot_start: 140,
                dot_end: 245,
                dot_radius_div: 11,
                afterglow: false,
                wobble: true,
            },
        }
    }
}

fn phase255(value: usize, start: usize, end: usize) -> usize {
    if value <= start {
        0
    } else if value >= end {
        255
    } else {
        (value - start) * 255 / (end - start).max(1)
    }
}

fn scale_linear(size: usize, amount: usize) -> usize {
    ((size as u64 * amount as u64) / 255) as usize
}

fn scale_square(size: usize, amount: usize) -> usize {
    let amount = amount as u64;
    ((size as u64 * amount * amount) / (255 * 255)) as usize
}

fn scale_cubic(size: usize, amount: usize) -> usize {
    let amount = amount as u64;
    ((size as u64 * amount * amount * amount) / (255 * 255 * 255)) as usize
}

fn draw_crt_phosphor_afterglow(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    w: usize,
    h: usize,
    phase: usize,
    counters: &mut TransitionCounters,
) {
    let step_x = (w / 80).clamp(6, 14);
    let step_y = (h / 58).clamp(4, 10);
    let block_w = (step_x / 2).max(2);
    let block_h = (step_y / 2).max(1);
    let amount = (108usize.saturating_sub(phase * 86 / 255)).clamp(14, 108) as u8;
    for y in (0..h).step_by(step_y) {
        for x in (0..w).step_by(step_x) {
            if ((x / step_x + y / step_y + phase / 18) & 3) == 0 {
                continue;
            }
            let base = fade_pixel_fast(a[y * w + x], amount);
            let tint = fade_pixel_fast(color(54, 240, 170), amount.saturating_sub(6));
            fill_block(dst, w, h, x, y, block_w, block_h, blend_fast(base, tint));
            counters.ghost_pixel_count += (block_w * block_h) as u64;
        }
    }
}

fn draw_crt_hot_line(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    line_w: usize,
    glow_h: usize,
    decay: usize,
    counters: &mut TransitionCounters,
) {
    let cx = w / 2;
    let cy = h / 2;
    let x0 = cx.saturating_sub(line_w / 2) as isize;
    let core = fade_pixel_fast(
        color(255, 255, 238),
        (255usize.saturating_sub(decay / 3)) as u8,
    );
    fill_rect(dst, w, h, x0, cy as isize, line_w, 1, core);
    counters.flash_pixel_count += line_w as u64;

    for band in 1..=glow_h {
        let band_w = (line_w + band * 18).min(w.max(1));
        let band_x = cx.saturating_sub(band_w / 2) as isize;
        let amount =
            (190usize.saturating_sub(decay / 2).saturating_sub(band * 20)).clamp(28, 190) as u8;
        let tint = fade_pixel_fast(color(100, 245, 210), amount);
        fill_rect(
            dst,
            w,
            h,
            band_x,
            cy as isize - band as isize,
            band_w,
            1,
            tint,
        );
        fill_rect(
            dst,
            w,
            h,
            band_x,
            cy as isize + band as isize,
            band_w,
            1,
            tint,
        );
        counters.flash_pixel_count += (band_w * 2) as u64;
    }
}

fn draw_crt_center_bloom(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    radius: usize,
    phase: usize,
    counters: &mut TransitionCounters,
) {
    if phase >= 252 || radius == 0 {
        return;
    }
    let cx = w / 2;
    let cy = h / 2;
    for band in 0..=radius {
        let width = (radius.saturating_sub(band) * 2 + 1).min(w.max(1));
        let amount = (245usize
            .saturating_sub(phase * 170 / 255)
            .saturating_sub(band * 18))
        .clamp(18, 245) as u8;
        let hot = if band <= 1 {
            color(255, 255, 236)
        } else {
            color(118, 240, 210)
        };
        let px = fade_pixel_fast(hot, amount);
        let x = cx.saturating_sub(width / 2) as isize;
        fill_rect(dst, w, h, x, cy as isize - band as isize, width, 1, px);
        if band > 0 {
            fill_rect(dst, w, h, x, cy as isize + band as isize, width, 1, px);
        }
        counters.flash_pixel_count += if band == 0 {
            width as u64
        } else {
            (width * 2) as u64
        };
    }
}

fn draw_burn_in_ghost(
    dst: &mut [CameraPixel],
    ghost: &mut [CameraPixel],
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    h: usize,
    progress: usize,
    counters: &mut TransitionCounters,
) {
    for y in (0..h).step_by(4) {
        for x in (0..w).step_by(4) {
            let idx = y * w + x;
            ghost[idx] = fade_pixel_fast(ghost[idx], 224);
            let base = if progress < 128 { a[idx] } else { b[idx] };
            let out = blend_fast(base, ghost[idx]);
            fill_block(dst, w, h, x, y, 4, 4, out);
            ghost[idx] = blend_fast(ghost[idx], b[idx]);
        }
    }
    counters.source_a_pixel_count += if progress < 128 { dst.len() as u64 } else { 0 };
    counters.source_b_pixel_count += if progress >= 128 { dst.len() as u64 } else { 0 };
    counters.ghost_pixel_count += dst.len() as u64;
    counters.revealed_pixel_count += dst.len() as u64;
}

fn draw_glitch_reveal(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TransitionCounters,
) {
    dst.copy_from_slice(a);
    counters.source_a_pixel_count += dst.len() as u64;
    let gate = (frame as usize * 3) % h.max(1);
    for band in 0..18 {
        let y = (band * 37 + frame as usize * 5) % h.max(1);
        let bh = 4 + (band * 3) % 18;
        let shift = tri_i32(frame as i32 + band as i32 * 11, 40, 28);
        for yy in y..(y + bh).min(h) {
            let reveal = yy <= gate || band & 3 == 0;
            for x in (0..w).step_by(4) {
                let sx = (x as i32 + shift).rem_euclid(w as i32) as usize;
                let px = if reveal {
                    b[yy * w + sx]
                } else {
                    a[yy * w + sx]
                };
                fill_block(dst, w, h, x, yy, 4, 1, px);
            }
            count_block(reveal, w, counters);
        }
        counters.glitch_band_count += 1;
    }
}

fn draw_priority_sprites(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TransitionCounters,
) {
    let mask = color(12, 12, 28);
    for i in 0..7 {
        let x = ((frame as usize * 5 + i * 97) % (w + 40)).saturating_sub(20);
        let y = h / 3 + (i * 23) % (h / 3).max(1);
        fill_rect(dst, w, h, x as isize, y as isize, 34, 24, mask);
        fill_rect(
            dst,
            w,
            h,
            x as isize + 6,
            y as isize + 5,
            22,
            14,
            color(255, 190, 60),
        );
    }
    counters.mask_cell_count += 7;
}

fn draw_marquee_base(
    dst: &mut [CameraPixel],
    a: &[CameraPixel],
    w: usize,
    h: usize,
    counters: &mut TransitionCounters,
) {
    blit_blocky_source(dst, a, w, h, 4, counters);
    let frame_c = color(35, 16, 8);
    let glass = color(20, 32, 42);
    fill_rect(dst, w, h, 0, 0, w, h / 8, frame_c);
    fill_rect(dst, w, h, 0, (h - h / 8) as isize, w, h / 8, frame_c);
    fill_rect(dst, w, h, 0, (h / 8) as isize, w, h * 3 / 8, glass);
    counters.mask_cell_count += 3;
}

fn draw_marquee_sweep(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TransitionCounters,
) {
    let sweep_x = (frame as usize * 10) % (w + 160);
    let left = sweep_x.saturating_sub(120);
    let right = sweep_x.min(w);
    for y in h / 8..h / 2 {
        let start = left.min(w);
        let end = right.min(w);
        if end > start {
            for px in &mut dst[y * w + start..y * w + end] {
                *px = blend_fast(*px, color(255, 240, 150));
            }
            counters.flash_pixel_count += (end - start) as u64;
        }
    }
    for i in 0..18 {
        let x = i * w / 18 + 4;
        let lit = ((i as u64 + frame / 8) & 3) == 0;
        fill_rect(
            dst,
            w,
            h,
            x as isize,
            (h / 16) as isize,
            (w / 40).max(5),
            (h / 18).max(5),
            if lit {
                color(255, 230, 100)
            } else {
                color(80, 48, 20)
            },
        );
        counters.mask_cell_count += 1;
    }
}

fn overlay_center_sample(
    dst: &mut [CameraPixel],
    source: &[CameraPixel],
    w: usize,
    h: usize,
    offset: usize,
    counters: &mut TransitionCounters,
) {
    let box_w = w / 3;
    let box_h = h / 3;
    let x0 = w / 2 - box_w / 2;
    let y0 = h / 2 - box_h / 2;
    for y in (0..box_h).step_by(4) {
        for x in (0..box_w).step_by(4) {
            let sx = (x * w / box_w.max(1) + offset) % w.max(1);
            let sy = y * h / box_h.max(1);
            fill_block(dst, w, h, x0 + x, y0 + y, 4, 4, source[sy * w + sx]);
            counters.source_a_pixel_count += 16;
        }
    }
}

fn blit_blocky_source(
    dst: &mut [CameraPixel],
    source: &[CameraPixel],
    w: usize,
    h: usize,
    block: usize,
    counters: &mut TransitionCounters,
) {
    for y in (0..h).step_by(block) {
        for x in (0..w).step_by(block) {
            fill_block(dst, w, h, x, y, block, block, source[y * w + x]);
            counters.source_a_pixel_count += (block * block) as u64;
        }
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
    let r = ((x * 3 + y * 2) & 255) as u8;
    let g = ((x / 2 + y * 5) & 255) as u8;
    let b = (((x ^ y) * 3) & 255) as u8;
    color(r, g, b)
}

fn copy_row(dst: &mut [CameraPixel], src: &[CameraPixel], w: usize, y: usize) {
    dst[y * w..(y + 1) * w].copy_from_slice(&src[y * w..(y + 1) * w]);
}

fn block_pick(
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    x: usize,
    y: usize,
    reveal: bool,
) -> CameraPixel {
    if reveal { b[y * w + x] } else { a[y * w + x] }
}

fn count_row(reveal: bool, w: usize, counters: &mut TransitionCounters) {
    count_block(reveal, w, counters);
}

fn count_block(reveal: bool, pixels: usize, counters: &mut TransitionCounters) {
    if reveal {
        counters.revealed_pixel_count += pixels as u64;
        counters.source_b_pixel_count += pixels as u64;
    } else {
        counters.hidden_pixel_count += pixels as u64;
        counters.source_a_pixel_count += pixels as u64;
    }
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

fn blit_block(
    dst: &mut [CameraPixel],
    src: &[CameraPixel],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    bw: usize,
    bh: usize,
) {
    for yy in y..(y + bh).min(h) {
        let start = yy * w + x.min(w);
        let end = yy * w + (x + bw).min(w);
        if end > start {
            dst[start..end].copy_from_slice(&src[start..end]);
        }
    }
}

fn fade_pixel_fast(px: CameraPixel, amount: u8) -> CameraPixel {
    let r = (((px.0 >> 11) & 31) as u32 * amount as u32 / 255) as u16;
    let g = (((px.0 >> 5) & 63) as u32 * amount as u32 / 255) as u16;
    let b = ((px.0 & 31) as u32 * amount as u32 / 255) as u16;
    CameraPixel((r << 11) | (g << 5) | b)
}

fn blend_fast(a: CameraPixel, b: CameraPixel) -> CameraPixel {
    let r = (((a.0 >> 11) & 31) + ((b.0 >> 11) & 31)) / 2;
    let g = (((a.0 >> 5) & 63) + ((b.0 >> 5) & 63)) / 2;
    let bl = ((a.0 & 31) + (b.0 & 31)) / 2;
    CameraPixel((r << 11) | (g << 5) | bl)
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
    let glyph = glyph5x7(ch);
    for yy in 0..7 {
        let bits = glyph[yy];
        for xx in 0..5 {
            let on = bits & (1 << (4 - xx)) != 0;
            if on && x + xx < w && y + yy < h {
                dst[(y + yy) * w + x + xx] = c;
            }
        }
    }
}

fn glyph5x7(ch: u8) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        b'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        b'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        b'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        b'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        b'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        b'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        b'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        b'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        b'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        b'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        b'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        b'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        b'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        b'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        b'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        b'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        b'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        b'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        b'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        b'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        b'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        b'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        b'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        b'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        b'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        b'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        b'0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        b'1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        b'2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        b'3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        b'4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        b'5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        b'6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        b'7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        b'8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        b'9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        b'-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        b'/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        b':' => [
            0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000,
        ],
        b' ' => [0; 7],
        _ => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b00100, 0b00000, 0b00100,
        ],
    }
}

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
        assert_eq!(TransitionEffectKind::all().len(), 20);
        assert_eq!(
            TransitionEffectKind::all()[0].label(),
            "venetian-blinds-wipe"
        );
        assert_eq!(
            TransitionEffectKind::all()[19].label(),
            "cabinet-marquee-light-sweep"
        );
        for kind in TransitionEffectKind::all() {
            assert_eq!(TransitionEffectKind::parse(kind.label()), Some(*kind));
            assert_eq!(
                TransitionEffectKind::parse(&kind.label().replace('-', "_")),
                Some(*kind)
            );
        }
        assert!(TransitionEffectKind::parse("bogus").is_none());
    }

    #[test]
    fn every_effect_renders_nonblank_and_deterministic() {
        let (w, h) = (96, 54);
        let images = synthetic_transition_images(3);
        for &kind in TransitionEffectKind::all() {
            let mut a = vec![CameraPixel(0); w * h];
            let mut b = vec![CameraPixel(0); w * h];
            let mut sa = TransitionEffectRenderState::new(w, h);
            let mut sb = TransitionEffectRenderState::new(w, h);
            let stats =
                render_transition_effect_frame(&mut a, &mut sa, w, h, &images, kind, 42, None);
            render_transition_effect_frame(&mut b, &mut sb, w, h, &images, kind, 42, None);
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
        let images = synthetic_transition_images(3);
        for &kind in TransitionEffectKind::all() {
            let mut a = vec![CameraPixel(0); w * h];
            let mut b = vec![CameraPixel(0); w * h];
            let mut state = TransitionEffectRenderState::new(w, h);
            render_transition_effect_frame(&mut a, &mut state, w, h, &images, kind, 0, None);
            render_transition_effect_frame(&mut b, &mut state, w, h, &images, kind, 60, None);
            assert_ne!(hash(&a), hash(&b), "{} did not visibly move", kind.label());
        }
    }

    #[test]
    fn counters_cover_transition_families() {
        let (w, h) = (80, 45);
        let images = synthetic_transition_images(2);
        for &kind in TransitionEffectKind::all() {
            let mut frame = vec![CameraPixel(0); w * h];
            let mut state = TransitionEffectRenderState::new(w, h);
            let stats = render_transition_effect_frame(
                &mut frame, &mut state, w, h, &images, kind, 17, None,
            );
            let total = stats.mask_cell_count
                + stats.revealed_pixel_count
                + stats.hidden_pixel_count
                + stats.source_a_pixel_count
                + stats.source_b_pixel_count
                + stats.shake_offset_px
                + stats.flash_pixel_count
                + stats.warp_sample_count
                + stats.ghost_pixel_count
                + stats.glitch_band_count;
            assert!(total > 0, "{} had no transition counters", kind.label());
        }
    }

    #[test]
    fn hud_label_font_covers_effect_labels() {
        let unknown = glyph5x7(b'?');
        for kind in TransitionEffectKind::all() {
            for byte in kind.label().bytes() {
                assert_ne!(
                    glyph5x7(byte),
                    unknown,
                    "{} used unsupported HUD glyph {}",
                    kind.label(),
                    byte as char
                );
            }
        }

        let (w, h) = (260, 32);
        let mut frame = vec![CameraPixel(0); w * h];
        draw_label(&mut frame, w, h, "crt-power-off-phosphor-bloom 5/6");
        assert!(nonblank(&frame));
    }

    #[test]
    fn retained_ghost_is_deterministic_and_persistent() {
        let (w, h) = (96, 54);
        let images = synthetic_transition_images(2);
        let mut frame = vec![CameraPixel(0); w * h];
        let mut state = TransitionEffectRenderState::new(w, h);
        render_transition_effect_frame(
            &mut frame,
            &mut state,
            w,
            h,
            &images,
            TransitionEffectKind::BurnInGhostCrossfade,
            0,
            None,
        );
        let h0 = hash(&frame);
        render_transition_effect_frame(
            &mut frame,
            &mut state,
            w,
            h,
            &images,
            TransitionEffectKind::BurnInGhostCrossfade,
            1,
            None,
        );
        assert_ne!(h0, hash(&frame));
        assert!(frame.iter().filter(|px| px.0 != 0).count() > 20);
    }

    #[test]
    fn small_sizes_do_not_panic() {
        let images = synthetic_transition_images(1);
        for &kind in TransitionEffectKind::all() {
            for &(w, h) in &[(1, 1), (7, 5), (16, 9)] {
                let mut frame = vec![CameraPixel(0); w * h];
                let mut state = TransitionEffectRenderState::new(w, h);
                render_transition_effect_frame(
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
