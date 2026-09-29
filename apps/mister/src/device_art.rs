// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Generic device art for the game list and system page: a TV for consoles, a
//! monitor for computers and a handheld for handhelds. Only Arcade uses the
//! cabinet render.
//!
//! Each image has the cabinet's exact geometry: 483x519 with a 320x320 screen
//! opening at (82, 61) that is pure black, so the preview compositor and the
//! Slint layouts treat every device the same. The art is drawn once, in
//! floating point, and quantised to RGB565 with ordered dithering. Everything
//! is deterministic and nothing is allocated after the first call.

use std::sync::OnceLock;

pub const DEVICE_WIDTH: usize = 483;
pub const DEVICE_HEIGHT: usize = 519;
/// Screen opening inside the art, pure black.
pub const SCREEN_X: usize = 82;
pub const SCREEN_Y: usize = 61;
pub const SCREEN_SIZE: usize = 320;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceKind {
    Tv,
    Monitor,
    Handheld,
}

impl DeviceKind {
    const fn index(self) -> usize {
        match self {
            Self::Tv => 0,
            Self::Monitor => 1,
            Self::Handheld => 2,
        }
    }
}

/// The device's little-endian RGB565 pixels, ready for a Slint image.
pub fn device_rgb565(kind: DeviceKind) -> &'static [u16] {
    static IMAGES: [OnceLock<Vec<u16>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    IMAGES[kind.index()].get_or_init(|| render(kind))
}

/// Rows of the art that are visible on the HDMI pages (the rest is clipped).
pub const VISIBLE_TOP: usize = 42;
pub const VISIBLE_HEIGHT: usize = 423;

/// The visible part of the device box-filtered to `width` x `height`, for
/// the CRT pages. Rasters with non-square pixels pass a squashed height.
/// Averages in 8-bit sRGB and re-quantises with ordered dithering.
pub fn hero_rgb565(kind: DeviceKind, width: usize, height: usize) -> Vec<u16> {
    const BAYER: [[f32; 4]; 4] = [
        [0.0, 8.0, 2.0, 10.0],
        [12.0, 4.0, 14.0, 6.0],
        [3.0, 11.0, 1.0, 9.0],
        [15.0, 7.0, 13.0, 5.0],
    ];
    let source = device_rgb565(kind);
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        let (y0, y1) = (
            VISIBLE_TOP + y * VISIBLE_HEIGHT / height,
            VISIBLE_TOP + ((y + 1) * VISIBLE_HEIGHT).div_ceil(height),
        );
        for x in 0..width {
            let (x0, x1) = (
                x * DEVICE_WIDTH / width,
                ((x + 1) * DEVICE_WIDTH).div_ceil(width),
            );
            let mut sum = [0.0_f32; 3];
            let mut count = 0.0_f32;
            for sy in y0..y1.min(DEVICE_HEIGHT) {
                for sx in x0..x1.min(DEVICE_WIDTH) {
                    let p = source[sy * DEVICE_WIDTH + sx];
                    let (r, g, b) = (p >> 11, (p >> 5) & 63, p & 31);
                    sum[0] += f32::from((r << 3) | (r >> 2));
                    sum[1] += f32::from((g << 2) | (g >> 4));
                    sum[2] += f32::from((b << 3) | (b >> 2));
                    count += 1.0;
                }
            }
            let threshold = (BAYER[y % 4][x % 4] + 0.5) / 16.0;
            let quantise = |value: f32, levels: f32| {
                ((value / count.max(1.0)) / 255.0 * levels + threshold)
                    .floor()
                    .min(levels) as u16
            };
            out.push(
                (quantise(sum[0], 31.0) << 11)
                    | (quantise(sum[1], 63.0) << 5)
                    | quantise(sum[2], 31.0),
            );
        }
    }
    out
}

type Rgb = [f32; 3];

fn hex(value: u32) -> Rgb {
    [
        ((value >> 16) & 255) as f32,
        ((value >> 8) & 255) as f32,
        (value & 255) as f32,
    ]
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

struct Canvas {
    pixels: Vec<Rgb>,
}

impl Canvas {
    fn new() -> Self {
        Self {
            pixels: vec![[0.0; 3]; DEVICE_WIDTH * DEVICE_HEIGHT],
        }
    }

    /// Blend `colour` over a pixel with `coverage` in 0..=1.
    fn blend(&mut self, x: usize, y: usize, colour: Rgb, coverage: f32) {
        if x >= DEVICE_WIDTH || y >= DEVICE_HEIGHT || coverage <= 0.0 {
            return;
        }
        let pixel = &mut self.pixels[y * DEVICE_WIDTH + x];
        *pixel = mix(*pixel, colour, coverage);
    }

    /// Antialiased rounded rectangle filled by `shade(x, y)`.
    fn rounded(
        &mut self,
        (x0, y0, x1, y1): (f32, f32, f32, f32),
        radius: f32,
        shade: impl Fn(f32, f32) -> Rgb,
    ) {
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let (hw, hh) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
        let left = (x0.floor() as usize).saturating_sub(1);
        let top = (y0.floor() as usize).saturating_sub(1);
        for y in top..=(y1.ceil() as usize + 1).min(DEVICE_HEIGHT - 1) {
            for x in left..=(x1.ceil() as usize + 1).min(DEVICE_WIDTH - 1) {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let (dx, dy) = (
                    (px - cx).abs() - (hw - radius),
                    (py - cy).abs() - (hh - radius),
                );
                let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt()
                    + dx.max(dy).min(0.0)
                    - radius;
                self.blend(x, y, shade(px, py), (0.5 - outside).clamp(0.0, 1.0));
            }
        }
    }

    fn circle(&mut self, cx: f32, cy: f32, radius: f32, shade: impl Fn(f32, f32) -> Rgb) {
        self.rounded(
            (cx - radius, cy - radius, cx + radius, cy + radius),
            radius,
            shade,
        );
    }

    fn solid(&mut self, rect: (f32, f32, f32, f32), radius: f32, colour: Rgb) {
        self.rounded(rect, radius, |_, _| colour);
    }

    /// A vertical gradient between two colours across `[top, bottom]`.
    fn vertical(
        top_colour: Rgb,
        bottom_colour: Rgb,
        top: f32,
        bottom: f32,
    ) -> impl Fn(f32, f32) -> Rgb {
        move |_, y| mix(top_colour, bottom_colour, (y - top) / (bottom - top))
    }

    /// A rounded shell lit from above: a gradient with a highlighted top edge
    /// and a shadowed bottom edge, both clipped to the rounded shape.
    fn shell(
        &mut self,
        rect: (f32, f32, f32, f32),
        radius: f32,
        (top_colour, bottom_colour): (Rgb, Rgb),
        gradient_bottom: f32,
        (light, dark): (Rgb, Rgb),
    ) {
        let (y0, y1) = (rect.1, rect.3);
        self.rounded(rect, radius, move |_, y| {
            let base = mix(top_colour, bottom_colour, (y - y0) / (gradient_bottom - y0));
            if y < y0 + 3.0 {
                mix(base, light, 0.85 - (y - y0) * 0.2)
            } else if y > y1 - 3.0 {
                mix(base, dark, 0.7)
            } else {
                base
            }
        });
    }

    fn into_rgb565(self) -> Vec<u16> {
        const BAYER: [[f32; 4]; 4] = [
            [0.0, 8.0, 2.0, 10.0],
            [12.0, 4.0, 14.0, 6.0],
            [3.0, 11.0, 1.0, 9.0],
            [15.0, 7.0, 13.0, 5.0],
        ];
        let mut out = Vec::with_capacity(self.pixels.len());
        for (index, pixel) in self.pixels.iter().enumerate() {
            let (x, y) = (index % DEVICE_WIDTH, index / DEVICE_WIDTH);
            let in_screen = (SCREEN_X..SCREEN_X + SCREEN_SIZE).contains(&x)
                && (SCREEN_Y..SCREEN_Y + SCREEN_SIZE).contains(&y);
            if in_screen {
                out.push(0);
                continue;
            }
            let threshold = (BAYER[y % 4][x % 4] + 0.5) / 16.0;
            let quantise = |value: f32, levels: f32| {
                let scaled = value.clamp(0.0, 255.0) / 255.0 * levels;
                (scaled + threshold).floor().min(levels) as u16
            };
            out.push(
                (quantise(pixel[0], 31.0) << 11)
                    | (quantise(pixel[1], 63.0) << 5)
                    | quantise(pixel[2], 31.0),
            );
        }
        out
    }
}

fn render(kind: DeviceKind) -> Vec<u16> {
    let mut canvas = Canvas::new();
    match kind {
        DeviceKind::Tv => draw_tv(&mut canvas),
        DeviceKind::Monitor => draw_monitor(&mut canvas),
        DeviceKind::Handheld => draw_handheld(&mut canvas),
    }
    canvas.into_rgb565()
}

const SCREEN: (f32, f32, f32, f32) = (
    SCREEN_X as f32,
    SCREEN_Y as f32,
    (SCREEN_X + SCREEN_SIZE) as f32,
    (SCREEN_Y + SCREEN_SIZE) as f32,
);

/// A dark recess and an accent hairline around the black opening.
fn glass_surround(canvas: &mut Canvas, accent: Rgb, recess: f32) {
    let (x0, y0, x1, y1) = SCREEN;
    canvas.solid(
        (x0 - recess, y0 - recess, x1 + recess, y1 + recess),
        10.0,
        hex(0x07080a),
    );
    canvas.solid(
        (
            x0 - recess + 2.0,
            y0 - recess + 2.0,
            x1 + recess - 2.0,
            y1 + recess - 2.0,
        ),
        8.0,
        mix(hex(0x07080a), accent, 0.22),
    );
    canvas.solid((x0 - 3.0, y0 - 3.0, x1 + 3.0, y1 + 3.0), 6.0, hex(0x020203));
}

/// A console TV: a chunky dark set with a knob column, speaker grille and feet.
fn draw_tv(canvas: &mut Canvas) {
    let accent = hex(0x5a71e7);
    let body = (8.0, 46.0, 475.0, 478.0);
    canvas.solid(
        (body.0 - 2.0, body.1 - 2.0, body.2 + 2.0, body.3 + 2.0),
        34.0,
        hex(0x0b0c10),
    );
    canvas.shell(
        body,
        32.0,
        (hex(0x33363d), hex(0x1a1c21)),
        body.3,
        (hex(0x565a63), hex(0x101115)),
    );
    // Screen bezel and glass surround.
    canvas.solid((60.0, 52.0, 424.0, 402.0), 26.0, hex(0x121317));
    canvas.solid((62.0, 54.0, 422.0, 400.0), 24.0, hex(0x1d1f25));
    glass_surround(canvas, accent, 12.0);
    // Right: two knobs with a pointer, then a speaker grille.
    for (index, cy) in [126.0_f32, 200.0].into_iter().enumerate() {
        let cx = 449.0;
        canvas.circle(
            cx,
            cy,
            15.0,
            Canvas::vertical(hex(0x666a73), hex(0x25272c), cy - 15.0, cy + 15.0),
        );
        canvas.circle(
            cx,
            cy,
            11.0,
            Canvas::vertical(hex(0x40434a), hex(0x2b2d33), cy - 11.0, cy + 11.0),
        );
        let angle = if index == 0 { -0.6_f32 } else { 0.9 };
        for step in 4..11 {
            let (px, py) = (
                cx + angle.sin() * step as f32,
                cy - angle.cos() * step as f32,
            );
            canvas.circle(px, py, 1.3, |_, _| hex(0xd9dbe0));
        }
    }
    for row in 0..9 {
        let y = 246.0 + row as f32 * 14.0;
        canvas.solid((434.0, y, 466.0, y + 5.0), 2.5, hex(0x0d0e11));
    }
    // Left: a matching grille, and a power light on the lower bezel.
    for row in 0..9 {
        let y = 246.0 + row as f32 * 14.0;
        canvas.solid((17.0, y, 49.0, y + 5.0), 2.5, hex(0x0d0e11));
    }
    canvas.circle(112.0, 422.0, 4.5, |_, _| mix(hex(0x0a0b0e), accent, 0.95));
    canvas.circle(112.0, 422.0, 8.0, |_, _| mix(hex(0x1d1f25), accent, 0.18));
    canvas.circle(112.0, 422.0, 4.5, |_, _| accent);
    // Badge plate on the lower bezel, and feet.
    canvas.solid((190.0, 412.0, 293.0, 432.0), 5.0, hex(0x0d0e11));
    canvas.solid((192.0, 414.0, 291.0, 430.0), 4.0, hex(0x2b2e35));
    canvas.solid((66.0, 470.0, 138.0, 519.0), 12.0, hex(0x0b0c10));
    canvas.solid((345.0, 470.0, 417.0, 519.0), 12.0, hex(0x0b0c10));
}

/// A retro beige monitor on a swivel stand, with the front of a keyboard.
fn draw_monitor(canvas: &mut Canvas) {
    let accent = hex(0xe6c23a);
    let beige = (hex(0xdcd3ba), hex(0xb3aa92));
    let shell = (30.0, 46.0, 453.0, 424.0);
    canvas.solid(
        (shell.0 - 2.0, shell.1 - 2.0, shell.2 + 2.0, shell.3 + 2.0),
        26.0,
        hex(0x14120d),
    );
    canvas.shell(
        shell,
        24.0,
        (beige.0, beige.1),
        shell.3,
        (hex(0xf6f0dc), hex(0x8c846f)),
    );
    // Recessed dark bezel, then the accent hairline and the glass.
    canvas.solid((60.0, 50.0, 424.0, 402.0), 18.0, hex(0x8f8770));
    canvas.solid((63.0, 53.0, 421.0, 399.0), 16.0, hex(0x24231f));
    glass_surround(canvas, accent, 10.0);
    // Control strip: a badge, power light and two buttons.
    canvas.solid(
        (88.0, 404.0, 152.0, 416.0),
        4.0,
        mix(beige.1, hex(0x5c5645), 0.55),
    );
    canvas.circle(383.0, 410.0, 4.0, |_, _| accent);
    canvas.solid(
        (398.0, 404.0, 418.0, 416.0),
        4.0,
        mix(beige.1, hex(0x5c5645), 0.4),
    );
    canvas.solid(
        (424.0, 404.0, 444.0, 416.0),
        4.0,
        mix(beige.1, hex(0x5c5645), 0.4),
    );
    // Neck, base and the front of a keyboard.
    canvas.rounded(
        (194.0, 424.0, 289.0, 452.0),
        6.0,
        Canvas::vertical(hex(0xa39a83), hex(0x7b735f), 424.0, 452.0),
    );
    canvas.shell(
        (112.0, 446.0, 371.0, 476.0),
        12.0,
        (beige.0, beige.1),
        476.0,
        (hex(0xf6f0dc), hex(0x8c846f)),
    );
    canvas.shell(
        (58.0, 484.0, 425.0, 545.0),
        14.0,
        (beige.0, beige.1),
        545.0,
        (hex(0xf6f0dc), hex(0x8c846f)),
    );
    for column in 0..22 {
        let x = 76.0 + column as f32 * 15.6;
        canvas.solid((x, 496.0, x + 12.0, 506.0), 2.0, hex(0x9a927b));
    }
}

/// A handheld: light grey shell, dark screen bezel, D-pad and buttons.
fn draw_handheld(canvas: &mut Canvas) {
    let accent = hex(0x35c48f);
    let shell = (34.0, 46.0, 449.0, 560.0);
    canvas.solid(
        (shell.0 - 2.0, shell.1 - 2.0, shell.2 + 2.0, shell.3 + 2.0),
        28.0,
        hex(0x0d100e),
    );
    canvas.shell(
        shell,
        26.0,
        (hex(0xd3d4cb), hex(0xa7a99f)),
        480.0,
        (hex(0xf3f4ec), hex(0x7d8078)),
    );
    // Dark screen bezel with the accent band along its top and a label.
    canvas.solid((56.0, 52.0, 427.0, 412.0), 20.0, hex(0x2c2e33));
    canvas.solid((58.0, 54.0, 425.0, 410.0), 18.0, hex(0x3c3f46));
    glass_surround(canvas, accent, 12.0);
    canvas.solid(
        (82.0, 391.0, 130.0, 396.0),
        2.0,
        mix(hex(0x3c3f46), accent, 0.9),
    );
    canvas.solid((84.0, 399.0, 200.0, 404.0), 2.0, hex(0x7d8078));
    // D-pad on the left.
    let (dx, dy) = (140.0_f32, 452.0_f32);
    let pad = |canvas: &mut Canvas, extra: f32, colour: Rgb| {
        canvas.solid(
            (
                dx - 44.0 - extra,
                dy - 14.0 - extra,
                dx + 44.0 + extra,
                dy + 14.0 + extra,
            ),
            6.0,
            colour,
        );
        canvas.solid(
            (
                dx - 14.0 - extra,
                dy - 44.0 - extra,
                dx + 14.0 + extra,
                dy + 44.0 + extra,
            ),
            6.0,
            colour,
        );
    };
    pad(canvas, 3.0, hex(0x6e7169));
    pad(canvas, 0.0, hex(0x2a2c31));
    canvas.circle(dx, dy, 7.0, |_, _| hex(0x1c1e22));
    // A and B buttons, slightly diagonal, on the right.
    for (bx, by) in [(342.0_f32, 462.0_f32), (392.0, 436.0)] {
        canvas.circle(bx, by, 22.0, |_, _| hex(0x6e7169));
        canvas.circle(
            bx,
            by,
            19.0,
            Canvas::vertical(
                mix(accent, hex(0xffffff), 0.25),
                mix(accent, hex(0x0a0d0b), 0.45),
                by - 19.0,
                by + 19.0,
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [DeviceKind; 3] = [DeviceKind::Tv, DeviceKind::Monitor, DeviceKind::Handheld];

    #[test]
    fn every_device_has_the_cabinet_geometry_and_a_black_opening() {
        for kind in KINDS {
            let pixels = device_rgb565(kind);
            assert_eq!(pixels.len(), DEVICE_WIDTH * DEVICE_HEIGHT, "{kind:?}");
            for y in SCREEN_Y..SCREEN_Y + SCREEN_SIZE {
                for x in SCREEN_X..SCREEN_X + SCREEN_SIZE {
                    assert_eq!(
                        pixels[y * DEVICE_WIDTH + x],
                        0,
                        "{kind:?} opening at {x},{y}"
                    );
                }
            }
        }
    }

    #[test]
    fn devices_are_distinct_and_frame_the_opening() {
        let images: Vec<_> = KINDS.iter().map(|kind| device_rgb565(*kind)).collect();
        assert!(images[0] != images[1] && images[1] != images[2] && images[0] != images[2]);
        for (kind, pixels) in KINDS.iter().zip(&images) {
            // The surround directly outside the opening is lit, not background.
            let above = pixels[(SCREEN_Y - 8) * DEVICE_WIDTH + SCREEN_X + 160];
            let beside = pixels[(SCREEN_Y + 160) * DEVICE_WIDTH + SCREEN_X - 8];
            assert!(above != 0 || beside != 0, "{kind:?} has no surround");
            // The visible crop (rows 42..465) contains a lot of device.
            let lit = pixels[42 * DEVICE_WIDTH..465 * DEVICE_WIDTH]
                .iter()
                .filter(|pixel| **pixel != 0)
                .count();
            assert!(lit > 60_000, "{kind:?} lit pixels {lit}");
        }
    }

    #[test]
    fn hero_keeps_the_device_and_the_requested_size() {
        for kind in KINDS {
            let hero = hero_rgb565(kind, 232, 102);
            assert_eq!(hero.len(), 232 * 102);
            let lit = hero.iter().filter(|pixel| **pixel != 0).count();
            assert!(lit > 232 * 102 / 4, "{kind:?} lit {lit}");
        }
    }

    /// `DEVICE_ART_DUMP=/dir cargo test ... dump_device_art -- --ignored`
    #[test]
    #[ignore = "writes review images"]
    fn dump_device_art() {
        let Some(dir) = std::env::var_os("DEVICE_ART_DUMP") else {
            return;
        };
        for kind in KINDS {
            let mut ppm = format!("P6\n{DEVICE_WIDTH} {DEVICE_HEIGHT}\n255\n").into_bytes();
            for pixel in device_rgb565(kind) {
                let (r, g, b) = (
                    (pixel >> 11) as u8,
                    ((pixel >> 5) & 63) as u8,
                    (pixel & 31) as u8,
                );
                ppm.extend([
                    (r << 3) | (r >> 2),
                    (g << 2) | (g >> 4),
                    (b << 3) | (b >> 2),
                ]);
            }
            std::fs::write(
                std::path::Path::new(&dir).join(format!("{kind:?}.ppm")),
                ppm,
            )
            .unwrap();
        }
    }
}
