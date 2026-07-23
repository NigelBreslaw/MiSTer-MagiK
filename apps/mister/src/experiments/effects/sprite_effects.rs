// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-testable classic arcade sprite/object effects.
#![allow(clippy::too_many_arguments)]

use std::time::Instant;

pub use super::camera_effects::pixel_to_rgb888;
use super::camera_effects::{CameraImage, CameraPixel, color, synthetic_images};
use super::render_helpers::{clear, elapsed_us, time};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpriteEffectKind {
    SpriteZoomTowardCamera,
    SpriteShrinkIntoDistance,
    MultiSpriteLargeObject,
    BossPartsAssemble,
    SpritePriorityForeground,
    SpriteClippingWindow,
    DropShadowCopy,
    BlobContactShadow,
    InvincibilityFlicker,
    AfterimageTrail,
    MotionSmearRepeats,
    ExplodingSpriteDebris,
    TileChunksFlyApart,
    ParticleSparkleBurst,
    BulletHellOrnaments,
    RotatingSpriteCard,
    SpriteSheetFlipbookLogo,
    PaletteSwappedVariants,
    MirroredSpriteReflections,
    ObjectOverloadFlicker,
}

impl SpriteEffectKind {
    pub const ALL: [Self; 20] = [
        Self::SpriteZoomTowardCamera,
        Self::SpriteShrinkIntoDistance,
        Self::MultiSpriteLargeObject,
        Self::BossPartsAssemble,
        Self::SpritePriorityForeground,
        Self::SpriteClippingWindow,
        Self::DropShadowCopy,
        Self::BlobContactShadow,
        Self::InvincibilityFlicker,
        Self::AfterimageTrail,
        Self::MotionSmearRepeats,
        Self::ExplodingSpriteDebris,
        Self::TileChunksFlyApart,
        Self::ParticleSparkleBurst,
        Self::BulletHellOrnaments,
        Self::RotatingSpriteCard,
        Self::SpriteSheetFlipbookLogo,
        Self::PaletteSwappedVariants,
        Self::MirroredSpriteReflections,
        Self::ObjectOverloadFlicker,
    ];

    pub fn all() -> &'static [Self] {
        &Self::ALL
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::SpriteZoomTowardCamera => "sprite-zoom-toward-camera",
            Self::SpriteShrinkIntoDistance => "sprite-shrink-into-distance",
            Self::MultiSpriteLargeObject => "multi-sprite-large-object",
            Self::BossPartsAssemble => "boss-parts-assemble",
            Self::SpritePriorityForeground => "sprite-priority-foreground",
            Self::SpriteClippingWindow => "sprite-clipping-window",
            Self::DropShadowCopy => "drop-shadow-copy",
            Self::BlobContactShadow => "blob-contact-shadow",
            Self::InvincibilityFlicker => "invincibility-flicker",
            Self::AfterimageTrail => "afterimage-trail",
            Self::MotionSmearRepeats => "motion-smear-repeats",
            Self::ExplodingSpriteDebris => "exploding-sprite-debris",
            Self::TileChunksFlyApart => "tile-chunks-fly-apart",
            Self::ParticleSparkleBurst => "particle-sparkle-burst",
            Self::BulletHellOrnaments => "bullet-hell-ornaments",
            Self::RotatingSpriteCard => "rotating-sprite-card",
            Self::SpriteSheetFlipbookLogo => "sprite-sheet-flipbook-logo",
            Self::PaletteSwappedVariants => "palette-swapped-variants",
            Self::MirroredSpriteReflections => "mirrored-sprite-reflections",
            Self::ObjectOverloadFlicker => "object-overload-flicker",
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
pub struct SpriteEffectFrameStats {
    pub clear_us: u64,
    pub background_us: u64,
    pub projection_us: u64,
    pub image_blit_us: u64,
    pub sprite_us: u64,
    pub post_us: u64,
    pub hud_us: u64,
    pub sprite_count: u64,
    pub sprite_pixels: u64,
    pub particle_count: u64,
    pub flicker_skip_count: u64,
}

impl SpriteEffectFrameStats {
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
struct SpriteCounters {
    sprite_count: u64,
    sprite_pixels: u64,
    particle_count: u64,
    flicker_skip_count: u64,
}

impl SpriteCounters {
    fn record_sprite(&mut self, pixels: u64) {
        if pixels > 0 {
            self.sprite_count += 1;
            self.sprite_pixels += pixels;
        }
    }
}

pub struct SpriteEffectRenderState {
    scratch: Vec<CameraPixel>,
    atlas: SpriteAtlas,
    w: usize,
    h: usize,
}

impl SpriteEffectRenderState {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            scratch: vec![CameraPixel(0); w * h],
            atlas: SpriteAtlas::new(),
            w,
            h,
        }
    }

    fn resize(&mut self, w: usize, h: usize) {
        if self.w == w && self.h == h {
            return;
        }
        self.scratch.resize(w * h, CameraPixel(0));
        self.w = w;
        self.h = h;
    }
}

pub fn render_sprite_effect_frame(
    dst: &mut [CameraPixel],
    state: &mut SpriteEffectRenderState,
    w: usize,
    h: usize,
    images: &[CameraImage],
    kind: SpriteEffectKind,
    frame: u64,
    hud: Option<&str>,
) -> SpriteEffectFrameStats {
    assert_eq!(dst.len(), w * h);
    state.resize(w, h);

    let mut stats = SpriteEffectFrameStats::default();
    let mut counters = SpriteCounters::default();

    let t = Instant::now();
    clear(dst, color(2, 4, 14));
    stats.clear_us = elapsed_us(t);

    match kind {
        SpriteEffectKind::SpriteZoomTowardCamera => {
            time(&mut stats.background_us, || {
                draw_grid_stage(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_zoom_sprite(dst, w, h, &state.atlas, frame, true, &mut counters)
            });
        }
        SpriteEffectKind::SpriteShrinkIntoDistance => {
            time(&mut stats.background_us, || {
                draw_grid_stage(dst, w, h, frame / 2)
            });
            time(&mut stats.sprite_us, || {
                render_zoom_sprite(dst, w, h, &state.atlas, frame, false, &mut counters)
            });
        }
        SpriteEffectKind::MultiSpriteLargeObject => {
            time(&mut stats.background_us, || draw_hangar(dst, w, h, frame));
            time(&mut stats.sprite_us, || {
                render_multi_sprite_object(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::BossPartsAssemble => {
            time(&mut stats.background_us, || {
                draw_hangar(dst, w, h, frame / 2)
            });
            time(&mut stats.sprite_us, || {
                render_boss_assembly(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::SpritePriorityForeground => {
            time(&mut stats.background_us, || {
                draw_layered_street(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_priority_foreground(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::SpriteClippingWindow => {
            time(&mut stats.background_us, || {
                draw_scan_grid(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_clipping_window(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::DropShadowCopy => {
            time(&mut stats.background_us, || draw_floor(dst, w, h, frame));
            time(&mut stats.sprite_us, || {
                render_drop_shadow_copy(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::BlobContactShadow => {
            time(&mut stats.background_us, || {
                draw_floor(dst, w, h, frame / 2)
            });
            time(&mut stats.sprite_us, || {
                render_blob_contact_shadow(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::InvincibilityFlicker => {
            time(&mut stats.background_us, || draw_arena(dst, w, h, frame));
            time(&mut stats.sprite_us, || {
                render_invincibility_flicker(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::AfterimageTrail => {
            time(&mut stats.background_us, || {
                draw_speed_lane(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_afterimage_trail(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::MotionSmearRepeats => {
            time(&mut stats.background_us, || {
                draw_speed_lane(dst, w, h, frame / 2)
            });
            time(&mut stats.sprite_us, || {
                render_motion_smear(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::ExplodingSpriteDebris => {
            time(&mut stats.background_us, || {
                draw_arena(dst, w, h, frame / 3)
            });
            time(&mut stats.sprite_us, || {
                render_exploding_debris(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::TileChunksFlyApart => {
            time(&mut stats.background_us, || {
                draw_scan_grid(dst, w, h, frame / 3)
            });
            time(&mut stats.sprite_us, || {
                render_tile_chunks(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::ParticleSparkleBurst => {
            time(&mut stats.background_us, || {
                draw_dark_radial(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_sparkle_burst(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::BulletHellOrnaments => {
            time(&mut stats.background_us, || {
                draw_dark_radial(dst, w, h, frame / 2)
            });
            time(&mut stats.sprite_us, || {
                render_bullet_hell(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::RotatingSpriteCard => {
            time(&mut stats.background_us, || {
                draw_card_table(dst, w, h, frame)
            });
            time(&mut stats.image_blit_us, || {
                render_card_texture(dst, w, h, images, frame)
            });
            time(&mut stats.sprite_us, || {
                render_rotating_card(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::SpriteSheetFlipbookLogo => {
            time(&mut stats.background_us, || {
                draw_scan_grid(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_flipbook_logo(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::PaletteSwappedVariants => {
            time(&mut stats.background_us, || {
                draw_palette_wall(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_palette_variants(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
        SpriteEffectKind::MirroredSpriteReflections => {
            time(&mut stats.background_us, || {
                draw_reflection_floor(dst, w, h, frame)
            });
            time(&mut stats.image_blit_us, || {
                render_reflection_texture(dst, w, h, images, frame)
            });
            time(&mut stats.sprite_us, || {
                render_mirrored_reflection(dst, w, h, &state.atlas, frame, &mut counters)
            });
            time(&mut stats.post_us, || {
                fade_lower_half(dst, w, h, color(0, 18, 34), 88)
            });
        }
        SpriteEffectKind::ObjectOverloadFlicker => {
            time(&mut stats.background_us, || {
                draw_overload_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_object_overload(dst, w, h, &state.atlas, frame, &mut counters)
            });
        }
    }

    if let Some(text) = hud {
        time(&mut stats.hud_us, || draw_label(dst, w, h, text));
    }

    stats.sprite_count = counters.sprite_count;
    stats.sprite_pixels = counters.sprite_pixels;
    stats.particle_count = counters.particle_count;
    stats.flicker_skip_count = counters.flicker_skip_count;
    stats
}

pub fn synthetic_sprite_images(count: usize) -> Vec<CameraImage> {
    synthetic_images(count)
}

#[derive(Clone)]
struct Sprite {
    w: usize,
    h: usize,
    pixels: Vec<CameraPixel>,
}

impl Sprite {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            pixels: vec![CameraPixel(0); w * h],
        }
    }

    fn set(&mut self, x: usize, y: usize, c: CameraPixel) {
        if x < self.w && y < self.h {
            self.pixels[y * self.w + x] = c;
        }
    }

    fn fill_rect(&mut self, x: usize, y: usize, rw: usize, rh: usize, c: CameraPixel) {
        for yy in y..(y + rh).min(self.h) {
            for xx in x..(x + rw).min(self.w) {
                self.set(xx, yy, c);
            }
        }
    }

    fn recolor(&self, colors: &[CameraPixel]) -> Self {
        let mut out = self.clone();
        for (idx, px) in out.pixels.iter_mut().enumerate() {
            if px.0 != 0 {
                let rgb = pixel_to_rgb888(*px);
                let bright = (((rgb >> 16) & 255) + ((rgb >> 8) & 255) + (rgb & 255)) / 3;
                *px = blend(color(0, 0, 0), colors[idx % colors.len()], bright as u8);
            }
        }
        out
    }
}

struct SpriteAtlas {
    ship: Sprite,
    ship_variants: Vec<Sprite>,
    card: Sprite,
    boss_parts: Vec<Sprite>,
    tile: Sprite,
    chunk: Sprite,
    bullet: Sprite,
    sparkle: Sprite,
    logo_frames: Vec<Sprite>,
}

impl SpriteAtlas {
    fn new() -> Self {
        let ship = build_ship_sprite();
        let ship_variants = vec![
            ship.recolor(&[color(250, 70, 90), color(255, 220, 90), color(90, 240, 255)]),
            ship.recolor(&[
                color(80, 255, 130),
                color(255, 255, 160),
                color(60, 150, 255),
            ]),
            ship.recolor(&[
                color(190, 90, 255),
                color(255, 190, 255),
                color(255, 120, 80),
            ]),
            ship.recolor(&[
                color(255, 150, 40),
                color(255, 245, 140),
                color(80, 200, 255),
            ]),
        ];
        Self {
            ship,
            ship_variants,
            card: build_card_sprite(),
            boss_parts: build_boss_parts(),
            tile: build_tile_sprite(),
            chunk: build_chunk_sprite(),
            bullet: build_bullet_sprite(),
            sparkle: build_sparkle_sprite(),
            logo_frames: build_logo_frames(),
        }
    }
}

fn build_ship_sprite() -> Sprite {
    let mut s = Sprite::new(32, 32);
    let body = color(215, 235, 255);
    let shadow = color(62, 110, 165);
    let trim = color(255, 76, 94);
    let cockpit = color(85, 230, 255);
    let flame = color(255, 210, 64);
    for y in 0..25 {
        let half = (y / 2 + 2).min(13) as i32;
        for x in (16 - half)..=(16 + half) {
            let c = if x < 16 - half + 2 || x > 16 + half - 2 {
                shadow
            } else {
                body
            };
            s.set(x as usize, y + 3, c);
        }
    }
    s.fill_rect(13, 8, 6, 7, cockpit);
    s.fill_rect(4, 19, 7, 7, trim);
    s.fill_rect(21, 19, 7, 7, trim);
    s.fill_rect(13, 27, 3, 4, flame);
    s.fill_rect(17, 27, 3, 4, flame);
    s
}

fn build_card_sprite() -> Sprite {
    let mut s = Sprite::new(48, 64);
    let border = color(242, 238, 210);
    let fill_a = color(42, 70, 130);
    let fill_b = color(210, 64, 95);
    s.fill_rect(0, 0, 48, 64, border);
    s.fill_rect(3, 3, 42, 58, fill_a);
    for y in 8..56 {
        for x in 7..41 {
            if ((x / 6 + y / 6) & 1) == 0 {
                s.set(x, y, fill_b);
            }
        }
    }
    s.fill_rect(14, 23, 20, 18, color(255, 220, 92));
    s
}

fn build_boss_parts() -> Vec<Sprite> {
    let colors = [
        color(165, 210, 255),
        color(255, 100, 120),
        color(255, 216, 96),
        color(96, 236, 172),
        color(178, 112, 255),
        color(255, 148, 72),
    ];
    (0..12)
        .map(|i| {
            let mut s = Sprite::new(24, 24);
            let base = colors[i % colors.len()];
            let shade = tint_pixel(base, 105);
            s.fill_rect(2, 2, 20, 20, shade);
            s.fill_rect(4, 4, 16, 16, base);
            if i & 1 == 0 {
                s.fill_rect(7, 0, 10, 24, color(235, 245, 255));
            } else {
                s.fill_rect(0, 7, 24, 10, color(235, 245, 255));
            }
            if i % 3 == 0 {
                s.fill_rect(8, 8, 8, 8, color(40, 70, 120));
            }
            s
        })
        .collect()
}

fn build_tile_sprite() -> Sprite {
    let mut s = Sprite::new(16, 16);
    s.fill_rect(0, 0, 16, 16, color(30, 42, 72));
    s.fill_rect(1, 1, 14, 14, color(80, 132, 178));
    s.fill_rect(3, 3, 10, 3, color(180, 220, 255));
    s.fill_rect(3, 9, 10, 4, color(24, 60, 102));
    s
}

fn build_chunk_sprite() -> Sprite {
    let mut s = Sprite::new(12, 12);
    s.fill_rect(1, 1, 10, 10, color(225, 94, 80));
    s.fill_rect(2, 2, 8, 3, color(255, 210, 80));
    s.fill_rect(3, 7, 6, 3, color(92, 42, 80));
    s
}

fn build_bullet_sprite() -> Sprite {
    let mut s = Sprite::new(8, 8);
    for y in 0..8 {
        for x in 0..8 {
            let dx = x as i32 - 3;
            let dy = y as i32 - 3;
            let d = dx * dx + dy * dy;
            if d <= 14 {
                let c = if d <= 4 {
                    color(255, 255, 240)
                } else {
                    color(255, 120, 72)
                };
                s.set(x, y, c);
            }
        }
    }
    s
}

fn build_sparkle_sprite() -> Sprite {
    let mut s = Sprite::new(9, 9);
    for i in 0..9 {
        s.set(4, i, color(255, 255, 210));
        s.set(i, 4, color(255, 255, 210));
    }
    s.set(3, 3, color(115, 240, 255));
    s.set(5, 3, color(115, 240, 255));
    s.set(3, 5, color(115, 240, 255));
    s.set(5, 5, color(115, 240, 255));
    s
}

fn build_logo_frames() -> Vec<Sprite> {
    (0..4)
        .map(|frame| {
            let mut s = Sprite::new(72, 24);
            let a = [
                color(255, 82, 100),
                color(255, 206, 74),
                color(76, 232, 190),
                color(100, 180, 255),
            ];
            s.fill_rect(0, 0, 72, 24, color(10, 18, 42));
            for glyph in 0..5 {
                let x = 5 + glyph * 13;
                let c = a[(glyph + frame) % a.len()];
                s.fill_rect(x, 4, 9, 16, c);
                s.fill_rect(x + 2, 7, 5, 10, color(10, 18, 42));
                s.fill_rect(x + ((frame + glyph) % 3), 4, 3, 16, c);
            }
            s.fill_rect(5 + frame * 3, 20, 18, 2, color(255, 255, 230));
            s
        })
        .collect()
}

fn put(dst: &mut [CameraPixel], w: usize, h: usize, x: isize, y: isize, c: CameraPixel) {
    if x >= 0 && y >= 0 {
        let x = x as usize;
        let y = y as usize;
        if x < w && y < h {
            dst[y * w + x] = c;
        }
    }
}

fn fill_rect(
    dst: &mut [CameraPixel],
    screen_w: usize,
    screen_h: usize,
    x: isize,
    y: isize,
    rw: usize,
    rh: usize,
    c: CameraPixel,
) {
    let x0 = x.max(0) as usize;
    let y0 = y.max(0) as usize;
    let x1 = (x + rw as isize).clamp(0, screen_w as isize) as usize;
    let y1 = (y + rh as isize).clamp(0, screen_h as isize) as usize;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    for yy in y0..y1 {
        dst[yy * screen_w + x0..yy * screen_w + x1].fill(c);
    }
}

fn tint_pixel(px: CameraPixel, amount: u8) -> CameraPixel {
    let rgb = pixel_to_rgb888(px);
    let a = amount as u32;
    color(
        (((rgb >> 16) & 255) * a / 255) as u8,
        (((rgb >> 8) & 255) * a / 255) as u8,
        ((rgb & 255) * a / 255) as u8,
    )
}

fn blend(a: CameraPixel, b: CameraPixel, amount: u8) -> CameraPixel {
    let ar = pixel_to_rgb888(a);
    let br = pixel_to_rgb888(b);
    let t = amount as u32;
    let inv = 255 - t;
    color(
        ((((ar >> 16) & 255) * inv + ((br >> 16) & 255) * t) / 255) as u8,
        ((((ar >> 8) & 255) * inv + ((br >> 8) & 255) * t) / 255) as u8,
        (((ar & 255) * inv + (br & 255) * t) / 255) as u8,
    )
}

fn triangle(v: usize) -> u8 {
    let x = v & 255;
    if x < 128 {
        (x * 2) as u8
    } else {
        ((255 - x) * 2) as u8
    }
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

fn draw_sprite_scaled(
    dst: &mut [CameraPixel],
    screen_w: usize,
    screen_h: usize,
    sprite: &Sprite,
    x: isize,
    y: isize,
    out_w: usize,
    out_h: usize,
    tint: u8,
    flip_x: bool,
    flip_y: bool,
) -> u64 {
    draw_sprite_scaled_clipped(
        dst, screen_w, screen_h, sprite, x, y, out_w, out_h, tint, flip_x, flip_y, 0, 0, screen_w,
        screen_h,
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_sprite_scaled_clipped(
    dst: &mut [CameraPixel],
    screen_w: usize,
    screen_h: usize,
    sprite: &Sprite,
    x: isize,
    y: isize,
    out_w: usize,
    out_h: usize,
    tint: u8,
    flip_x: bool,
    flip_y: bool,
    clip_x: usize,
    clip_y: usize,
    clip_w: usize,
    clip_h: usize,
) -> u64 {
    if out_w == 0 || out_h == 0 || screen_w == 0 || screen_h == 0 {
        return 0;
    }
    let cx0 = clip_x.min(screen_w);
    let cy0 = clip_y.min(screen_h);
    let cx1 = (clip_x + clip_w).min(screen_w);
    let cy1 = (clip_y + clip_h).min(screen_h);
    let dx0 = (x.max(0) as usize).max(cx0);
    let dy0 = (y.max(0) as usize).max(cy0);
    let dx1 = ((x + out_w as isize).clamp(0, screen_w as isize) as usize).min(cx1);
    let dy1 = ((y + out_h as isize).clamp(0, screen_h as isize) as usize).min(cy1);
    if dx1 <= dx0 || dy1 <= dy0 {
        return 0;
    }
    let step_x = ((sprite.w << 16) / out_w.max(1)).max(1);
    let step_y = ((sprite.h << 16) / out_h.max(1)).max(1);
    let base_x = (dx0 as isize - x).max(0) as usize * step_x;
    let mut sy_fp = (dy0 as isize - y).max(0) as usize * step_y;
    let mut drawn = 0u64;
    for dy in dy0..dy1 {
        let mut sy = (sy_fp >> 16).min(sprite.h - 1);
        if flip_y {
            sy = sprite.h - 1 - sy;
        }
        let mut sx_fp = base_x;
        for dx in dx0..dx1 {
            let mut sx = (sx_fp >> 16).min(sprite.w - 1);
            if flip_x {
                sx = sprite.w - 1 - sx;
            }
            let px = sprite.pixels[sy * sprite.w + sx];
            if px.0 != 0 {
                dst[dy * screen_w + dx] = if tint == 255 {
                    px
                } else {
                    tint_pixel(px, tint)
                };
                drawn += 1;
            }
            sx_fp += step_x;
        }
        sy_fp += step_y;
    }
    drawn
}

fn draw_ellipse(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    cx: isize,
    cy: isize,
    rx: isize,
    ry: isize,
    c: CameraPixel,
) {
    if rx <= 0 || ry <= 0 {
        return;
    }
    let y0 = (cy - ry).max(0) as usize;
    let y1 = (cy + ry).clamp(0, h as isize) as usize;
    for y in y0..y1 {
        let dy = y as isize - cy;
        let span_sq = rx * rx * (ry * ry - dy * dy).max(0) / (ry * ry).max(1);
        let span = int_sqrt(span_sq as u64) as isize;
        fill_rect(
            dst,
            w,
            h,
            cx - span,
            y as isize,
            (span * 2).max(1) as usize,
            1,
            c,
        );
    }
}

fn int_sqrt(v: u64) -> u64 {
    let mut x = v;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

fn draw_grid_stage(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let p = (y * 255 / h.max(1)) as u8;
        dst[y * w..(y + 1) * w].fill(color(6 + p / 18, 10 + p / 11, 30 + p / 5));
    }
    let horizon = h / 2;
    for y in horizon..h {
        let depth = y - horizon + 1;
        if ((depth + frame as usize / 2) & 15) == 0 {
            dst[y * w..(y + 1) * w].fill(color(70, 92, 120));
        }
        let center = w / 2;
        let step = (depth / 2).max(12);
        for x in (center..w).step_by(step) {
            put(dst, w, h, x as isize, y as isize, color(42, 70, 105));
        }
        for x in (0..=center).rev().step_by(step) {
            put(dst, w, h, x as isize, y as isize, color(42, 70, 105));
        }
    }
}

fn draw_hangar(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    draw_grid_stage(dst, w, h, frame);
    for i in 0..10 {
        let x = i * w / 10;
        fill_rect(dst, w, h, x as isize, 0, 4, h, color(16, 22, 45));
        let light = triangle((frame as usize * 3 + i * 23) & 255);
        fill_rect(
            dst,
            w,
            h,
            x as isize + 8,
            20,
            12,
            4,
            color(40 + light / 5, 80 + light / 4, 120 + light / 3),
        );
    }
}

fn draw_layered_street(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let p = (y * 255 / h.max(1)) as u8;
        dst[y * w..(y + 1) * w].fill(color(8, 14 + p / 9, 34 + p / 8));
    }
    for i in 0..16 {
        let x = ((i * 101 + frame as usize / 2) % (w + 80)) as isize - 40;
        fill_rect(dst, w, h, x, h as isize / 2 - 50, 34, 90, color(20, 28, 54));
        fill_rect(
            dst,
            w,
            h,
            x + 6,
            h as isize / 2 - 35,
            4,
            8,
            color(230, 180, 74),
        );
    }
}

fn draw_scan_grid(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let scan = if (y + frame as usize / 3) & 7 == 0 {
            15
        } else {
            0
        };
        dst[y * w..(y + 1) * w].fill(color(5 + scan, 14 + scan, 30 + scan * 2));
    }
    for x in (0..w).step_by(24) {
        for y in (0..h).step_by(3) {
            put(dst, w, h, x as isize, y as isize, color(24, 54, 82));
        }
    }
}

fn draw_floor(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let p = (y * 255 / h.max(1)) as u8;
        dst[y * w..(y + 1) * w].fill(if y < h / 2 {
            color(8, 18 + p / 9, 44 + p / 4)
        } else {
            color(26 + p / 10, 24 + p / 14, 34 + p / 20)
        });
    }
    for y in (h / 2..h).step_by(22) {
        let shift = (frame as usize / 2 + y) % 38;
        for x in (0..w + 38).step_by(38) {
            fill_rect(
                dst,
                w,
                h,
                x as isize - shift as isize,
                y as isize,
                18,
                2,
                color(78, 72, 88),
            );
        }
    }
}

fn draw_arena(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let p = (y * 255 / h.max(1)) as u8;
        let pulse = triangle((frame as usize * 2 + y / 3) & 255) / 14;
        dst[y * w..(y + 1) * w].fill(color(4 + pulse, 10 + p / 16, 26 + p / 7));
    }
    let horizon = h / 2;
    for y in (horizon..h).step_by(24) {
        let depth = y - horizon + 1;
        let inset = (w / 2).saturating_sub(depth * w / h.max(1));
        let stripe_w = w.saturating_sub(inset * 2);
        fill_rect(
            dst,
            w,
            h,
            inset as isize,
            y as isize,
            stripe_w,
            2,
            color(24, 54, 84),
        );
    }
    for i in 0..18usize {
        let x = (hash(i as u32 * 61) as usize + frame as usize * (i % 3 + 1)) % w.max(1);
        let y = h / 4 + (hash(i as u32 * 97) as usize % (h / 2).max(1));
        fill_rect(dst, w, h, x as isize, y as isize, 4, 4, color(62, 90, 128));
    }
}

fn draw_speed_lane(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    clear(dst, color(3, 8, 22));
    for i in 0..80 {
        let y = (hash(i * 53) as usize + frame as usize * (2 + i as usize % 4)) % h.max(1);
        let x0 = (hash(i * 97) as usize) % w.max(1);
        fill_rect(
            dst,
            w,
            h,
            x0 as isize,
            y as isize,
            30 + (i as usize & 31),
            2,
            color(38, 76, 116),
        );
    }
}

fn draw_dark_radial(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    let cx = w as isize / 2 + triangle(frame as usize & 255) as isize / 10 - 13;
    let cy = h as isize / 2;
    for y in 0..h {
        for x in 0..w {
            let d = ((x as isize - cx).unsigned_abs() + (y as isize - cy).unsigned_abs()) as u8;
            dst[y * w + x] = color(3 + d / 22, 8 + d / 18, 22 + d / 10);
        }
    }
}

fn draw_card_table(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let p = (y * 255 / h.max(1)) as u8;
        dst[y * w..(y + 1) * w].fill(color(14 + p / 20, 42 + p / 8, 44 + p / 10));
    }
    let stripe = (frame as usize / 3) & 31;
    for x in (0..w + 32).step_by(32) {
        fill_rect(
            dst,
            w,
            h,
            x as isize - stripe as isize,
            0,
            8,
            h,
            color(10, 32, 34),
        );
    }
}

fn draw_palette_wall(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    let cell_w = 48usize;
    let cell_h = 36usize;
    for y in (0..h).step_by(cell_h) {
        for x in (0..w).step_by(cell_w) {
            let band = ((x / cell_w + y / cell_h + frame as usize / 16) & 3) as u8;
            let c = match band {
                0 => color(22, 26, 56),
                1 => color(26, 58, 48),
                2 => color(64, 34, 54),
                _ => color(70, 52, 28),
            };
            fill_rect(dst, w, h, x as isize, y as isize, cell_w, cell_h, c);
        }
    }
}

fn draw_reflection_floor(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let p = (y * 255 / h.max(1)) as u8;
        dst[y * w..(y + 1) * w].fill(if y < h / 2 {
            color(4, 10 + p / 10, 34 + p / 4)
        } else {
            color(0, 24 + p / 10, 38 + p / 8)
        });
    }
    let water = h / 2 + 20;
    for y in water..h {
        let wave = triangle((frame as usize * 3 + y) & 255) as isize / 18;
        for x in (0..w).step_by(9) {
            put(dst, w, h, x as isize + wave, y as isize, color(40, 96, 120));
        }
    }
}

fn draw_overload_backdrop(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    draw_scan_grid(dst, w, h, frame);
    for y in (0..h).step_by(18) {
        let c = if ((y + frame as usize) / 18) & 1 == 0 {
            color(44, 20, 42)
        } else {
            color(12, 30, 54)
        };
        fill_rect(dst, w, h, 0, y as isize, w, 2, c);
    }
}

fn render_zoom_sprite(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    toward: bool,
    counters: &mut SpriteCounters,
) {
    let t = triangle((frame as usize * 3) & 255) as usize;
    let size = if toward {
        32 + t * 5 / 2
    } else {
        32 + (255 - t) * 5 / 2
    }
    .max(6);
    let x = w as isize / 2 - size as isize / 2;
    let y = h as isize / 2 - size as isize / 2 + 12;
    draw_ellipse(
        dst,
        w,
        h,
        w as isize / 2,
        y + size as isize - 2,
        size as isize / 2,
        size as isize / 8,
        color(2, 4, 12),
    );
    let pixels = draw_sprite_scaled(dst, w, h, &atlas.ship, x, y, size, size, 255, false, false);
    counters.record_sprite(pixels);
}

fn render_multi_sprite_object(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let tile = (w.min(h) / 12).clamp(8, 42);
    let cols = 10usize;
    let rows = 6usize;
    let ox = w as isize / 2 - (cols * tile) as isize / 2;
    let oy = h as isize / 2 - (rows * tile) as isize / 2
        + triangle(frame as usize & 255) as isize / 18
        - 7;
    for row in 0..rows {
        for col in 0..cols {
            if row == 0 && !(2..=7).contains(&col) {
                continue;
            }
            let sprite = &atlas.boss_parts[(row * cols + col) % atlas.boss_parts.len()];
            let x = ox + (col * tile) as isize;
            let y = oy + (row * tile) as isize;
            let pixels = draw_sprite_scaled(
                dst,
                w,
                h,
                sprite,
                x,
                y,
                tile + 2,
                tile + 2,
                255,
                col & 1 == 0,
                false,
            );
            counters.record_sprite(pixels);
        }
    }
}

fn render_boss_assembly(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let tile = (w.min(h) / 9).clamp(10, 54);
    let phase = (frame % 150) as isize;
    let progress = phase.min(90);
    let layout = [
        (-2, -1),
        (-1, -1),
        (0, -1),
        (1, -1),
        (2, -1),
        (-2, 0),
        (-1, 0),
        (0, 0),
        (1, 0),
        (2, 0),
        (-1, 1),
        (1, 1),
    ];
    for (i, &(gx, gy)) in layout.iter().enumerate() {
        let tx = w as isize / 2 + gx * tile as isize - tile as isize / 2;
        let ty = h as isize / 2 + gy * tile as isize - tile as isize / 2;
        let sx = if i & 1 == 0 {
            -(tile as isize) * 3
        } else {
            w as isize + tile as isize * 2
        };
        let sy = ((i * 47) % h.max(1)) as isize;
        let x = sx + (tx - sx) * progress / 90;
        let y = sy + (ty - sy) * progress / 90;
        let pixels = draw_sprite_scaled(
            dst,
            w,
            h,
            &atlas.boss_parts[i % atlas.boss_parts.len()],
            x,
            y,
            tile,
            tile,
            255,
            false,
            false,
        );
        counters.record_sprite(pixels);
    }
}

fn render_priority_foreground(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let size = (h / 5).max(18);
    let span = w + size * 2;
    let x = ((frame as usize * 4) % span) as isize - size as isize;
    let y = h as isize / 2 - size as isize / 2 + triangle((frame as usize * 2) & 255) as isize / 16;
    let pixels = draw_sprite_scaled(dst, w, h, &atlas.ship, x, y, size, size, 255, false, false);
    counters.record_sprite(pixels);
    for i in 0..7 {
        let px = (i * w / 6) as isize - 14;
        fill_rect(dst, w, h, px, h as isize / 2 - 70, 28, 160, color(4, 8, 18));
        fill_rect(
            dst,
            w,
            h,
            px + 5,
            h as isize / 2 - 66,
            6,
            152,
            color(28, 38, 62),
        );
    }
    let glint_x = ((frame as usize * 3) % (w + 12)) as isize - 6;
    fill_rect(
        dst,
        w,
        h,
        glint_x,
        h as isize - 14,
        12,
        3,
        color(220, 238, 255),
    );
}

fn render_clipping_window(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let clip_w = (w / 3).max(24).min(w.max(1));
    let clip_h = (h / 3).max(18).min(h.max(1));
    let max_x = w.saturating_sub(clip_w).max(1);
    let clip_x = triangle((frame as usize * 2) & 255) as usize * max_x / 255;
    let clip_y = h.saturating_sub(clip_h) / 2;
    fill_rect(
        dst,
        w,
        h,
        clip_x as isize - 3,
        clip_y as isize - 3,
        clip_w + 6,
        clip_h + 6,
        color(225, 225, 190),
    );
    fill_rect(
        dst,
        w,
        h,
        clip_x as isize,
        clip_y as isize,
        clip_w,
        clip_h,
        color(12, 20, 42),
    );
    let size = (h / 3).max(20);
    let x = ((frame as usize * 5) % (w + size)) as isize - size as isize / 2;
    let y = h as isize / 2 - size as isize / 2;
    let pixels = draw_sprite_scaled_clipped(
        dst,
        w,
        h,
        &atlas.ship,
        x,
        y,
        size,
        size,
        255,
        false,
        false,
        clip_x,
        clip_y,
        clip_w,
        clip_h,
    );
    counters.record_sprite(pixels);
}

fn render_drop_shadow_copy(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let size = (h / 4).max(24);
    let x =
        w as isize / 2 - size as isize / 2 + triangle((frame as usize * 2) & 255) as isize / 5 - 51;
    let y = h as isize / 2 - size as isize / 2;
    let shadow = draw_sprite_scaled(
        dst,
        w,
        h,
        &atlas.ship,
        x + 12,
        y + 14,
        size,
        size,
        55,
        false,
        false,
    );
    counters.record_sprite(shadow);
    let pixels = draw_sprite_scaled(dst, w, h, &atlas.ship, x, y, size, size, 255, false, false);
    counters.record_sprite(pixels);
}

fn render_blob_contact_shadow(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let size = (h / 4).max(24);
    let bob = triangle((frame as usize * 4) & 255) as isize / 5;
    let x = w as isize / 2 - size as isize / 2;
    let y = h as isize / 2 - size as isize / 2 - bob / 2;
    draw_ellipse(
        dst,
        w,
        h,
        w as isize / 2,
        h as isize / 2 + size as isize / 2,
        size as isize / 2 + 18 - bob / 6,
        size as isize / 8,
        color(2, 4, 10),
    );
    let pixels = draw_sprite_scaled(dst, w, h, &atlas.ship, x, y, size, size, 255, false, false);
    counters.record_sprite(pixels);
}

fn render_invincibility_flicker(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let size = (h / 4).max(24);
    let x =
        w as isize / 2 - size as isize / 2 + triangle((frame as usize * 2) & 255) as isize / 6 - 42;
    let y = h as isize / 2 - size as isize / 2;
    if (frame / 5) & 1 == 0 {
        let pixels =
            draw_sprite_scaled(dst, w, h, &atlas.ship, x, y, size, size, 255, false, false);
        counters.record_sprite(pixels);
    } else {
        counters.flicker_skip_count += 1;
        fill_rect(
            dst,
            w,
            h,
            x,
            y + size as isize / 2,
            size,
            3,
            color(255, 255, 210),
        );
    }
}

fn render_afterimage_trail(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let size = (h / 5).max(20);
    let x = ((frame as usize * 5) % (w + size * 2)) as isize - size as isize;
    let y = h as isize / 2 - size as isize / 2;
    for i in (1..=6).rev() {
        let tint = (55 + i * 22) as u8;
        let pixels = draw_sprite_scaled(
            dst,
            w,
            h,
            &atlas.ship,
            x - i as isize * 18,
            y,
            size,
            size,
            tint,
            false,
            false,
        );
        counters.record_sprite(pixels);
    }
    let pixels = draw_sprite_scaled(dst, w, h, &atlas.ship, x, y, size, size, 255, false, false);
    counters.record_sprite(pixels);
}

fn render_motion_smear(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let size = (h / 4).max(24);
    let x =
        w as isize / 2 - size as isize / 2 + triangle((frame as usize * 4) & 255) as isize / 3 - 85;
    let y = h as isize / 2 - size as isize / 2;
    for i in 0..9 {
        let tint = (80 + i * 18).min(255) as u8;
        let pixels = draw_sprite_scaled(
            dst,
            w,
            h,
            &atlas.ship,
            x - i as isize * 10,
            y + i as isize % 2,
            size,
            size,
            tint,
            false,
            false,
        );
        counters.record_sprite(pixels);
    }
}

fn render_exploding_debris(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let phase = (frame % 96) as isize;
    let cx = w as isize / 2;
    let cy = h as isize / 2;
    for i in 0..64usize {
        let hx = hash(i as u32 * 17);
        let hy = hash(i as u32 * 37);
        let vx = (hx as isize & 63) - 31;
        let vy = (hy as isize & 63) - 31;
        let x = cx + vx * phase / 18 - 6;
        let y = cy + vy * phase / 18 - 6 + phase * phase / 180;
        let size = 6 + (i & 7);
        let pixels = draw_sprite_scaled(
            dst,
            w,
            h,
            &atlas.chunk,
            x,
            y,
            size,
            size,
            255,
            i & 1 == 0,
            i & 2 == 0,
        );
        counters.record_sprite(pixels);
        counters.particle_count += 1;
    }
}

fn render_tile_chunks(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let phase = (frame % 120) as isize;
    let tile = (w.min(h) / 14).max(8);
    for row in 0..5 {
        for col in 0..9 {
            let target_x = w as isize / 2 + (col as isize - 4) * tile as isize;
            let target_y = h as isize / 2 + (row as isize - 2) * tile as isize;
            let vx = (col as isize - 4) * 5;
            let vy = (row as isize - 2) * 5 - 12;
            let x = target_x + vx * phase / 10;
            let y = target_y + vy * phase / 10 + phase * phase / 140;
            let pixels = draw_sprite_scaled(
                dst,
                w,
                h,
                &atlas.tile,
                x,
                y,
                tile,
                tile,
                255,
                (row + col) & 1 == 0,
                false,
            );
            counters.record_sprite(pixels);
            counters.particle_count += 1;
        }
    }
}

fn render_sparkle_burst(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let phase = (frame % 90) as isize;
    let cx = w as isize / 2;
    let cy = h as isize / 2;
    for i in 0..96usize {
        let angle_x = (hash(i as u32 * 91) as isize & 127) - 63;
        let angle_y = (hash(i as u32 * 41) as isize & 127) - 63;
        let x = cx + angle_x * phase / 28;
        let y = cy + angle_y * phase / 28;
        let size = 5 + ((i + phase as usize) & 7);
        let tint = 130 + ((i * 11 + frame as usize) & 95) as u8;
        let pixels = draw_sprite_scaled(
            dst,
            w,
            h,
            &atlas.sparkle,
            x,
            y,
            size,
            size,
            tint,
            false,
            false,
        );
        counters.record_sprite(pixels);
        counters.particle_count += 1;
    }
}

fn render_bullet_hell(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let cx = w as isize / 2;
    let cy = h as isize / 2;
    for i in 0..180usize {
        let ring = 24 + (i % 12) as isize * 10 + (frame as isize * (1 + (i & 3) as isize)) % 120;
        let sx = ((hash(i as u32 * 13) & 127) as isize - 63) * ring / 64;
        let sy = ((hash(i as u32 * 29) & 127) as isize - 63) * ring / 64;
        let x = (cx + sx).rem_euclid(w.max(1) as isize);
        let y = (cy + sy).rem_euclid(h.max(1) as isize);
        let size = 5 + (i & 5);
        let pixels = draw_sprite_scaled(
            dst,
            w,
            h,
            &atlas.bullet,
            x,
            y,
            size,
            size,
            255,
            false,
            false,
        );
        counters.record_sprite(pixels);
        counters.particle_count += 1;
    }
}

fn render_card_texture(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    images: &[CameraImage],
    frame: u64,
) {
    if images.is_empty() {
        return;
    }
    let img = &images[(frame as usize / 90) % images.len()];
    let out_w = (w / 4).max(32);
    let out_h = (h / 3).max(42);
    let x = w as isize / 2 - out_w as isize / 2;
    let y = h as isize / 2 - out_h as isize / 2;
    blit_image_scaled(dst, w, h, img, x, y, out_w, out_h, 95);
}

fn render_rotating_card(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let phase = triangle((frame as usize * 3) & 255) as usize;
    let card_h = (h / 3).max(46);
    let card_w = ((w / 5).max(28) * (35 + phase) / 290).max(10);
    let x = w as isize / 2 - card_w as isize / 2;
    let y = h as isize / 2 - card_h as isize / 2;
    let pixels = draw_sprite_scaled(
        dst,
        w,
        h,
        &atlas.card,
        x,
        y,
        card_w,
        card_h,
        255,
        phase > 128,
        false,
    );
    counters.record_sprite(pixels);
    fill_rect(dst, w, h, x - 3, y - 3, card_w + 6, 2, color(255, 250, 210));
    fill_rect(
        dst,
        w,
        h,
        x - 3,
        y + card_h as isize + 1,
        card_w + 6,
        2,
        color(80, 48, 68),
    );
}

fn render_flipbook_logo(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let idx = (frame as usize / 10) % atlas.logo_frames.len();
    let out_w = (w / 2).max(72);
    let out_h = (h / 5).max(24);
    let x = w as isize / 2 - out_w as isize / 2;
    let y = h as isize / 2 - out_h as isize / 2
        + triangle((frame as usize * 2) & 255) as isize / 24
        - 5;
    let pixels = draw_sprite_scaled(
        dst,
        w,
        h,
        &atlas.logo_frames[idx],
        x,
        y,
        out_w,
        out_h,
        255,
        false,
        false,
    );
    counters.record_sprite(pixels);
}

fn render_palette_variants(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let size = (h / 5).max(18);
    let gap = (w / 8).max(size + 4);
    for (i, sprite) in atlas.ship_variants.iter().enumerate() {
        let x =
            w as isize / 2 - (gap * atlas.ship_variants.len()) as isize / 2 + (i * gap) as isize;
        let y = h as isize / 2 - size as isize / 2
            + triangle((frame as usize * 3 + i * 45) & 255) as isize / 16
            - 8;
        let pixels = draw_sprite_scaled(dst, w, h, sprite, x, y, size, size, 255, false, false);
        counters.record_sprite(pixels);
    }
}

fn render_reflection_texture(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    images: &[CameraImage],
    frame: u64,
) {
    if let Some(img) = images.get((frame as usize / 120) % images.len().max(1)) {
        blit_image_scaled_cells(dst, w, h, img, 0, h as isize / 2, w, h / 2, 70, 4, 2);
    }
}

fn render_mirrored_reflection(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    let size = (h / 4).max(24);
    let x =
        w as isize / 2 - size as isize / 2 + triangle((frame as usize * 2) & 255) as isize / 6 - 42;
    let y = h as isize / 2 - size as isize;
    let pixels = draw_sprite_scaled(dst, w, h, &atlas.ship, x, y, size, size, 255, false, false);
    counters.record_sprite(pixels);
    let refl = draw_sprite_scaled(
        dst,
        w,
        h,
        &atlas.ship,
        x,
        y + size as isize + 4,
        size,
        size / 2,
        78,
        false,
        true,
    );
    counters.record_sprite(refl);
}

fn render_object_overload(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    atlas: &SpriteAtlas,
    frame: u64,
    counters: &mut SpriteCounters,
) {
    for i in 0..320usize {
        if (i + frame as usize / 3).is_multiple_of(5) {
            counters.flicker_skip_count += 1;
            continue;
        }
        let x = ((hash(i as u32 * 101) as usize + frame as usize * (1 + i % 5)) % (w + 24))
            as isize
            - 12;
        let y = ((hash(i as u32 * 211) as usize + frame as usize * (2 + i % 3)) % (h + 24))
            as isize
            - 12;
        let sprite = if i & 1 == 0 {
            &atlas.bullet
        } else {
            &atlas.sparkle
        };
        let size = 5 + (i & 9);
        let pixels = draw_sprite_scaled(dst, w, h, sprite, x, y, size, size, 255, false, false);
        counters.record_sprite(pixels);
        counters.particle_count += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_image_scaled(
    dst: &mut [CameraPixel],
    screen_w: usize,
    screen_h: usize,
    img: &CameraImage,
    x: isize,
    y: isize,
    out_w: usize,
    out_h: usize,
    tint: u8,
) {
    if out_w == 0 || out_h == 0 || img.w == 0 || img.h == 0 {
        return;
    }
    let dx0 = x.max(0) as usize;
    let dy0 = y.max(0) as usize;
    let dx1 = (x + out_w as isize).clamp(0, screen_w as isize) as usize;
    let dy1 = (y + out_h as isize).clamp(0, screen_h as isize) as usize;
    if dx1 <= dx0 || dy1 <= dy0 {
        return;
    }
    let step_x = ((img.w << 16) / out_w.max(1)).max(1);
    let step_y = ((img.h << 16) / out_h.max(1)).max(1);
    let base_x = (dx0 as isize - x).max(0) as usize * step_x;
    let mut sy_fp = (dy0 as isize - y).max(0) as usize * step_y;
    for dy in dy0..dy1 {
        let sy = (sy_fp >> 16).min(img.h - 1);
        let mut sx_fp = base_x;
        let dst_row = dy * screen_w;
        let src_row = sy * img.stride;
        for dx in dx0..dx1 {
            let sx = (sx_fp >> 16).min(img.w - 1);
            let px = img.pixels[src_row + sx];
            dst[dst_row + dx] = if tint == 255 {
                px
            } else {
                blend(dst[dst_row + dx], px, tint)
            };
            sx_fp += step_x;
        }
        sy_fp += step_y;
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_image_scaled_cells(
    dst: &mut [CameraPixel],
    screen_w: usize,
    screen_h: usize,
    img: &CameraImage,
    x: isize,
    y: isize,
    out_w: usize,
    out_h: usize,
    tint: u8,
    cell_w: usize,
    cell_h: usize,
) {
    if out_w == 0 || out_h == 0 || img.w == 0 || img.h == 0 {
        return;
    }
    let cell_w = cell_w.max(1);
    let cell_h = cell_h.max(1);
    let dx0 = x.max(0) as usize;
    let dy0 = y.max(0) as usize;
    let dx1 = (x + out_w as isize).clamp(0, screen_w as isize) as usize;
    let dy1 = (y + out_h as isize).clamp(0, screen_h as isize) as usize;
    if dx1 <= dx0 || dy1 <= dy0 {
        return;
    }
    for dy in (dy0..dy1).step_by(cell_h) {
        let sy = ((dy as isize - y).max(0) as usize * img.h / out_h).min(img.h - 1);
        for dx in (dx0..dx1).step_by(cell_w) {
            let sx = ((dx as isize - x).max(0) as usize * img.w / out_w).min(img.w - 1);
            let px = img.pixels[sy * img.stride + sx];
            let c = if tint == 255 {
                px
            } else {
                blend(dst[dy * screen_w + dx], px, tint)
            };
            fill_rect(
                dst,
                screen_w,
                screen_h,
                dx as isize,
                dy as isize,
                cell_w,
                cell_h,
                c,
            );
        }
    }
}

fn fade_lower_half(dst: &mut [CameraPixel], w: usize, h: usize, c: CameraPixel, amount: u8) {
    let scan_gap = 3usize;
    let tint = tint_pixel(c, amount.max(1));
    for y in (h / 2..h).step_by(scan_gap) {
        dst[y * w..(y + 1) * w].fill(tint);
    }
}

fn draw_label(dst: &mut [CameraPixel], w: usize, h: usize, text: &str) {
    let bg_h = 18usize.min(h);
    fill_rect(dst, w, h, 0, 0, w, bg_h, color(0, 0, 0));
    let fg = color(255, 245, 170);
    let mut x0 = 6usize;
    for ch in text.chars().take(120) {
        if x0 + 6 >= w {
            break;
        }
        let glyph = glyph5x7(ch);
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    put(dst, w, h, (x0 + col) as isize, (5 + row) as isize, fg);
                }
            }
        }
        x0 += 6;
    }
}

fn glyph5x7(ch: char) -> [u8; 7] {
    match ch {
        '-' | '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        '/' => [0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0, 0],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b11100,
        ],
        'a' => [0, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111, 0],
        'b' => [0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b11110, 0],
        'c' => [0, 0b01111, 0b10000, 0b10000, 0b10000, 0b01111, 0],
        'd' => [0b00001, 0b00001, 0b01111, 0b10001, 0b10001, 0b01111, 0],
        'e' => [0, 0b01110, 0b10001, 0b11111, 0b10000, 0b01111, 0],
        'f' => [0b00110, 0b01000, 0b11100, 0b01000, 0b01000, 0b01000, 0],
        'g' => [0, 0b01111, 0b10001, 0b01111, 0b00001, 0b11110, 0],
        'h' => [0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0],
        'i' => [0b00100, 0, 0b01100, 0b00100, 0b00100, 0b01110, 0],
        'j' => [0b00010, 0, 0b00110, 0b00010, 0b10010, 0b10010, 0b01100],
        'k' => [0b10000, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0],
        'l' => [0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110, 0],
        'm' => [0, 0b11010, 0b10101, 0b10101, 0b10101, 0b10101, 0],
        'n' => [0, 0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0],
        'o' => [0, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110, 0],
        'p' => [0, 0b11110, 0b10001, 0b11110, 0b10000, 0b10000, 0],
        'q' => [0, 0b01110, 0b10001, 0b10001, 0b10101, 0b01110, 0b00011],
        'r' => [0, 0b10110, 0b11001, 0b10000, 0b10000, 0b10000, 0],
        's' => [0, 0b01111, 0b10000, 0b01110, 0b00001, 0b11110, 0],
        't' => [0b01000, 0b11100, 0b01000, 0b01000, 0b01001, 0b00110, 0],
        'u' => [0, 0b10001, 0b10001, 0b10001, 0b10001, 0b01111, 0],
        'v' => [0, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100, 0],
        'w' => [0, 0b10001, 0b10001, 0b10101, 0b10101, 0b01010, 0],
        'x' => [0, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0],
        'y' => [0, 0b10001, 0b10001, 0b01111, 0b00001, 0b11110, 0],
        'z' => [0, 0b11111, 0b00010, 0b00100, 0b01000, 0b11111, 0],
        _ => [0, 0, 0, 0, 0, 0, 0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash_frame(pixels: &[CameraPixel]) -> u64 {
        pixels.iter().fold(0xcbf2_9ce4_8422_2325, |acc, px| {
            (acc ^ px.0 as u64).wrapping_mul(0x1000_0000_01b3)
        })
    }

    #[test]
    fn labels_parse_in_stable_order() {
        let labels = SpriteEffectKind::labels();
        assert!(labels.contains("sprite-zoom-toward-camera"));
        assert!(labels.contains("object-overload-flicker"));
        assert_eq!(SpriteEffectKind::all().len(), 20);
        assert_eq!(
            SpriteEffectKind::all()[0].label(),
            "sprite-zoom-toward-camera"
        );
        assert_eq!(
            SpriteEffectKind::all()[19].label(),
            "object-overload-flicker"
        );
        for kind in SpriteEffectKind::all() {
            assert_eq!(SpriteEffectKind::parse(kind.label()), Some(*kind));
            assert_eq!(
                SpriteEffectKind::parse(&kind.label().replace('-', "_")),
                Some(*kind)
            );
        }
        assert!(SpriteEffectKind::parse("bogus").is_none());
    }

    #[test]
    fn renders_every_effect_deterministically_and_nonblank() {
        let w = 96;
        let h = 54;
        let images = synthetic_sprite_images(4);
        for &kind in SpriteEffectKind::all() {
            let mut state_a = SpriteEffectRenderState::new(w, h);
            let mut state_b = SpriteEffectRenderState::new(w, h);
            let mut a = vec![CameraPixel(0); w * h];
            let mut b = vec![CameraPixel(0); w * h];
            render_sprite_effect_frame(&mut a, &mut state_a, w, h, &images, kind, 7, None);
            render_sprite_effect_frame(&mut b, &mut state_b, w, h, &images, kind, 7, None);
            assert_eq!(a, b, "{kind:?} should be deterministic");
            assert!(a.iter().any(|px| px.0 != 0), "{kind:?} should draw pixels");
        }
    }

    #[test]
    fn animated_effects_change_between_frames() {
        let w = 96;
        let h = 54;
        let images = synthetic_sprite_images(4);
        for &kind in SpriteEffectKind::all() {
            let mut state = SpriteEffectRenderState::new(w, h);
            let mut a = vec![CameraPixel(0); w * h];
            let mut b = vec![CameraPixel(0); w * h];
            render_sprite_effect_frame(&mut a, &mut state, w, h, &images, kind, 0, None);
            render_sprite_effect_frame(&mut b, &mut state, w, h, &images, kind, 60, None);
            assert_ne!(
                hash_frame(&a),
                hash_frame(&b),
                "{kind:?} should visibly animate"
            );
        }
    }

    #[test]
    fn stats_draw_sum_matches_buckets() {
        let w = 64;
        let h = 36;
        let images = synthetic_sprite_images(2);
        let mut state = SpriteEffectRenderState::new(w, h);
        let mut frame = vec![CameraPixel(0); w * h];
        let stats = render_sprite_effect_frame(
            &mut frame,
            &mut state,
            w,
            h,
            &images,
            SpriteEffectKind::BulletHellOrnaments,
            12,
            Some("bullet-hell-ornaments"),
        );
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

    #[test]
    fn sprite_counters_cover_particles_and_flicker() {
        let w = 96;
        let h = 54;
        let images = synthetic_sprite_images(1);
        let mut state = SpriteEffectRenderState::new(w, h);
        let mut frame = vec![CameraPixel(0); w * h];
        let particles = render_sprite_effect_frame(
            &mut frame,
            &mut state,
            w,
            h,
            &images,
            SpriteEffectKind::BulletHellOrnaments,
            20,
            None,
        );
        assert!(particles.particle_count >= 100);
        assert!(particles.sprite_count > 0);
        assert!(particles.sprite_pixels > 0);

        let overload = render_sprite_effect_frame(
            &mut frame,
            &mut state,
            w,
            h,
            &images,
            SpriteEffectKind::ObjectOverloadFlicker,
            20,
            None,
        );
        assert!(overload.flicker_skip_count > 0);
        assert!(overload.sprite_count > 0);
    }

    #[test]
    fn small_sizes_do_not_panic() {
        let images = synthetic_sprite_images(1);
        for &kind in SpriteEffectKind::all() {
            let w = 8;
            let h = 6;
            let mut state = SpriteEffectRenderState::new(w, h);
            let mut frame = vec![CameraPixel(0); w * h];
            render_sprite_effect_frame(&mut frame, &mut state, w, h, &images, kind, 3, None);
        }
    }
}
