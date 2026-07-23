// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-testable retro framebuffer effects used by the device benchmark.

use std::sync::OnceLock;

pub const EFFECT_NAMES: &[&str] = &[
    "palette_cycle",
    "plasma",
    "copper_bars",
    "starfield",
    "crt_pass",
    "tile_parallax",
    "mode7_floor",
    "afterimage",
    "dither_spotlight",
    "wipe_transition",
    "chunky_distortion",
    "fire_haze",
    "vhs_glitch",
];

pub const EFFECT_SIZES: &[(usize, usize)] = &[
    (320, 180),
    (320, 224),
    (480, 270),
    (640, 360),
    (640, 448),
    (960, 540),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectKind {
    PaletteCycle,
    Plasma,
    CopperBars,
    Starfield,
    CrtPass,
    TileParallax,
    Mode7Floor,
    Afterimage,
    DitherSpotlight,
    WipeTransition,
    ChunkyDistortion,
    FireHaze,
    VhsGlitch,
}

impl EffectKind {
    pub fn all() -> &'static [Self] {
        &[
            Self::PaletteCycle,
            Self::Plasma,
            Self::CopperBars,
            Self::Starfield,
            Self::CrtPass,
            Self::TileParallax,
            Self::Mode7Floor,
            Self::Afterimage,
            Self::DitherSpotlight,
            Self::WipeTransition,
            Self::ChunkyDistortion,
            Self::FireHaze,
            Self::VhsGlitch,
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::PaletteCycle => "palette_cycle",
            Self::Plasma => "plasma",
            Self::CopperBars => "copper_bars",
            Self::Starfield => "starfield",
            Self::CrtPass => "crt_pass",
            Self::TileParallax => "tile_parallax",
            Self::Mode7Floor => "mode7_floor",
            Self::Afterimage => "afterimage",
            Self::DitherSpotlight => "dither_spotlight",
            Self::WipeTransition => "wipe_transition",
            Self::ChunkyDistortion => "chunky_distortion",
            Self::FireHaze => "fire_haze",
            Self::VhsGlitch => "vhs_glitch",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::all().iter().copied().find(|k| k.name() == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectSize {
    pub w: usize,
    pub h: usize,
}

impl EffectSize {
    pub fn parse(s: &str) -> Option<Self> {
        let (w, h) = s.split_once('x')?;
        let w = w.parse().ok()?;
        let h = h.parse().ok()?;
        let size = Self { w, h };
        supported_size(size).then_some(size)
    }

    pub fn scale_to_1080p(self) -> Option<usize> {
        integer_scale_to_1080p(self.w, self.h)
    }

    pub fn scale_to_half_1080p(self) -> Option<usize> {
        integer_scale_to_rect(self.w, self.h, 960, 540)
    }
}

pub fn supported_size(size: EffectSize) -> bool {
    EFFECT_SIZES.contains(&(size.w, size.h))
}

pub fn integer_scale_to_1080p(w: usize, h: usize) -> Option<usize> {
    integer_scale_to_rect(w, h, 1920, 1080)
}

pub fn integer_scale_to_rect(
    w: usize,
    h: usize,
    target_w: usize,
    target_h: usize,
) -> Option<usize> {
    let sx = target_w / w;
    let sy = target_h / h;
    (sx == sy && sx > 0 && w * sx == target_w && h * sy == target_h).then_some(sx)
}

pub struct EffectState {
    kind: EffectKind,
    size: EffectSize,
    aux: Vec<u32>,
    heat: Vec<u8>,
    scratch: Vec<i32>,
}

impl EffectState {
    pub fn new(kind: EffectKind, size: EffectSize) -> Self {
        let len = size.w * size.h;
        Self {
            kind,
            size,
            aux: vec![0; len],
            heat: vec![0; len],
            scratch: vec![0; size.w.max(size.h)],
        }
    }

    pub fn render(&mut self, frame: u64, out: &mut [u32]) {
        assert_eq!(out.len(), self.size.w * self.size.h);
        match self.kind {
            EffectKind::PaletteCycle => palette_cycle(self.size, frame, out),
            EffectKind::Plasma => plasma(self.size, frame, out),
            EffectKind::CopperBars => copper_bars(self.size, frame, out),
            EffectKind::Starfield => starfield(self.size, frame, out),
            EffectKind::CrtPass => crt_pass(self.size, frame, out),
            EffectKind::TileParallax => tile_parallax(self.size, frame, out),
            EffectKind::Mode7Floor => mode7_floor(self.size, frame, out),
            EffectKind::Afterimage => afterimage(self.size, frame, out, &mut self.aux),
            EffectKind::DitherSpotlight => {
                dither_spotlight(self.size, frame, out, &mut self.scratch)
            }
            EffectKind::WipeTransition => wipe_transition(self.size, frame, out),
            EffectKind::ChunkyDistortion => chunky_distortion(self.size, frame, out),
            EffectKind::FireHaze => fire_haze(self.size, frame, out, &mut self.heat),
            EffectKind::VhsGlitch => vhs_glitch(self.size, frame, out, &mut self.scratch),
        }
        draw_label(self.size, self.kind.name(), out);
    }
}

struct SourceImage {
    w: usize,
    h: usize,
    pixels: Vec<u32>,
}

struct CrtImage {
    w: usize,
    h: usize,
    warped: Vec<u32>,
    normal: Vec<u32>,
    spans: Vec<RowSpan>,
}

#[derive(Clone, Copy)]
struct RowSpan {
    start: usize,
    end: usize,
}

fn rgb(r: u32, g: u32, b: u32) -> u32 {
    ((r & 255) << 16) | ((g & 255) << 8) | (b & 255)
}

fn wave(v: i32) -> u32 {
    let t = (v & 255) as u32;
    if t < 128 { t * 2 } else { (255 - t) * 2 }
}

fn palette(i: u32) -> u32 {
    palette_table()[(i & 255) as usize]
}

fn palette_table() -> &'static [u32; 256] {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0; 256];
        let mut i = 0;
        while i < table.len() {
            table[i] = rgb(wave(i as i32), wave(i as i32 + 85), wave(i as i32 + 170));
            i += 1;
        }
        table
    })
}

fn fire_palette_table() -> &'static [u32; 256] {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0; 256];
        let mut i = 0;
        while i < table.len() {
            let v = i as u32;
            table[i] = if v < 85 {
                rgb(v * 2, 0, 0)
            } else if v < 170 {
                rgb(170 + (v - 85), (v - 85) * 2, 0)
            } else {
                rgb(255, 170 + (v - 170), (v - 170) * 3)
            };
            i += 1;
        }
        table
    })
}

fn avg3_table() -> &'static [u8; 766] {
    static TABLE: OnceLock<[u8; 766]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0; 766];
        let mut i = 0;
        while i < table.len() {
            table[i] = (i / 3) as u8;
            i += 1;
        }
        table
    })
}

fn vhs_source_image() -> &'static SourceImage {
    static IMAGE: OnceLock<SourceImage> = OnceLock::new();
    IMAGE.get_or_init(vhs_fallback_image)
}

fn crt_image_for_size(size: EffectSize) -> Option<&'static CrtImage> {
    macro_rules! cached {
        ($name:ident, $w:literal, $h:literal) => {{
            static $name: OnceLock<CrtImage> = OnceLock::new();
            Some($name.get_or_init(|| build_crt_image(EffectSize { w: $w, h: $h })))
        }};
    }
    match (size.w, size.h) {
        (320, 180) => cached!(CRT_320_180, 320, 180),
        (320, 224) => cached!(CRT_320_224, 320, 224),
        (480, 270) => cached!(CRT_480_270, 480, 270),
        (640, 360) => cached!(CRT_640_360, 640, 360),
        (640, 448) => cached!(CRT_640_448, 640, 448),
        (960, 540) => cached!(CRT_960_540, 960, 540),
        _ => None,
    }
}

fn build_crt_image(size: EffectSize) -> CrtImage {
    let src = vhs_source_image();
    let mut warped = barrel_warp_source_image(src, size.w, size.h);
    let mut spans = Vec::with_capacity(size.h);
    for y in 0..size.h {
        spans.push(visible_span(&warped[y * size.w..(y + 1) * size.w]));
    }
    let normal = chroma_aberrate_image(size.w, size.h, &warped, 1);
    CrtImage {
        w: size.w,
        h: size.h,
        warped: std::mem::take(&mut warped),
        normal,
        spans,
    }
}

fn barrel_warp_source_image(src: &SourceImage, dst_w: usize, dst_h: usize) -> Vec<u32> {
    const BARREL: f32 = 0.18;
    const ZOOM: f32 = 0.84;
    let mut pixels = vec![0; dst_w * dst_h];
    let dst_cx = (dst_w - 1) as f32 * 0.5;
    let dst_cy = (dst_h - 1) as f32 * 0.5;
    let src_cx = (src.w - 1) as f32 * 0.5;
    let src_cy = (src.h - 1) as f32 * 0.5;
    let inv_dst_cx = 1.0 / dst_cx.max(1.0);
    let inv_dst_cy = 1.0 / dst_cy.max(1.0);

    for y in 0..dst_h {
        let ny = (y as f32 - dst_cy) * inv_dst_cy;
        for x in 0..dst_w {
            let nx = (x as f32 - dst_cx) * inv_dst_cx;
            let r2 = nx * nx + ny * ny;
            let warp = ZOOM * (1.0 + BARREL * r2);
            let sx = nx * warp * src_cx + src_cx;
            let sy = ny * warp * src_cy + src_cy;
            pixels[y * dst_w + x] = sample_source_nearest(src, sx, sy);
        }
    }

    pixels
}

fn visible_span(row: &[u32]) -> RowSpan {
    let start = row.iter().position(|&p| p != 0).unwrap_or(0);
    let end = row.iter().rposition(|&p| p != 0).map_or(0, |x| x + 1);
    RowSpan { start, end }
}

fn chroma_aberrate_image(w: usize, h: usize, src: &[u32], aberr: usize) -> Vec<u32> {
    let mut out = vec![0; w * h];
    for y in 0..h {
        let src_row = &src[y * w..(y + 1) * w];
        let out_row = &mut out[y * w..(y + 1) * w];
        for (x, pixel) in out_row.iter_mut().enumerate().take(w) {
            *pixel = chroma_pixel(src_row, x, aberr);
        }
    }
    out
}

fn chroma_pixel(row: &[u32], x: usize, aberr: usize) -> u32 {
    let sx_r = (x + aberr).min(row.len() - 1);
    let sx_b = x.saturating_sub(aberr);
    (row[sx_r] & 0x00ff0000) | (row[x] & 0x0000ff00) | (row[sx_b] & 0x000000ff)
}

fn sample_source_nearest(src: &SourceImage, x: f32, y: f32) -> u32 {
    if x < 0.0 || y < 0.0 || x > (src.w - 1) as f32 || y > (src.h - 1) as f32 {
        return 0;
    }
    let sx = (x + 0.5) as usize;
    let sy = (y + 0.5) as usize;
    src.pixels[sy.min(src.h - 1) * src.w + sx.min(src.w - 1)]
}

fn vhs_fallback_image() -> SourceImage {
    let w = 240;
    let h = 240;
    let mut pixels = vec![0; w * h];
    for y in 0..h {
        for x in 0..w {
            let bars = ((x / 24) ^ (y / 24)) & 3;
            pixels[y * w + x] = match bars {
                0 => rgb(220, 60, 80),
                1 => rgb(60, 200, 150),
                2 => rgb(70, 100, 230),
                _ => rgb(230, 210, 80),
            };
        }
    }
    SourceImage { w, h, pixels }
}

fn palette_cycle(size: EffectSize, frame: u64, out: &mut [u32]) {
    let colors = palette_table();
    let f = (frame as usize * 4) & 255;
    for y in 0..size.h {
        let mut v = (y * 5 + f) & 255;
        let row = &mut out[y * size.w..(y + 1) * size.w];
        for pixel in row.iter_mut().take(size.w) {
            *pixel = colors[v];
            v = (v + 3) & 255;
        }
    }
}

fn plasma(size: EffectSize, frame: u64, out: &mut [u32]) {
    let f = frame as i32;
    let colors = palette_table();
    for y in 0..size.h {
        let yi = y as i32;
        let y_wave = wave(yi * 5 + f * 2);
        let row = &mut out[y * size.w..(y + 1) * size.w];
        for (x, pixel) in row.iter_mut().enumerate().take(size.w) {
            let xi = x as i32;
            let v = wave(xi * 4 + f * 3)
                + y_wave
                + wave((xi + yi) * 3 + f * 4)
                + wave((xi - yi) * 4 + f);
            *pixel = colors[((v / 4 + f as u32) & 255) as usize];
        }
    }
}

fn copper_bars(size: EffectSize, frame: u64, out: &mut [u32]) {
    let f = frame as i32;
    for y in 0..size.h {
        let yv = y as i32;
        let r = wave(yv * 6 + f * 5);
        let g = wave(yv * 4 + f * 3 + 80);
        let b = wave(yv * 7 - f * 2 + 160);
        let shade = 96 + wave(yv * 11 + f * 6) / 2;
        let c = rgb(r * shade / 255, g * shade / 255, b * shade / 255);
        out[y * size.w..(y + 1) * size.w].fill(c);
    }
}

fn starfield(size: EffectSize, frame: u64, out: &mut [u32]) {
    out.fill(rgb(2, 3, 14));
    for i in 0..900 {
        let seed = i * 977 + 13;
        let z = ((seed * 37 + frame as usize * 3) % 256).max(1);
        let sx = ((seed * 29) % 512) as isize - 256;
        let sy = ((seed * 71) % 320) as isize - 160;
        let x = size.w as isize / 2 + sx * 64 / z as isize;
        let y = size.h as isize / 2 + sy * 64 / z as isize;
        if x >= 0 && y >= 0 && x < size.w as isize && y < size.h as isize {
            let v = 255 - z as u32;
            out[y as usize * size.w + x as usize] = rgb(v, v, 255);
        }
    }
}

fn crt_pass(size: EffectSize, frame: u64, out: &mut [u32]) {
    let f = frame as usize;
    for y in 0..size.h {
        for x in 0..size.w {
            let bars = ((x + f * 2) / 16 + y / 12) & 7;
            let scan = if y & 1 == 0 { 170 } else { 255 };
            let mask = match x % 3 {
                0 => rgb(255, 120, 120),
                1 => rgb(120, 255, 120),
                _ => rgb(120, 120, 255),
            };
            let base = if bars < 4 { 210 } else { 70 };
            out[y * size.w + x] = mul_color(mask, base * scan / 255);
        }
    }
}

fn tile_parallax(size: EffectSize, frame: u64, out: &mut [u32]) {
    for y in 0..size.h {
        for x in 0..size.w {
            let l0 = (((x + frame as usize) / 24) ^ ((y + frame as usize / 2) / 24)) & 1;
            let l1 = (((x + frame as usize * 2) / 48) + ((y + frame as usize) / 32)) & 3;
            let c = match (l0, l1) {
                (0, 0) => rgb(16, 35, 68),
                (1, 0) => rgb(27, 75, 102),
                (0, 1) => rgb(80, 42, 98),
                (1, 1) => rgb(110, 70, 42),
                (_, 2) => rgb(16, 92, 74),
                _ => rgb(112, 32, 62),
            };
            out[y * size.w + x] = c;
        }
    }
}

fn mode7_floor(size: EffectSize, frame: u64, out: &mut [u32]) {
    let f = frame as i32;
    let horizon = size.h / 3;
    let sky_den = horizon.max(1) as u32;
    for y in 0..size.h {
        let row = &mut out[y * size.w..(y + 1) * size.w];
        if y < horizon {
            let v = 20 + (y as u32 * 80 / sky_den);
            row.fill(rgb(4, 8 + v / 2, 28 + v));
            continue;
        }
        let dy = (y - horizon + 1) as i32;
        let depth = 900 / dy;
        let wy = depth * 12 + f * 5;
        let mut wx_fixed = (-(size.w as i32 / 2) * depth) + f * 64;
        for dst in row.iter_mut() {
            let wx = wx_fixed >> 5;
            let grid = ((wx >> 4) ^ (wy >> 4)) & 1;
            *dst = if grid == 0 {
                rgb(24, 94, 100)
            } else {
                rgb(112, 44, 94)
            };
            wx_fixed += depth;
        }
    }
}

fn afterimage(size: EffectSize, frame: u64, out: &mut [u32], aux: &mut [u32]) {
    for p in aux.iter_mut() {
        *p = mul_color(*p, 220);
    }
    let cx = (size.w / 2) as i32 + ((wave(frame as i32 * 5) as i32 - 128) * size.w as i32 / 330);
    let cy =
        (size.h / 2) as i32 + ((wave(frame as i32 * 7 + 80) as i32 - 128) * size.h as i32 / 330);
    let r = size.h.max(32) as i32 / 8;
    for y in (cy - r).max(0)..(cy + r).min(size.h as i32) {
        for x in (cx - r).max(0)..(cx + r).min(size.w as i32) {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r * r {
                aux[y as usize * size.w + x as usize] = rgb(255, 210, 80);
            }
        }
    }
    out.copy_from_slice(aux);
}

fn dither_spotlight(size: EffectSize, frame: u64, out: &mut [u32], scratch: &mut [i32]) {
    const BAYER: [[i32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    const BRIGHT: u32 = (210 << 16) | (168 << 8) | 105;
    const DIM: u32 = (55 << 16) | (44 << 8) | 27;
    let cx = (size.w / 2) as i32 + ((wave(frame as i32 * 3) as i32 - 128) * size.w as i32 / 360);
    let cy =
        (size.h / 2) as i32 + ((wave(frame as i32 * 4 + 70) as i32 - 128) * size.h as i32 / 360);
    let max_d = (size.w.min(size.h) as i32 / 2).max(1);
    for (x, dst) in scratch[..size.w].iter_mut().enumerate() {
        let dx = x as i32 - cx;
        *dst = dx * dx;
    }
    for y in 0..size.h {
        let dy = y as i32 - cy;
        let dy2 = dy * dy;
        let bayer = BAYER[y & 3];
        let row = &mut out[y * size.w..(y + 1) * size.w];
        for x in 0..size.w {
            let d = ((scratch[x] + dy2) / max_d).min(255);
            let threshold = bayer[x & 3] * 12 + 80;
            row[x] = if d < threshold { BRIGHT } else { DIM };
        }
    }
}

fn wipe_transition(size: EffectSize, frame: u64, out: &mut [u32]) {
    let sweep = (frame as usize * 6) % (size.w + size.h);
    for y in 0..size.h {
        for x in 0..size.w {
            let a = ((x / 12) ^ (y / 12)) & 1;
            let open = x + y < sweep;
            out[y * size.w + x] = match (open, a) {
                (true, 0) => rgb(220, 60, 80),
                (true, _) => rgb(255, 180, 80),
                (false, 0) => rgb(8, 12, 28),
                (false, _) => rgb(18, 28, 56),
            };
        }
    }
}

fn chunky_distortion(size: EffectSize, frame: u64, out: &mut [u32]) {
    let f = frame as i32;
    for y in 0..size.h {
        let shift = wave(y as i32 * 6 + f * 5) as isize / 8 - 16;
        for x in 0..size.w {
            let sx = (x as isize + shift).rem_euclid(size.w as isize) as usize;
            let block = ((sx / 16) + (y / 16) + frame as usize / 8) & 7;
            out[y * size.w + x] = palette((block * 32 + sx / 3 + y / 2) as u32);
        }
    }
}

fn fire_haze(size: EffectSize, frame: u64, out: &mut [u32], heat: &mut [u8]) {
    let w = size.w;
    let h = size.h;
    let avg3 = avg3_table();
    for x in 0..w {
        heat[(h - 1) * w + x] = if ((x + frame as usize * 3) / 9) & 1 == 0 {
            255
        } else {
            180
        };
    }
    for y in (1..h).rev() {
        let row = y * w;
        let prev = (y - 1) * w;
        for x in 0..w {
            let below = heat[row + x] as usize;
            let left = heat[row + if x == 0 { w - 1 } else { x - 1 }] as usize;
            let right = heat[row + if x + 1 == w { 0 } else { x + 1 }] as usize;
            let decay = ((x + y + frame as usize) & 3) as u16;
            heat[prev + x] = (avg3[below + left + right] as u16).saturating_sub(decay) as u8;
        }
    }
    let colors = fire_palette_table();
    for (dst, &v) in out.iter_mut().zip(heat.iter()) {
        *dst = colors[v as usize];
    }
}

fn vhs_glitch(size: EffectSize, frame: u64, out: &mut [u32], _x_map: &mut [i32]) {
    if let Some(crt) = crt_image_for_size(size) {
        render_crt_image(crt, frame as usize, out);
    } else {
        let crt = build_crt_image(size);
        render_crt_image(&crt, frame as usize, out);
    }
}

fn render_crt_image(crt: &CrtImage, frame: usize, out: &mut [u32]) {
    debug_assert_eq!(out.len(), crt.w * crt.h);
    let glitch = CrtGlitchFrame::new(crt.h, frame);
    for y in 0..crt.h {
        let row_range = y * crt.w..(y + 1) * crt.w;
        let dst_row = &mut out[row_range.clone()];
        let span = crt.spans[y];
        if span.start > 0 {
            dst_row[..span.start].fill(0);
        }
        if span.end < crt.w {
            dst_row[span.end..].fill(0);
        }
        if span.end <= span.start {
            continue;
        }

        let scan = crt_scanline_amount(y, frame);
        if let Some(glitch) = glitch.for_row(y) {
            render_crt_glitch_row(
                &crt.warped[row_range],
                &mut dst_row[span.start..span.end],
                span.start,
                scan,
                glitch,
            );
        } else {
            render_crt_normal_row(
                &crt.normal[row_range],
                &mut dst_row[span.start..span.end],
                span.start,
                scan,
            );
        }
    }
}

fn render_crt_normal_row(src_row: &[u32], dst: &mut [u32], x0: usize, scan: u32) {
    for (i, dst) in dst.iter_mut().enumerate() {
        *dst = scale_color_256(src_row[x0 + i], scan);
    }
}

fn render_crt_glitch_row(
    src_row: &[u32],
    dst: &mut [u32],
    x0: usize,
    scan: u32,
    glitch: CrtGlitch,
) {
    let w = src_row.len();
    let scan = (scan * glitch.amount / 256).min(255);
    let mut sx = wrap_shifted_x(x0, glitch.shift_px, w);
    for dst in dst {
        let p = chroma_pixel(src_row, sx, 3);
        *dst = scale_color_256(p, scan) ^ glitch.noise;
        sx += 1;
        if sx == w {
            sx = 0;
        }
    }
}

fn wrap_shifted_x(x: usize, shift: i32, w: usize) -> usize {
    if shift >= 0 {
        let mut sx = x + shift as usize;
        if sx >= w {
            sx -= w;
        }
        sx
    } else {
        let shift = (-shift) as usize % w;
        if x >= shift { x - shift } else { w + x - shift }
    }
}

struct CrtGlitch {
    shift_px: i32,
    amount: u32,
    noise: u32,
}

#[derive(Clone, Copy)]
struct CrtGlitchFrame {
    y0: usize,
    y1: usize,
    center: isize,
    active: bool,
}

impl CrtGlitchFrame {
    fn new(h: usize, frame: usize) -> Self {
        const PERIOD: usize = 360;
        const DURATION: usize = 88;
        let phase = (frame + 72) % PERIOD;
        if phase >= DURATION {
            return Self {
                y0: 0,
                y1: 0,
                center: 0,
                active: false,
            };
        }

        let span = h as isize + 48;
        let center = h as isize + 24 - (phase as isize * span / DURATION as isize);
        let y0 = (center - 7).max(0) as usize;
        let y1 = (center + 8).clamp(0, h as isize) as usize;
        Self {
            y0,
            y1,
            center,
            active: y1 > y0,
        }
    }

    fn for_row(self, y: usize) -> Option<CrtGlitch> {
        if !self.active || y < self.y0 || y >= self.y1 {
            return None;
        }
        let distance = (y as isize - self.center).unsigned_abs();
        let seed = hash32(
            (y as u32).wrapping_mul(0x45d9f3b)
                ^ (self.center as i32 as u32).wrapping_mul(0x1f12bb5),
        );
        let strength = (8 - distance) as u32;
        let direction = if seed & 1 == 0 { -1 } else { 1 };
        let shift_px = direction * (3 + ((seed >> 8) & 7) as i32) * strength as i32 / 2;
        let amount = if distance <= 2 {
            318
        } else {
            190 + strength * 10
        };
        let noise = ((seed >> 24) & 3) * 0x00010101;
        Some(CrtGlitch {
            shift_px,
            amount,
            noise,
        })
    }
}

fn crt_scanline_amount(y: usize, frame: usize) -> u32 {
    let seed =
        hash32((y as u32).wrapping_mul(0x45d9f3b) ^ ((frame / 4) as u32).wrapping_mul(0x9e37));
    let roll = y
        .wrapping_add(frame * 2)
        .wrapping_add((seed as usize) & 7)
        .is_multiple_of(41);
    if y & 1 == 0 {
        138 + (seed & 23)
    } else if roll {
        220 + ((seed >> 5) & 11)
    } else {
        242 + ((seed >> 6) & 14)
    }
}

fn hash32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

fn scale_color_256(c: u32, amount: u32) -> u32 {
    let rb = (((c & 0x00ff00ff) * amount) >> 8) & 0x00ff00ff;
    let g = (((c & 0x0000ff00) * amount) >> 8) & 0x0000ff00;
    rb | g
}

fn mul_color(c: u32, amount: u32) -> u32 {
    let r = ((c >> 16) & 255) * amount / 255;
    let g = ((c >> 8) & 255) * amount / 255;
    let b = (c & 255) * amount / 255;
    rgb(r, g, b)
}

fn draw_label(size: EffectSize, text: &str, out: &mut [u32]) {
    const CHAR_W: usize = 5;
    const CHAR_H: usize = 7;
    const GAP: usize = 1;
    const PAD: usize = 3;
    let chars: Vec<char> = text.chars().collect();
    let label_w = chars.len() * (CHAR_W + GAP) + PAD * 2 - GAP;
    let label_h = CHAR_H + PAD * 2;
    if label_w >= size.w || label_h >= size.h {
        return;
    }
    let x0 = size.w - label_w - 4;
    let y0 = 4;
    for y in y0..y0 + label_h {
        for x in x0..x0 + label_w {
            out[y * size.w + x] = rgb(0, 0, 0);
        }
    }
    for (i, ch) in chars.iter().enumerate() {
        draw_char(size, x0 + PAD + i * (CHAR_W + GAP), y0 + PAD, *ch, out);
    }
}

fn draw_char(size: EffectSize, x0: usize, y0: usize, ch: char, out: &mut [u32]) {
    let glyph = glyph5x7(ch);
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..5 {
            if (bits >> (4 - col)) & 1 != 0 {
                let x = x0 + col;
                let y = y0 + row;
                if x < size.w && y < size.h {
                    out[y * size.w + x] = rgb(255, 245, 170);
                }
            }
        }
    }
}

fn glyph5x7(ch: char) -> [u8; 7] {
    match ch {
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        'a' => [0, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111, 0],
        'b' => [0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b11110, 0],
        'c' => [0, 0b01111, 0b10000, 0b10000, 0b10000, 0b01111, 0],
        'd' => [0b00001, 0b00001, 0b01111, 0b10001, 0b10001, 0b01111, 0],
        'e' => [0, 0b01110, 0b10001, 0b11111, 0b10000, 0b01111, 0],
        'f' => [0b00110, 0b01000, 0b11100, 0b01000, 0b01000, 0b01000, 0],
        'g' => [0, 0b01111, 0b10001, 0b01111, 0b00001, 0b11110, 0],
        'h' => [0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0],
        'i' => [0b00100, 0, 0b01100, 0b00100, 0b00100, 0b01110, 0],
        'k' => [0b10000, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0],
        'l' => [0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110, 0],
        'm' => [0, 0b11010, 0b10101, 0b10101, 0b10101, 0b10101, 0],
        'n' => [0, 0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0],
        'o' => [0, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110, 0],
        'p' => [0, 0b11110, 0b10001, 0b11110, 0b10000, 0b10000, 0],
        'r' => [0, 0b10110, 0b11001, 0b10000, 0b10000, 0b10000, 0],
        's' => [0, 0b01111, 0b10000, 0b01110, 0b00001, 0b11110, 0],
        't' => [0b01000, 0b11100, 0b01000, 0b01000, 0b01001, 0b00110, 0],
        'w' => [0, 0b10001, 0b10001, 0b10101, 0b10101, 0b01010, 0],
        'y' => [0, 0b10001, 0b10001, 0b01111, 0b00001, 0b11110, 0],
        _ => [0, 0, 0, 0, 0, 0, 0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_effect_names() {
        for &name in EFFECT_NAMES {
            assert_eq!(EffectKind::parse(name).map(EffectKind::name), Some(name));
        }
        assert!(EffectKind::parse("bogus").is_none());
    }

    #[test]
    fn validates_1080p_integer_scales() {
        assert_eq!(integer_scale_to_1080p(320, 180), Some(6));
        assert_eq!(integer_scale_to_1080p(480, 270), Some(4));
        assert_eq!(integer_scale_to_1080p(640, 360), Some(3));
        assert_eq!(integer_scale_to_1080p(960, 540), Some(2));
        assert_eq!(integer_scale_to_1080p(512, 288), None);
        assert_eq!(integer_scale_to_rect(320, 224, 640, 448), Some(2));
        assert_eq!(EffectSize { w: 320, h: 180 }.scale_to_half_1080p(), Some(3));
        assert_eq!(EffectSize { w: 480, h: 270 }.scale_to_half_1080p(), Some(2));
        assert_eq!(EffectSize { w: 640, h: 360 }.scale_to_half_1080p(), None);
        assert_eq!(EffectSize { w: 640, h: 448 }.scale_to_half_1080p(), None);
        assert_eq!(EffectSize { w: 960, h: 540 }.scale_to_half_1080p(), Some(1));
    }

    #[test]
    fn vhs_glitch_caches_all_supported_effect_sizes() {
        for &(w, h) in EFFECT_SIZES {
            assert!(
                crt_image_for_size(EffectSize { w, h }).is_some(),
                "vhs_glitch should cache {w}x{h}"
            );
        }
    }

    #[test]
    fn effects_are_deterministic_and_non_black() {
        let size = EffectSize { w: 32, h: 18 };
        for &kind in EffectKind::all() {
            let mut a = vec![0; size.w * size.h];
            let mut b = vec![0; size.w * size.h];
            EffectState::new(kind, size).render(7, &mut a);
            EffectState::new(kind, size).render(7, &mut b);
            assert_eq!(a, b, "{kind:?} should be deterministic");
            assert!(a.iter().any(|&p| p != 0), "{kind:?} should draw pixels");
        }
    }
}
