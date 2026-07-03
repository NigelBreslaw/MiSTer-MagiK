//! Host-testable classic arcade camera/background effects.
#![allow(clippy::too_many_arguments)]

use std::time::Instant;

use super::render_helpers::{clear, elapsed_us, time};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct CameraPixel(pub u16);

#[derive(Clone, Debug)]
pub struct CameraImage {
    pub pixels: Vec<CameraPixel>,
    pub w: usize,
    pub h: usize,
    pub stride: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraEffectKind {
    MultiLayerParallax,
    ForegroundSilhouettes,
    FixedStarfieldScroll,
    SpriteStarfield,
    LineScrollDepthBands,
    ColumnScrollShimmer,
    RowScrollWater,
    HeatHazeRaster,
    PerspectiveRoad,
    CheckerboardFloor,
    TubeScaling,
    RotatingTunnelRings,
    Pseudo3dHorizonBend,
    Mode7RotatingFloor,
    RotateZoomBackground,
    InfiniteCloudBank,
    CityLightsParallax,
    RainSnowLayers,
    ForegroundObstructionWipe,
    IsometricTileDrift,
}

impl CameraEffectKind {
    pub const ALL: [Self; 20] = [
        Self::MultiLayerParallax,
        Self::ForegroundSilhouettes,
        Self::FixedStarfieldScroll,
        Self::SpriteStarfield,
        Self::LineScrollDepthBands,
        Self::ColumnScrollShimmer,
        Self::RowScrollWater,
        Self::HeatHazeRaster,
        Self::PerspectiveRoad,
        Self::CheckerboardFloor,
        Self::TubeScaling,
        Self::RotatingTunnelRings,
        Self::Pseudo3dHorizonBend,
        Self::Mode7RotatingFloor,
        Self::RotateZoomBackground,
        Self::InfiniteCloudBank,
        Self::CityLightsParallax,
        Self::RainSnowLayers,
        Self::ForegroundObstructionWipe,
        Self::IsometricTileDrift,
    ];

    pub fn all() -> &'static [Self] {
        &Self::ALL
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::MultiLayerParallax => "multi-layer-parallax",
            Self::ForegroundSilhouettes => "foreground-silhouettes",
            Self::FixedStarfieldScroll => "fixed-starfield-scroll",
            Self::SpriteStarfield => "sprite-starfield",
            Self::LineScrollDepthBands => "line-scroll-depth-bands",
            Self::ColumnScrollShimmer => "column-scroll-shimmer",
            Self::RowScrollWater => "row-scroll-water",
            Self::HeatHazeRaster => "heat-haze-raster",
            Self::PerspectiveRoad => "perspective-road",
            Self::CheckerboardFloor => "checkerboard-floor",
            Self::TubeScaling => "tube-scaling",
            Self::RotatingTunnelRings => "rotating-tunnel-rings",
            Self::Pseudo3dHorizonBend => "pseudo3d-horizon-bend",
            Self::Mode7RotatingFloor => "mode7-rotating-floor",
            Self::RotateZoomBackground => "rotate-zoom-background",
            Self::InfiniteCloudBank => "infinite-cloud-bank",
            Self::CityLightsParallax => "city-lights-parallax",
            Self::RainSnowLayers => "rain-snow-layers",
            Self::ForegroundObstructionWipe => "foreground-obstruction-wipe",
            Self::IsometricTileDrift => "isometric-tile-drift",
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
pub struct CameraEffectFrameStats {
    pub clear_us: u64,
    pub background_us: u64,
    pub projection_us: u64,
    pub image_blit_us: u64,
    pub sprite_us: u64,
    pub post_us: u64,
    pub hud_us: u64,
}

impl CameraEffectFrameStats {
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

pub struct CameraEffectRenderState {
    scratch: Vec<CameraPixel>,
    layer: Vec<CameraPixel>,
    stars: Vec<Star>,
    w: usize,
    h: usize,
}

impl CameraEffectRenderState {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            scratch: vec![CameraPixel(0); w * h],
            layer: vec![CameraPixel(0); w * h],
            stars: build_stars(w, h, 512),
            w,
            h,
        }
    }

    fn resize(&mut self, w: usize, h: usize) {
        if self.w == w && self.h == h {
            return;
        }
        self.scratch.resize(w * h, CameraPixel(0));
        self.layer.resize(w * h, CameraPixel(0));
        self.stars = build_stars(w, h, 512);
        self.w = w;
        self.h = h;
    }
}

#[derive(Clone, Copy)]
struct Star {
    x: i32,
    y: i32,
    z: u16,
    speed: u16,
    color: CameraPixel,
}

pub fn render_camera_effect_frame(
    dst: &mut [CameraPixel],
    state: &mut CameraEffectRenderState,
    w: usize,
    h: usize,
    images: &[CameraImage],
    kind: CameraEffectKind,
    frame: u64,
    hud: Option<&str>,
) -> CameraEffectFrameStats {
    assert_eq!(dst.len(), w * h);
    state.resize(w, h);
    let mut stats = CameraEffectFrameStats::default();

    let t = Instant::now();
    clear(dst, color(1, 3, 11));
    stats.clear_us = elapsed_us(t);

    match kind {
        CameraEffectKind::MultiLayerParallax => {
            time(&mut stats.background_us, || {
                draw_gradient(dst, w, h, frame, 12)
            });
            time(&mut stats.image_blit_us, || {
                render_parallax_layers(dst, w, h, images, frame)
            });
        }
        CameraEffectKind::ForegroundSilhouettes => {
            time(&mut stats.background_us, || {
                draw_gradient(dst, w, h, frame, 18)
            });
            time(&mut stats.image_blit_us, || {
                render_parallax_layers(dst, w, h, images, frame / 2)
            });
            time(&mut stats.sprite_us, || draw_silhouettes(dst, w, h, frame));
        }
        CameraEffectKind::FixedStarfieldScroll => {
            time(&mut stats.background_us, || {
                draw_scroll_bands(dst, w, h, frame, 4)
            });
            time(&mut stats.sprite_us, || {
                draw_fixed_starfield(dst, w, h, frame)
            });
            time(&mut stats.image_blit_us, || {
                render_parallax_layers_tinted(dst, w, h, images, frame, 150)
            });
        }
        CameraEffectKind::SpriteStarfield => {
            time(&mut stats.background_us, || clear(dst, color(0, 0, 12)));
            time(&mut stats.sprite_us, || {
                draw_sprite_starfield(dst, state, w, h, frame)
            });
        }
        CameraEffectKind::LineScrollDepthBands => {
            time(&mut stats.background_us, || {
                render_line_scroll_bands(dst, w, h, frame)
            });
            time(&mut stats.image_blit_us, || {
                render_parallax_layers_tinted(dst, w, h, images, frame / 2, 160)
            });
        }
        CameraEffectKind::ColumnScrollShimmer => {
            time(&mut stats.image_blit_us, || {
                render_column_shimmer(dst, w, h, images, frame)
            });
        }
        CameraEffectKind::RowScrollWater => {
            time(&mut stats.background_us, || {
                draw_gradient(dst, w, h, frame, 28)
            });
            time(&mut stats.projection_us, || {
                render_water_rows(dst, w, h, images, frame)
            });
        }
        CameraEffectKind::HeatHazeRaster => {
            time(&mut stats.background_us, || {
                draw_gradient(dst, w, h, frame, 44)
            });
            time(&mut stats.image_blit_us, || {
                render_parallax_layers_tinted(dst, w, h, images, frame, 220)
            });
            time(&mut stats.post_us, || heat_haze(dst, state, w, h, frame));
        }
        CameraEffectKind::PerspectiveRoad => {
            time(&mut stats.background_us, || draw_horizon(dst, w, h, frame));
            time(&mut stats.projection_us, || {
                render_perspective_road(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || draw_road_signs(dst, w, h, frame));
        }
        CameraEffectKind::CheckerboardFloor => {
            time(&mut stats.background_us, || {
                draw_horizon(dst, w, h, frame / 2)
            });
            time(&mut stats.projection_us, || {
                render_checkerboard_floor(dst, w, h, frame)
            });
        }
        CameraEffectKind::TubeScaling => {
            time(&mut stats.background_us, || clear(dst, color(0, 0, 8)));
            time(&mut stats.projection_us, || {
                render_tube(dst, w, h, images, frame, false)
            });
        }
        CameraEffectKind::RotatingTunnelRings => {
            time(&mut stats.background_us, || clear(dst, color(2, 0, 10)));
            time(&mut stats.projection_us, || {
                render_tunnel_rings(dst, w, h, frame)
            });
            time(&mut stats.image_blit_us, || {
                render_tube(dst, w, h, images, frame, true)
            });
        }
        CameraEffectKind::Pseudo3dHorizonBend => {
            time(&mut stats.background_us, || {
                render_bent_horizon(dst, w, h, frame)
            });
            time(&mut stats.projection_us, || {
                render_perspective_road(dst, w, h, frame / 2)
            });
        }
        CameraEffectKind::Mode7RotatingFloor => {
            time(&mut stats.background_us, || {
                draw_horizon(dst, w, h, frame / 3)
            });
            time(&mut stats.projection_us, || {
                render_mode7_floor(dst, w, h, images, frame)
            });
        }
        CameraEffectKind::RotateZoomBackground => {
            time(&mut stats.background_us, || clear(dst, color(4, 4, 18)));
            time(&mut stats.projection_us, || {
                render_rotate_zoom(dst, w, h, images, frame)
            });
        }
        CameraEffectKind::InfiniteCloudBank => {
            time(&mut stats.background_us, || {
                render_cloud_bank(dst, w, h, frame)
            });
        }
        CameraEffectKind::CityLightsParallax => {
            time(&mut stats.background_us, || {
                render_city_lights(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                draw_fixed_starfield(dst, w, h / 2, frame / 4)
            });
        }
        CameraEffectKind::RainSnowLayers => {
            time(&mut stats.background_us, || {
                draw_gradient(dst, w, h, frame / 4, 8)
            });
            time(&mut stats.image_blit_us, || {
                render_parallax_layers_tinted(dst, w, h, images, frame / 3, 140)
            });
            time(&mut stats.sprite_us, || render_weather(dst, w, h, frame));
        }
        CameraEffectKind::ForegroundObstructionWipe => {
            time(&mut stats.background_us, || {
                draw_gradient(dst, w, h, frame, 16)
            });
            time(&mut stats.image_blit_us, || {
                render_parallax_layers_tinted(dst, w, h, images, frame, 210)
            });
            time(&mut stats.sprite_us, || {
                render_obstruction_wipe(dst, w, h, frame)
            });
        }
        CameraEffectKind::IsometricTileDrift => {
            time(&mut stats.background_us, || clear(dst, color(3, 5, 16)));
            time(&mut stats.projection_us, || {
                render_isometric_drift(dst, w, h, images, frame)
            });
        }
    }

    if let Some(text) = hud {
        time(&mut stats.hud_us, || draw_label(dst, w, h, text));
    }

    stats
}

pub fn synthetic_images(count: usize) -> Vec<CameraImage> {
    let mut images = Vec::new();
    for idx in 0..count.max(1) {
        let w = 160usize;
        let h = 120usize;
        let mut pixels = vec![CameraPixel(0); w * h];
        for y in 0..h {
            for x in 0..w {
                let checker = ((x / 16 + y / 16 + idx) & 1) as u8;
                let r = ((idx * 37 + x * 2 + y) & 255) as u8;
                let g = ((idx * 53 + y * 3) & 255) as u8;
                let b = ((idx * 71 + x + y * 2) & 255) as u8;
                pixels[y * w + x] = if checker == 0 {
                    color(r / 2, g, b)
                } else {
                    color(r, g / 2, b / 2)
                };
            }
        }
        images.push(CameraImage {
            pixels,
            w,
            h,
            stride: w,
        });
    }
    images
}

pub fn color(r: u8, g: u8, b: u8) -> CameraPixel {
    CameraPixel(((r as u16 & 0xf8) << 8) | ((g as u16 & 0xfc) << 3) | ((b as u16) >> 3))
}

pub fn pixel_to_rgb888(pixel: CameraPixel) -> u32 {
    let r = ((pixel.0 >> 11) & 0x1f) as u32;
    let g = ((pixel.0 >> 5) & 0x3f) as u32;
    let b = (pixel.0 & 0x1f) as u32;
    ((r * 255 / 31) << 16) | ((g * 255 / 63) << 8) | (b * 255 / 31)
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

fn sample_image(images: &[CameraImage], idx: usize, x: usize, y: usize) -> CameraPixel {
    if images.is_empty() {
        return color(20, 80, 130);
    }
    let img = &images[idx % images.len()];
    img.pixels[(y.min(img.h - 1)) * img.stride + x.min(img.w - 1)]
}

fn blit_scaled(
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
    if out_w == 0 || out_h == 0 {
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
                tint_pixel(px, tint)
            };
            sx_fp += step_x;
        }
        sy_fp += step_y;
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

fn draw_gradient(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64, hue: u8) {
    for y in 0..h {
        let p = (y * 255 / h.max(1)) as u8;
        let wave = triangle((y / 4 + frame as usize / 2) & 255) / 7;
        let c = color(
            hue.saturating_add(wave / 2),
            p / 4 + wave / 3,
            34u8.saturating_add(p / 2),
        );
        dst[y * w..(y + 1) * w].fill(c);
    }
}

fn draw_horizon(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let sky = y < h / 2;
        let p = (y * 255 / h.max(1)) as u8;
        let c = if sky {
            color(8, 18 + p / 6, 46 + p / 3)
        } else {
            color(8 + p / 10, 22 + p / 8, 16 + p / 14)
        };
        dst[y * w..(y + 1) * w].fill(c);
    }
    let glow_y = (h as isize / 2 + triangle(frame as usize & 255) as isize / 18 - 7)
        .clamp(0, h.saturating_sub(1) as isize) as usize;
    for y in glow_y.saturating_sub(2)..(glow_y + 3).min(h) {
        dst[y * w..(y + 1) * w].fill(color(200, 75, 120));
    }
}

fn draw_scroll_bands(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64, speed: usize) {
    for y in 0..h {
        let band = ((y / 18 + frame as usize / speed.max(1)) & 7) as u8;
        let c = color(4 + band * 4, 16 + band * 9, 46 + band * 12);
        dst[y * w..(y + 1) * w].fill(c);
    }
}

fn render_parallax_layers(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    images: &[CameraImage],
    frame: u64,
) {
    render_parallax_layers_tinted(dst, w, h, images, frame, 230);
}

fn render_parallax_layers_tinted(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    images: &[CameraImage],
    frame: u64,
    tint: u8,
) {
    let layers = [(6usize, 90usize, 150u8), (3, 180, tint), (1, 290, 255)];
    for (layer, &(speed, y, layer_tint)) in layers.iter().enumerate() {
        let img_idx = layer + frame as usize / 240;
        if images.is_empty() {
            let x = -((frame as isize / speed as isize) % 220);
            for repeat in 0..6 {
                fill_rect(
                    dst,
                    w,
                    h,
                    x + repeat * 220,
                    y as isize,
                    180,
                    90,
                    color(20 + layer as u8 * 40, 70 + layer as u8 * 30, 120),
                );
            }
        } else {
            let img = &images[img_idx % images.len()];
            let out_w = 220 + layer * 80;
            let out_h = 150 + layer * 42;
            let offset = (frame as usize / speed) % out_w.max(1);
            let mut x = -(offset as isize);
            while x < w as isize {
                blit_scaled(dst, w, h, img, x, y as isize, out_w, out_h, layer_tint);
                x += out_w as isize + 22;
            }
        }
    }
}

fn draw_silhouettes(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    let base_y = h as isize - 88;
    let c = color(0, 0, 4);
    for i in 0..10 {
        let x = ((i * 123) - (frame as isize * (2 + (i & 1))) % (w as isize + 140)) + w as isize;
        let height = (72 + (i * 17 % 80)) as usize;
        fill_rect(dst, w, h, x, base_y - height as isize, 36, height, c);
        fill_rect(dst, w, h, x - 14, base_y - height as isize / 2, 64, 14, c);
    }
}

fn draw_fixed_starfield(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for i in 0..360usize {
        let x = (hash(i as u32 * 17) as usize + frame as usize * (1 + i % 3)) % w.max(1);
        let y = (hash(i as u32 * 31) as usize) % h.max(1);
        let b = 120 + ((i * 19) & 127) as u8;
        put(dst, w, h, x as isize, y as isize, color(b / 2, b, 255));
    }
}

fn build_stars(w: usize, h: usize, count: usize) -> Vec<Star> {
    (0..count)
        .map(|i| {
            let seed = i as u32;
            let hx = hash(seed.wrapping_mul(0x1f1f_0101));
            let hy = hash(seed.wrapping_mul(0x045d_9f3b));
            Star {
                x: (hx as usize % w.max(1)) as i32 - w as i32 / 2,
                y: (hy as usize % h.max(1)) as i32 - h as i32 / 2,
                z: (16 + (hash(i as u32 * 97) & 255)) as u16,
                speed: (2 + (i & 7)) as u16,
                color: color(80 + ((i * 29) & 127) as u8, 180, 255),
            }
        })
        .collect()
}

fn draw_sprite_starfield(
    dst: &mut [CameraPixel],
    state: &CameraEffectRenderState,
    w: usize,
    h: usize,
    frame: u64,
) {
    for star in &state.stars {
        let z = ((star.z as u64 + 255 - ((frame * star.speed as u64) & 255)) & 255).max(1);
        let x = w as i64 / 2 + star.x as i64 * 128 / z as i64;
        let y = h as i64 / 2 + star.y as i64 * 128 / z as i64;
        let size = if z < 32 {
            3
        } else if z < 96 {
            2
        } else {
            1
        };
        fill_rect(dst, w, h, x as isize, y as isize, size, size, star.color);
    }
}

fn render_line_scroll_bands(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let depth = (y * 8 / h.max(1)).max(1);
        let shift = ((triangle((frame as usize * depth / 2 + y / 3) & 255) as isize) - 128)
            * depth as isize
            / 10;
        let color_a = color(
            8 + depth as u8 * 10,
            20 + depth as u8 * 8,
            50 + depth as u8 * 12,
        );
        let color_b = color(40 + depth as u8 * 9, 12 + depth as u8 * 6, 80);
        for x in 0..w {
            dst[y * w + x] = if (((x as isize + shift).max(0) as usize / 38) & 1) == 0 {
                color_a
            } else {
                color_b
            };
        }
    }
}

fn render_column_shimmer(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    images: &[CameraImage],
    frame: u64,
) {
    let img = images.first();
    for x in 0..w {
        let offset = triangle((x / 4 + frame as usize * 2) & 255) as isize / 18 - 7;
        for y in 0..h {
            let yy = (y as isize + offset).rem_euclid(h as isize) as usize;
            dst[y * w + x] = if let Some(img) = img {
                let sx = x * img.w / w;
                let sy = yy * img.h / h;
                img.pixels[sy.min(img.h - 1) * img.stride + sx.min(img.w - 1)]
            } else {
                color((x * 255 / w) as u8, (yy * 255 / h) as u8, 160)
            };
        }
    }
}

fn render_water_rows(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    images: &[CameraImage],
    frame: u64,
) {
    let horizon = h / 2;
    if let Some(img) = images.first() {
        blit_scaled(dst, w, h, img, 210, 45, w.saturating_sub(420), 190, 220);
    }
    for y in horizon..h {
        let wave = triangle((y / 2 + frame as usize * 3) & 255) as isize / 8 - 16;
        let depth = y - horizon + 1;
        for x in 0..w {
            let sx = ((x as isize + wave).rem_euclid(w as isize)) as usize;
            let base = dst[(horizon.saturating_sub(depth / 3).min(h - 1)) * w + sx];
            dst[y * w + x] = blend(color(0, 20, 45), base, 120);
        }
    }
}

fn heat_haze(
    dst: &mut [CameraPixel],
    state: &mut CameraEffectRenderState,
    w: usize,
    h: usize,
    frame: u64,
) {
    state.scratch.copy_from_slice(dst);
    for y in 0..h {
        let shift = triangle((frame as usize * 4 + y / 3) & 255) as isize / 11 - 12;
        for x in 0..w {
            let sx = (x as isize + shift).clamp(0, w as isize - 1) as usize;
            dst[y * w + x] = state.scratch[y * w + sx];
        }
    }
}

fn render_perspective_road(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    let horizon = h / 2;
    for y in horizon..h {
        let depth = y - horizon + 1;
        let lane_w = (w / 12 + depth * 2).max(1);
        let center = w as isize / 2
            + (triangle((frame as usize / 2 + depth / 3) & 255) as isize - 128) * depth as isize
                / h.max(1) as isize;
        let stripe = ((depth * 6 + frame as usize * 7) / 34) & 1;
        for x in 0..w {
            let dx = (x as isize - center).unsigned_abs();
            dst[y * w + x] = if dx < lane_w {
                if stripe == 0 {
                    color(230, 230, 150)
                } else {
                    color(40, 44, 58)
                }
            } else if dx < lane_w * 3 {
                color(42, 42, 56)
            } else if ((dx / 34 + frame as usize / 8) & 1) == 0 {
                color(160, 42, 64)
            } else {
                color(230, 220, 180)
            };
        }
    }
}

fn draw_road_signs(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for i in 0..8usize {
        let z = ((i * 48 + frame as usize * 5) % 380).max(24);
        let scale = 320 / z.max(1);
        let x = if i & 1 == 0 {
            (w / 2).saturating_sub(z)
        } else {
            w / 2 + z / 2
        };
        let y = h / 2 + 280usize.saturating_sub(z);
        fill_rect(
            dst,
            w,
            h,
            x as isize,
            y as isize,
            10 + scale * 8,
            8 + scale * 6,
            color(255, 60, 90),
        );
    }
}

fn render_checkerboard_floor(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    let horizon = h / 2;
    for y in horizon..h {
        let depth = y - horizon + 1;
        let row_scale = (depth * depth / 48).max(1);
        let sy = (row_scale + frame as usize * 5) / 28;
        for x in 0..w {
            let sx = ((x as isize - w as isize / 2) * row_scale as isize / w.max(1) as isize)
                + frame as isize / 3;
            let checker = ((sx.div_euclid(16) + sy as isize) & 1) == 0;
            dst[y * w + x] = if checker {
                color(220, 70, 140)
            } else {
                color(20, 20, 36)
            };
        }
    }
}

fn render_tube(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    images: &[CameraImage],
    frame: u64,
    sparse: bool,
) {
    let cx = w as isize / 2;
    let cy = h as isize / 2;
    let max_r = w.min(h) / 2;
    for r in (18..max_r).step_by(if sparse { 44 } else { 22 }) {
        let phase = frame as usize * 3 + r;
        let c = color(
            40 + (triangle(phase & 255) / 2),
            70 + (triangle((phase + 80) & 255) / 3),
            150,
        );
        stroke_ring(dst, w, h, cx, cy, r as isize, c);
    }
    for i in 0..8 {
        if let Some(img) = images.get((i + frame as usize / 90) % images.len().max(1)) {
            let z = 36 + ((i * 43 + frame as usize * 3) % 280);
            let out_w = 420usize.saturating_sub(z).max(48);
            let out_h = out_w * 3 / 4;
            let x = cx + ((i as isize * 67 + frame as isize * 2) % 360) - 180 - out_w as isize / 2;
            let y = cy + ((i as isize * 43 + frame as isize) % 220) - 110 - out_h as isize / 2;
            blit_scaled(dst, w, h, img, x, y, out_w, out_h, 190);
        }
    }
}

fn render_tunnel_rings(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    let cx = w as isize / 2;
    let cy = h as isize / 2;
    for i in 0..18usize {
        let r = 14 + ((i * 29 + frame as usize * 4) % (w.min(h) / 2).max(1));
        let c = color(120 + (i * 7) as u8, 50 + (i * 11) as u8, 220);
        stroke_ring(dst, w, h, cx, cy, r as isize, c);
    }
}

fn stroke_ring(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    cx: isize,
    cy: isize,
    r: isize,
    c: CameraPixel,
) {
    let mut x = r;
    let mut y = 0;
    let mut err = 1 - x;
    while x >= y {
        for &(px, py) in &[
            (x, y),
            (y, x),
            (-y, x),
            (-x, y),
            (-x, -y),
            (-y, -x),
            (y, -x),
            (x, -y),
        ] {
            put(dst, w, h, cx + px, cy + py, c);
        }
        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }
}

fn render_bent_horizon(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let bend = triangle((frame as usize * 2 + y / 2) & 255) as isize / 6 - 21;
        for x in 0..w {
            let horizon =
                h as isize / 2 + bend * (x as isize - w as isize / 2).abs() / w.max(1) as isize;
            dst[y * w + x] = if y as isize <= horizon {
                color(4, 18 + (y * 40 / h) as u8, 66)
            } else {
                color(24, 34, 22)
            };
        }
    }
}

fn render_mode7_floor(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    images: &[CameraImage],
    frame: u64,
) {
    let horizon = h / 2;
    let img = images.first();
    let angle = frame as i32 & 255;
    let sin = triangle(angle as usize) as i32 - 128;
    let cos = triangle((angle as usize + 64) & 255) as i32 - 128;
    let mut y = horizon;
    while y < h {
        let depth = (y - horizon + 1) as i32;
        let scale = (8200 / depth.max(1)).max(2);
        for x in 0..w {
            let dx = x as i32 - w as i32 / 2;
            let tx = ((dx * cos - depth * sin) / scale + frame as i32 * 2).rem_euclid(256);
            let ty = ((dx * sin + depth * cos) / scale + frame as i32 * 3).rem_euclid(256);
            dst[y * w + x] = if let Some(img) = img {
                sample_image(
                    images,
                    0,
                    tx as usize * img.w / 256,
                    ty as usize * img.h / 256,
                )
            } else if (((tx / 24) + (ty / 24)) & 1) == 0 {
                color(40, 220, 180)
            } else {
                color(20, 24, 60)
            };
        }
        if y + 1 < h {
            let row = y * w;
            let next = (y + 1) * w;
            let (head, tail) = dst.split_at_mut(next);
            tail[..w].copy_from_slice(&head[row..row + w]);
        }
        y += 2;
    }
}

fn render_rotate_zoom(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    images: &[CameraImage],
    frame: u64,
) {
    let img = images.first();
    let angle = frame as i32 & 255;
    let sin = triangle(angle as usize) as i32 - 128;
    let cos = triangle((angle as usize + 64) & 255) as i32 - 128;
    let zoom = 90 + triangle((frame as usize * 2) & 255) as i32 / 3;
    for y in (0..h).step_by(2) {
        for x in 0..w {
            let dx = x as i32 - w as i32 / 2;
            let dy = y as i32 - h as i32 / 2;
            let tx = ((dx * cos - dy * sin) / zoom + 128).rem_euclid(256) as usize;
            let ty = ((dx * sin + dy * cos) / zoom + 128).rem_euclid(256) as usize;
            let px = if let Some(img) = img {
                sample_image(images, 0, tx * img.w / 256, ty * img.h / 256)
            } else {
                color(tx as u8, ty as u8, 180)
            };
            dst[y * w + x] = px;
        }
        if y + 1 < h {
            let row = y * w;
            let next = (y + 1) * w;
            let (head, tail) = dst.split_at_mut(next);
            tail[..w].copy_from_slice(&head[row..row + w]);
        }
    }
}

fn render_cloud_bank(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    draw_gradient(dst, w, h, frame / 3, 20);
    for layer in 0..4usize {
        let y = 60 + layer * 70;
        let speed = 1 + layer;
        let c = color(
            35 + layer as u8 * 20,
            72 + layer as u8 * 18,
            120 + layer as u8 * 12,
        );
        for i in 0..12usize {
            let x = ((i * 123 + frame as usize * speed) % (w + 180)) as isize - 140;
            fill_rect(dst, w, h, x, y as isize, 120, 18 + layer * 8, c);
            fill_rect(dst, w, h, x + 34, y as isize - 18, 90, 28 + layer * 4, c);
        }
    }
}

fn render_city_lights(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    draw_gradient(dst, w, h, frame / 4, 5);
    for layer in 0..4usize {
        let base_y = h as isize - 70 - layer as isize * 36;
        let speed = 1 + layer * 2;
        for b in 0..16usize {
            let x = ((b * 82 + frame as usize * speed) % (w + 100)) as isize - 80;
            let bh = 42 + ((b * 23 + layer * 19) % 120) as isize;
            fill_rect(
                dst,
                w,
                h,
                x,
                base_y - bh,
                42,
                bh as usize,
                color(4, 6, 18 + layer as u8 * 8),
            );
            for wy in (base_y - bh + 8..base_y - 4).step_by(13) {
                for wx in (x + 6..x + 36).step_by(11) {
                    if hash(
                        (wx as u32)
                            .wrapping_add(wy as u32)
                            .wrapping_add(frame as u32 / 20),
                    ) & 3
                        == 0
                    {
                        fill_rect(dst, w, h, wx, wy, 4, 4, color(255, 210, 80));
                    }
                }
            }
        }
    }
}

fn render_weather(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for i in 0..420usize {
        let x = ((hash(i as u32 * 37) as usize + frame as usize * (2 + i % 5)) % w) as isize;
        let y = ((hash(i as u32 * 73) as usize + frame as usize * (4 + i % 7)) % h) as isize;
        let rain = i & 3 != 0;
        if rain {
            fill_rect(dst, w, h, x, y, 1, 8 + i % 5, color(80, 160, 230));
        } else {
            fill_rect(dst, w, h, x, y, 2, 2, color(230, 240, 255));
        }
    }
}

fn render_obstruction_wipe(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    let c = color(0, 0, 2);
    for i in 0..8usize {
        let speed = 3 + i % 4;
        let x = ((i * 180 + frame as usize * speed) % (w + 220)) as isize - 160;
        let trunk_w = 18 + i % 3 * 8;
        fill_rect(dst, w, h, x, 0, trunk_w, h, c);
        fill_rect(dst, w, h, x - 60, 40 + (i * 23 % 160) as isize, 150, 30, c);
    }
}

fn render_isometric_drift(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    images: &[CameraImage],
    frame: u64,
) {
    let tile_w = 64isize;
    let tile_h = 32isize;
    let ox = w as isize / 2;
    let oy = 44isize + (triangle((frame as usize) & 255) as isize / 10 - 12);
    for gy in 0..16isize {
        for gx in -8..16isize {
            let sx = ox + (gx - gy) * tile_w / 2 - (frame as isize % tile_w);
            let sy = oy + (gx + gy) * tile_h / 2;
            let idx = (gx + gy * 3).unsigned_abs() + frame as usize / 120;
            let c = if images.is_empty() {
                let v = ((gx + gy + frame as isize / 20) & 7) as u8;
                color(20 + v * 22, 80 + v * 9, 130 + v * 5)
            } else {
                sample_image(
                    images,
                    idx,
                    (idx * 17) % images[idx % images.len()].w,
                    (idx * 29) % images[idx % images.len()].h,
                )
            };
            draw_diamond(dst, w, h, sx, sy, tile_w / 2, tile_h / 2, c);
        }
    }
}

fn draw_diamond(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    cx: isize,
    cy: isize,
    rx: isize,
    ry: isize,
    c: CameraPixel,
) {
    for yy in -ry..=ry {
        let span = rx * (ry - yy.abs()) / ry.max(1);
        fill_rect(dst, w, h, cx - span, cy + yy, (span * 2 + 1) as usize, 1, c);
    }
}

fn put(dst: &mut [CameraPixel], w: usize, h: usize, x: isize, y: isize, c: CameraPixel) {
    if x >= 0 && y >= 0 && x < w as isize && y < h as isize {
        dst[y as usize * w + x as usize] = c;
    }
}

fn triangle(v: usize) -> u8 {
    let t = (v & 255) as u8;
    if t < 128 {
        t.saturating_mul(2)
    } else {
        (255 - t).saturating_mul(2)
    }
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

fn draw_label(dst: &mut [CameraPixel], w: usize, h: usize, text: &str) {
    const CHAR_W: usize = 5;
    const CHAR_H: usize = 7;
    const GAP: usize = 1;
    const PAD: usize = 4;
    let label_w = text.len() * (CHAR_W + GAP) + PAD * 2;
    let label_h = CHAR_H + PAD * 2;
    if label_w >= w || label_h >= h {
        return;
    }
    let x0 = 6usize;
    let y0 = 6usize;
    fill_rect(
        dst,
        w,
        h,
        x0 as isize,
        y0 as isize,
        label_w,
        label_h,
        color(0, 0, 0),
    );
    for (i, ch) in text.chars().enumerate() {
        draw_char(dst, w, h, x0 + PAD + i * (CHAR_W + GAP), y0 + PAD, ch);
    }
}

fn draw_char(dst: &mut [CameraPixel], w: usize, h: usize, x0: usize, y0: usize, ch: char) {
    for (row, bits) in glyph5x7(ch).iter().enumerate() {
        for col in 0..5 {
            if (bits >> (4 - col)) & 1 != 0 {
                put(
                    dst,
                    w,
                    h,
                    (x0 + col) as isize,
                    (y0 + row) as isize,
                    color(255, 245, 170),
                );
            }
        }
    }
}

fn glyph5x7(ch: char) -> [u8; 7] {
    match ch {
        '-' | '_' => [0, 0, 0, 0, 0, 0, 0b11111],
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
        let labels = CameraEffectKind::labels();
        assert!(labels.contains("multi-layer-parallax"));
        assert!(labels.contains("isometric-tile-drift"));
        for kind in CameraEffectKind::all() {
            assert_eq!(CameraEffectKind::parse(kind.label()), Some(*kind));
            assert_eq!(
                CameraEffectKind::parse(&kind.label().replace('-', "_")),
                Some(*kind)
            );
        }
        assert_eq!(CameraEffectKind::all().len(), 20);
        assert!(CameraEffectKind::parse("bogus").is_none());
    }

    #[test]
    fn renders_every_effect_deterministically_and_nonblank() {
        let w = 96;
        let h = 54;
        let images = synthetic_images(4);
        for &kind in CameraEffectKind::all() {
            let mut state_a = CameraEffectRenderState::new(w, h);
            let mut state_b = CameraEffectRenderState::new(w, h);
            let mut a = vec![CameraPixel(0); w * h];
            let mut b = vec![CameraPixel(0); w * h];
            render_camera_effect_frame(&mut a, &mut state_a, w, h, &images, kind, 7, None);
            render_camera_effect_frame(&mut b, &mut state_b, w, h, &images, kind, 7, None);
            assert_eq!(a, b, "{kind:?} should be deterministic");
            assert!(a.iter().any(|px| px.0 != 0), "{kind:?} should draw pixels");
        }
    }

    #[test]
    fn animated_effects_change_between_frames() {
        let w = 96;
        let h = 54;
        let images = synthetic_images(4);
        for &kind in CameraEffectKind::all() {
            let mut state = CameraEffectRenderState::new(w, h);
            let mut a = vec![CameraPixel(0); w * h];
            let mut b = vec![CameraPixel(0); w * h];
            render_camera_effect_frame(&mut a, &mut state, w, h, &images, kind, 0, None);
            render_camera_effect_frame(&mut b, &mut state, w, h, &images, kind, 60, None);
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
        let images = synthetic_images(2);
        let mut state = CameraEffectRenderState::new(w, h);
        let mut frame = vec![CameraPixel(0); w * h];
        let stats = render_camera_effect_frame(
            &mut frame,
            &mut state,
            w,
            h,
            &images,
            CameraEffectKind::PerspectiveRoad,
            12,
            Some("perspective-road"),
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
}
