//! Host-testable classic arcade screen transition effects.

use std::time::Instant;

pub use crate::camera_effects::pixel_to_rgb888;
use crate::camera_effects::{color, synthetic_images, CameraImage, CameraPixel};

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
    BurnInGhostCrossfade,
    GlitchySpritePriorityReveal,
    CabinetMarqueeLightSweep,
}

impl TransitionEffectKind {
    pub const ALL: [Self; 15] = [
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
            draw_crt_collapse(dst, &state.source_a, w, h, progress, &mut counters)
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
            let spoke =
                ((dy.abs() * 7 + dx.abs() * 5 + if dx ^ dy < 0 { 8 } else { 0 }) & 15) as i32;
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
            let dx = (x as i32 + 2 - cx).abs() as usize;
            let dy = (y as i32 + 2 - cy).abs() as usize;
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
    counters: &mut TransitionCounters,
) {
    clear(dst, color(0, 0, 0));
    let open_h = (h * (255 - progress).max(3) / 255).max(1);
    let y0 = h / 2 - open_h / 2;
    for y in 0..open_h {
        let sy = y * h / open_h.max(1);
        let dy = y0 + y;
        if dy < h {
            copy_row(dst, a, w, dy);
            let row = &mut dst[dy * w..(dy + 1) * w];
            for px in row {
                *px = fade_pixel_fast(*px, (255 - progress / 2) as u8);
            }
        }
        counters.source_a_pixel_count += w as u64;
        counters.revealed_pixel_count += w as u64;
        let _ = sy;
    }
    if progress > 180 {
        let line_y = h / 2;
        row(dst, w, line_y).fill(color(220, 255, 210));
        counters.flash_pixel_count += w as u64;
    }
    counters.hidden_pixel_count += (w * h).saturating_sub(w * open_h) as u64;
    counters.mask_cell_count += open_h as u64;
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

fn row(dst: &mut [CameraPixel], w: usize, y: usize) -> &mut [CameraPixel] {
    &mut dst[y * w..(y + 1) * w]
}

fn block_pick(
    a: &[CameraPixel],
    b: &[CameraPixel],
    w: usize,
    x: usize,
    y: usize,
    reveal: bool,
) -> CameraPixel {
    if reveal {
        b[y * w + x]
    } else {
        a[y * w + x]
    }
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
        assert_eq!(TransitionEffectKind::all().len(), 15);
        assert_eq!(
            TransitionEffectKind::all()[0].label(),
            "venetian-blinds-wipe"
        );
        assert_eq!(
            TransitionEffectKind::all()[14].label(),
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
