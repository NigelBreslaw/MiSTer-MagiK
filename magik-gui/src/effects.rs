//! Host-testable retro framebuffer effects used by the device benchmark.

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
];

pub const EFFECT_SIZES: &[(usize, usize)] = &[(320, 180), (480, 270), (640, 360), (960, 540)];

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
}

impl EffectState {
    pub fn new(kind: EffectKind, size: EffectSize) -> Self {
        let len = size.w * size.h;
        Self {
            kind,
            size,
            aux: vec![0; len],
            heat: vec![0; len],
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
            EffectKind::DitherSpotlight => dither_spotlight(self.size, frame, out),
            EffectKind::WipeTransition => wipe_transition(self.size, frame, out),
            EffectKind::ChunkyDistortion => chunky_distortion(self.size, frame, out),
            EffectKind::FireHaze => fire_haze(self.size, frame, out, &mut self.heat),
        }
        draw_label(self.size, self.kind.name(), out);
    }
}

fn rgb(r: u32, g: u32, b: u32) -> u32 {
    ((r & 255) << 16) | ((g & 255) << 8) | (b & 255)
}

fn wave(v: i32) -> u32 {
    let t = (v & 255) as u32;
    if t < 128 {
        t * 2
    } else {
        (255 - t) * 2
    }
}

fn palette(i: u32) -> u32 {
    let i = i & 255;
    rgb(wave(i as i32), wave(i as i32 + 85), wave(i as i32 + 170))
}

fn palette_cycle(size: EffectSize, frame: u64, out: &mut [u32]) {
    for y in 0..size.h {
        for x in 0..size.w {
            let v = ((x * 3 + y * 5) as u64 + frame * 4) as u32;
            out[y * size.w + x] = palette(v);
        }
    }
}

fn plasma(size: EffectSize, frame: u64, out: &mut [u32]) {
    let f = frame as i32;
    for y in 0..size.h {
        for x in 0..size.w {
            let xi = x as i32;
            let yi = y as i32;
            let v = wave(xi * 4 + f * 3)
                + wave(yi * 5 + f * 2)
                + wave((xi + yi) * 3 + f * 4)
                + wave((xi - yi) * 4 + f);
            out[y * size.w + x] = palette(v / 4 + f as u32);
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
    for y in 0..size.h {
        let horizon = size.h / 3;
        for x in 0..size.w {
            if y < horizon {
                let v = 20 + (y as u32 * 80 / horizon.max(1) as u32);
                out[y * size.w + x] = rgb(4, 8 + v / 2, 28 + v);
                continue;
            }
            let dy = (y - horizon + 1) as i32;
            let depth = 900 / dy;
            let wx = (x as i32 - size.w as i32 / 2) * depth / 32 + f * 2;
            let wy = depth * 12 + f * 5;
            let grid = ((wx >> 4) ^ (wy >> 4)) & 1;
            out[y * size.w + x] = if grid == 0 {
                rgb(24, 94, 100)
            } else {
                rgb(112, 44, 94)
            };
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

fn dither_spotlight(size: EffectSize, frame: u64, out: &mut [u32]) {
    const BAYER: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let cx = (size.w / 2) as i32 + ((wave(frame as i32 * 3) as i32 - 128) * size.w as i32 / 360);
    let cy =
        (size.h / 2) as i32 + ((wave(frame as i32 * 4 + 70) as i32 - 128) * size.h as i32 / 360);
    let max_d = (size.w.min(size.h) as i32 / 2).max(1);
    for y in 0..size.h {
        for x in 0..size.w {
            let dx = x as i32 - cx;
            let dy = y as i32 - cy;
            let d = ((dx * dx + dy * dy) / max_d).min(255) as u32;
            let threshold = BAYER[y & 3][x & 3] * 12;
            let bright = if d < threshold + 80 { 210 } else { 55 };
            out[y * size.w + x] = rgb(bright, bright * 4 / 5, bright / 2);
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
    for x in 0..w {
        heat[(h - 1) * w + x] = if ((x + frame as usize * 3) / 9) & 1 == 0 {
            255
        } else {
            180
        };
    }
    for y in (1..h).rev() {
        for x in 0..w {
            let below = heat[y * w + x] as u16;
            let left = heat[y * w + (x + w - 1) % w] as u16;
            let right = heat[y * w + (x + 1) % w] as u16;
            let decay = ((x + y + frame as usize) & 3) as u16;
            heat[(y - 1) * w + x] = ((below + left + right) / 3).saturating_sub(decay) as u8;
        }
    }
    for (dst, &v) in out.iter_mut().zip(heat.iter()) {
        let v = v as u32;
        *dst = if v < 85 {
            rgb(v * 2, 0, 0)
        } else if v < 170 {
            rgb(170 + (v - 85), (v - 85) * 2, 0)
        } else {
            rgb(255, 170 + (v - 170), (v - 170) * 3)
        };
    }
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
        assert_eq!(EffectSize { w: 320, h: 180 }.scale_to_half_1080p(), Some(3));
        assert_eq!(EffectSize { w: 480, h: 270 }.scale_to_half_1080p(), Some(2));
        assert_eq!(EffectSize { w: 640, h: 360 }.scale_to_half_1080p(), None);
        assert_eq!(EffectSize { w: 960, h: 540 }.scale_to_half_1080p(), Some(1));
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
