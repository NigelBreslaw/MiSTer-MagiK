// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Artwork and scale preparation: no allocations or artwork generation in motion.
use super::*;

#[cfg(test)]
#[derive(Default)]
struct Raster {
    pixels: Vec<Rgb565Pixel>,
    alpha: Vec<u8>,
    reflection: Vec<Rgb565Pixel>,
    opaque: Vec<(u16, u16)>,
}
/// Card bodies that do not depend on the label, shared by every generic card
/// of a level. Drawing a body costs a 16-sample antialiased pass over every
/// pixel; on the device that is most of a level change, so it runs once per
/// distinct body rather than once per card and face.
#[derive(Default)]
pub(super) struct BodyCache {
    surfaces: Vec<((LauncherCardId, u16, bool), Vec<Rgb565Pixel>)>,
    backs: Vec<((LauncherCardId, u16), Vec<Rgb565Pixel>)>,
}

/// Generic cards have a reverse side: the MagiK back, in the card's colour.
/// Cards with approved artwork are not generic and have none.
pub(super) fn has_back(card: &PreparedCard<'_>) -> bool {
    category_icon(card.id).is_some() && card.artwork.is_none() && card.rgb888.is_none()
}

impl BodyCache {
    /// The 180x252 MagiK back for a generic card's colour.
    fn back(&mut self, card: &PreparedCard<'_>) -> &[Rgb565Pixel] {
        let key = (card.id, card.colour);
        let index = match self.backs.iter().position(|(k, _)| *k == key) {
            Some(index) => index,
            None => {
                self.backs.push((key, back_surface(card)));
                self.backs.len() - 1
            }
        };
        &self.backs[index].1
    }

    fn surface(&mut self, card: &PreparedCard<'_>, detail: bool) -> &[Rgb565Pixel] {
        let key = (card.id, card.colour, detail);
        let index = match self.surfaces.iter().position(|(k, _)| *k == key) {
            Some(index) => index,
            None => {
                self.surfaces
                    .push((key, surface(card, 180, detail, None, false)));
                self.surfaces.len() - 1
            }
        };
        &self.surfaces[index].1
    }

    /// The HDMI landscape back face, when the card has one.
    pub(super) fn back_face(
        &mut self,
        card: &PreparedCard<'_>,
    ) -> Option<crate::launcher_flip::Face> {
        has_back(card).then(|| {
            crate::launcher_flip::Face::new(self.back(card).to_vec(), 180, card_height(180))
        })
    }
}

/// The MagiK back: the card's frame around a dark crosshatch, with the M
/// emblem in a diamond. Drawn over a generic card's own surface so the frame,
/// corners and silhouette match its front exactly.
fn back_surface(card: &PreparedCard<'_>) -> Vec<Rgb565Pixel> {
    const W: usize = 180;
    let height = card_height(W);
    let mut pixels = surface(card, W, false, None, false);
    let base = mix_colour(rgb(5, 8, 13), card.colour, 34);
    let line = mix_colour(base, card.colour, 70);
    let (cx, cy) = (W as i64 / 2, height as i64 / 2);
    let m = glyph('M');
    for y in 0..height {
        for x in 0..W {
            if !rounded_contains(x, y, W, height)
                || !inside_inset(x * 8 + 4, y * 8 + 4, W, height, 8)
            {
                continue;
            }
            let (dx, dy) = ((x as i64 - cx).abs(), (y as i64 - cy).abs());
            let mut colour = if (x + y).is_multiple_of(14) || (x + 2 * W - y).is_multiple_of(14) {
                line
            } else {
                base
            };
            let diamond = dx + dy;
            if diamond <= 46 {
                colour = if diamond >= 42 {
                    card.colour
                } else {
                    rgb(4, 6, 10)
                };
            }
            // The M: a 5x7 glyph at 7x, centred in the diamond.
            let (gx, gy) = (x as i64 - (cx - 17), y as i64 - (cy - 24));
            if diamond < 42 && (0..35).contains(&gx) && (0..49).contains(&gy) {
                let (col, row) = ((gx / 7) as usize, (gy / 7) as usize);
                if m[row] & (1 << (4 - col)) != 0 {
                    colour = CREAM;
                }
            }
            pixels[y * W + x] = Rgb565Pixel(colour);
        }
    }
    pixels
}

/// The card's title, and its game count on the focused face, in the
/// production fonts. `stride` x `rows` is the pixel buffer being drawn into and
/// `width` x `height` the card inside it. A title too wide for the card drops
/// to the smaller metadata font rather than being clipped.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_card_labels(
    pixels: &mut [Rgb565Pixel],
    stride: usize,
    rows: usize,
    card: &PreparedCard<'_>,
    width: usize,
    height: usize,
    detail: bool,
    fonts: LauncherTypography<'_>,
) {
    #[cfg(feature = "launcher-profile")]
    let _labels = crate::launcher_profile::span("prepare.labels");
    let heading = fonts.font_for(TextRole::Heading, card.name);
    let title = if heading.measure(card.name) + 12 <= width {
        heading
    } else {
        fonts.font_for(TextRole::Metadata, card.name)
    };
    title.draw_centered(
        pixels,
        stride,
        rows,
        (width / 2) as i32,
        (height * 73 / 100) as i32,
        card.name,
        CREAM,
    );
    if detail && let Some(game_count) = card.games {
        let games = format_games(game_count);
        fonts.font_for(TextRole::Metadata, &games).draw_centered(
            pixels,
            stride,
            rows,
            (width / 2) as i32,
            (height * 86 / 100) as i32,
            &games,
            CREAM,
        );
    }
}

pub(super) fn draw_face_labels(
    pixels: &mut [Rgb565Pixel],
    card: &PreparedCard<'_>,
    width: usize,
    detail: bool,
    fonts: Option<LauncherTypography<'_>>,
) {
    let height = card_height(width);
    if let Some(fonts) = fonts {
        draw_card_labels(pixels, width, height, card, width, height, detail, fonts);
        return;
    }
    // Portable fallback uses the same glyph rectangles, clipped to the face
    // instead of the temporary 960x540 logical surface.
    let mut draw = |mask: &[[u8; 7]], y: usize, maximum: usize| {
        let scale = ((width - 24) / (mask.len().max(1) * 6)).clamp(1, maximum) * 256;
        let origin = width.saturating_sub(mask.len() * 6 * scale / 256) / 2;
        for (i, glyph) in mask.iter().enumerate() {
            let glyph_x = origin + i * 6 * scale / 256;
            for (row, bits) in glyph.iter().enumerate() {
                for column in 0..5 {
                    if bits & (1 << (4 - column)) == 0 {
                        continue;
                    }
                    let x0 = glyph_x + column * scale / 256;
                    let x1 = (glyph_x + (column + 1) * scale / 256).max(x0 + 1);
                    let y0 = y + row * scale / 256;
                    let y1 = (y + (row + 1) * scale / 256).max(y0 + 1);
                    for yy in y0..y1.min(height) {
                        for xx in x0..x1.min(width) {
                            pixels[yy * width + xx] = Rgb565Pixel(CREAM);
                        }
                    }
                }
            }
        }
    };
    draw(&card.name_mask, height * 73 / 100, 3);
    if detail && card.games.is_some() {
        draw(&card.games_mask, height * 86 / 100, 2);
    }
}

/// `face`, reusing the label-free body when the card has no artwork and the
/// production fonts draw the labels.
pub(super) fn face_cached(
    card: &PreparedCard<'_>,
    width: usize,
    detail: bool,
    typography: Option<LauncherTypography<'_>>,
    cache: &mut BodyCache,
) -> crate::launcher_flip::Face {
    let (Some(fonts), 180, None) = (typography, width, card.artwork) else {
        return face(card, width, detail, typography);
    };
    let height = card_height(width);
    let mut pixels = cache.surface(card, detail).to_vec();
    draw_card_labels(
        &mut pixels,
        width,
        height,
        card,
        width,
        height,
        detail,
        fonts,
    );
    crate::launcher_flip::Face::new(pixels, width, height)
}

// The input is eight-bit and output thresholds are fixed. Cache these tiny
// transfer tables instead of calling powf for every reduced colour sample.
struct SrgbTransfer {
    decode: [f64; 256],
    boundaries: [f64; 255],
}
fn srgb_transfer() -> &'static SrgbTransfer {
    static TRANSFER: std::sync::OnceLock<SrgbTransfer> = std::sync::OnceLock::new();
    TRANSFER.get_or_init(|| {
        let linear = |s: f64| {
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        SrgbTransfer {
            decode: std::array::from_fn(|i| linear(i as f64 / 255.0)),
            boundaries: std::array::from_fn(|i| linear((i as f64 + 0.5) / 255.0)),
        }
    })
}
impl SrgbTransfer {
    fn encode(&self, linear: f64) -> u8 {
        self.boundaries
            .partition_point(|&boundary| linear >= boundary) as u8
    }
}

pub(super) fn reduce_rgb888(source: &[u8]) -> Vec<[u8; 3]> {
    assert_eq!(source.len(), 360 * 504 * 3);
    let transfer = srgb_transfer();
    let rgb8: Vec<[u8; 3]> = (0..252)
        .flat_map(|y| (0..180).map(move |x| (x, y)))
        .map(|(x, y)| {
            std::array::from_fn(|c| {
                let i = (y * 2 * 360 + x * 2) * 3 + c;
                let values = [
                    source[i],
                    source[i + 3],
                    source[i + 360 * 3],
                    source[i + 360 * 3 + 3],
                ];
                // Constant channels need neither a gamma decode nor a binary
                // search. This is exactly the same rounded sRGB result.
                if values.iter().all(|&v| v == values[0]) {
                    return values[0];
                }
                let mut sum = 0.0;
                for value in values {
                    sum += transfer.decode[value as usize];
                }
                transfer.encode(sum / 4.0)
            })
        })
        .collect();
    rgb8
}

pub(super) fn faces_rgb888(
    card: &PreparedCard<'_>,
    typography: Option<LauncherTypography<'_>>,
) -> [crate::launcher_flip::Face; 2] {
    #[cfg(feature = "launcher-profile")]
    let reduction = crate::launcher_profile::span("prepare.rgb888_linear_reduction");
    let rgb8 = reduce_rgb888(card.rgb888.expect("validated RGB888 source"));
    #[cfg(feature = "launcher-profile")]
    drop(reduction);
    let reference: Vec<_> = rgb8
        .iter()
        .map(|&[r, g, b]| {
            Rgb565Pixel((u16::from(r) >> 3) << 11 | (u16::from(g) >> 2) << 5 | u16::from(b) >> 3)
        })
        .collect();
    let mapped = PreparedCard {
        id: card.id,
        name: card.name,
        games: card.games,
        colour: card.colour,
        name_mask: card.name_mask.clone(),
        games_mask: card.games_mask.clone(),
        artwork: Some(&reference),
        rgb888: None,
    };
    std::array::from_fn(|index| {
        #[cfg(feature = "launcher-profile")]
        let _face = crate::launcher_profile::span("prepare.rgb888_face");
        let pixels = surface(&mapped, 180, index == 1, typography, true);
        crate::launcher_flip::Face::with_rgb8(pixels, &rgb8, &reference, 180, 252)
    })
}

// Independent single-face reference for exact startup pixel/mipmap parity.
#[cfg(test)]
fn reference_face_rgb888(
    card: &PreparedCard<'_>,
    detail: bool,
    typography: Option<LauncherTypography<'_>>,
) -> crate::launcher_flip::Face {
    #[cfg(feature = "launcher-profile")]
    let _rgb888 = crate::launcher_profile::span("prepare.rgb888_face");
    #[cfg(feature = "launcher-profile")]
    let reduction = crate::launcher_profile::span("prepare.rgb888_linear_reduction");
    let source = card.rgb888.expect("validated RGB888 source");
    let transfer = srgb_transfer();
    let rgb8: Vec<[u8; 3]> = (0..252)
        .flat_map(|y| (0..180).map(move |x| (x, y)))
        .map(|(x, y)| {
            std::array::from_fn(|c| {
                let mut sum = 0.0;
                for dy in 0..2 {
                    for dx in 0..2 {
                        sum += transfer.decode
                            [source[((y * 2 + dy) * 360 + x * 2 + dx) * 3 + c] as usize];
                    }
                }
                transfer.encode(sum / 4.0)
            })
        })
        .collect();
    #[cfg(feature = "launcher-profile")]
    drop(reduction);
    let reference: Vec<_> = rgb8
        .iter()
        .map(|&[r, g, b]| {
            Rgb565Pixel((u16::from(r) >> 3) << 11 | (u16::from(g) >> 2) << 5 | u16::from(b) >> 3)
        })
        .collect();
    let mapped = PreparedCard {
        id: card.id,
        name: card.name,
        games: card.games,
        colour: card.colour,
        name_mask: card.name_mask.clone(),
        games_mask: card.games_mask.clone(),
        artwork: Some(&reference),
        rgb888: None,
    };
    let mut face = face(&mapped, 180, detail, typography);
    face.texture.retain_rgb8(&rgb8, &reference);
    face
}

pub(super) fn face(
    card: &PreparedCard<'_>,
    width: usize,
    detail: bool,
    typography: Option<LauncherTypography<'_>>,
) -> crate::launcher_flip::Face {
    #[cfg(feature = "launcher-profile")]
    let _face = crate::launcher_profile::span("prepare.face");
    crate::launcher_flip::Face::new(
        surface(card, width, detail, typography, true),
        width,
        card_height(width),
    )
}

pub(super) fn surface(
    card: &PreparedCard<'_>,
    width: usize,
    detail: bool,
    typography: Option<LauncherTypography<'_>>,
    labels: bool,
) -> Vec<Rgb565Pixel> {
    #[cfg(feature = "launcher-profile")]
    let _surface = crate::launcher_profile::span("prepare.surface");
    let height = card_height(width);
    let mut canvas = vec![Rgb565Pixel(0); LOGICAL_WIDTH * LOGICAL_HEIGHT];
    let icon = category_icon(card.id);
    // Generic collection cards keep one dark tint of the collection colour on
    // both faces; the count label, not a colour flood, marks the focused card.
    let base = if detail && icon.is_none() {
        if card.id == LauncherCardId::Arcade {
            rgb(222, 35, 52)
        } else {
            card.colour
        }
    } else {
        mix_colour(rgb(12, 22, 30), card.colour, 44)
    };
    let trim = card.colour;
    let ink = CREAM;
    #[cfg(feature = "launcher-profile")]
    let surface_pixels = crate::launcher_profile::span("prepare.surface_pixels");
    for y in 0..height {
        for x in 0..width {
            if !rounded_contains(x, y, width, height) {
                continue;
            }
            let mut colour = framed_surface(card, base, trim, width, height, x, y);
            if icon.is_some()
                && card.artwork.is_none()
                && inside_inset(x * 8 + 4, y * 8 + 4, width, height, 8)
            {
                colour = lit_body(card.colour, width, height, x, y);
            }
            canvas[y * LOGICAL_WIDTH + x] = Rgb565Pixel(colour);
        }
    }
    #[cfg(feature = "launcher-profile")]
    drop(surface_pixels);
    // Approved photographic artwork replaces the generated category symbol.
    // Keep the old fallback for tests and consumers which do not supply art.
    if card.artwork.is_none() {
        if card.id == LauncherCardId::Arcade {
            const CABINET: [u16; 20] = [
                0x7ffe, 0x4002, 0x5ffa, 0x5ffa, 0x4002, 0x6006, 0x2004, 0x27e4, 0x2424, 0x2424,
                0x27e4, 0x2004, 0x300c, 0x7ffe, 0x4002, 0x47e2, 0x4422, 0x4422, 0x7ffe, 0x6006,
            ];
            let scale = 4;
            let left = (width - 16 * scale) / 2;
            let top = height * 22 / 100;
            for (y, bits) in CABINET.iter().enumerate() {
                for x in 0..16 {
                    if bits & (1 << (15 - x)) != 0 {
                        draw_rect(
                            &mut canvas,
                            left + x * scale,
                            top + y * scale,
                            scale,
                            scale,
                            ink,
                        );
                    }
                }
            }
        } else if let Some(bits) = icon {
            // The collection's pixel symbol with a shadow in its own colour.
            let scale = 5;
            let left = (width - 16 * scale) / 2;
            let top = height * 22 / 100;
            let shadow = mix_colour(card.colour, BACKGROUND, 150);
            let glass = mix_colour(rgb(5, 7, 12), card.colour, 128);
            for (offset, colour) in [(3, shadow), (0, ink)] {
                for (y, row) in bits.iter().enumerate() {
                    for (x, cell) in row.bytes().enumerate() {
                        if cell != b'0' {
                            draw_rect(
                                &mut canvas,
                                left + x * scale + offset,
                                top + y * scale + offset,
                                scale,
                                scale,
                                if offset == 0 && cell == b'2' {
                                    glass
                                } else {
                                    colour
                                },
                            );
                        }
                    }
                }
            }
        } else {
            // A restrained collection monogram for artwork-free consumers.
            let initial = text_mask(&card.name.chars().next().unwrap_or('?').to_string());
            draw_mask_scaled_centered(
                &mut canvas,
                4,
                height / 4 + 4,
                width,
                &initial,
                mix_colour(base, BACKGROUND, 160),
                8 * 256,
            );
            draw_mask_scaled_centered(&mut canvas, 0, height / 4, width, &initial, ink, 8 * 256);
        }
    }
    let mut pixels: Vec<_> = (0..height)
        .flat_map(|y| {
            canvas[y * LOGICAL_WIDTH..y * LOGICAL_WIDTH + width]
                .iter()
                .copied()
        })
        .collect();
    if labels {
        draw_face_labels(&mut pixels, card, width, detail, typography);
    }
    pixels
}

/// The interior of a generic card: the collection colour lit from above and
/// fading to near black, with a soft diagonal sheen. Ordered dithering keeps
/// the dark gradient from banding in RGB565.
fn lit_body(colour: u16, width: usize, height: usize, x: usize, y: usize) -> u16 {
    const BAYER: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let top = mix_colour(rgb(12, 22, 30), colour, 88);
    let bottom = mix_colour(rgb(5, 8, 12), colour, 22);
    let t = (y * 256 / height.max(1)) as u32;
    let threshold = BAYER[y % 4][x % 4] * 16 + 8;
    let channel = |shift: u32, bits: u32| {
        let mask = (1_u16 << bits) - 1;
        let a = u32::from((top >> shift) & mask);
        let b = u32::from((bottom >> shift) & mask);
        (((a * (256 - t) + b * t) * 256 / 256 + threshold) / 256).min(u32::from(mask)) as u16
    };
    let lit = (channel(11, 5) << 11) | (channel(5, 6) << 5) | channel(0, 5);
    // Sheen: a diagonal band, as on the flat cards.
    if x * 2 + y > width * 2 && x * 2 + y < width * 5 / 2 {
        mix_colour(lit, CREAM, 22)
    } else {
        lit
    }
}

/// 16-pixel-wide symbols for generic cards below the Consoles, Computers and
/// Handhelds root cards. Every group and system in a collection shares one.
fn category_icon(id: LauncherCardId) -> Option<&'static [&'static str; 10]> {
    const GAMEPAD: [&str; 10] = [
        "0011111111111100",
        "0111111111111110",
        "1110111111111011",
        "1100011111110101",
        "1110111111111011",
        "1111111111111111",
        "1111110000111111",
        "1111100000011111",
        "0111000000001110",
        "0010000000000100",
    ];
    const COMPUTER: [&str; 10] = [
        "0001111111111000",
        "0001222222221000",
        "0001222222221000",
        "0001222222221000",
        "0001222222221000",
        "0001222222221000",
        "0001222222221000",
        "0001111111101000",
        "0000001111000000",
        "0000111111110000",
    ];
    const HANDHELD: [&str; 10] = [
        "0111111111111110",
        "1111111111111111",
        "1111122222211111",
        "1101122222211011",
        "1000122222210111",
        "1101122222211111",
        "1111122222211111",
        "1111111111111111",
        "1111111111111111",
        "0111111111111110",
    ];
    match id {
        LauncherCardId::Consoles => Some(&GAMEPAD),
        LauncherCardId::Computers => Some(&COMPUTER),
        LauncherCardId::Handhelds => Some(&HANDHELD),
        _ => None,
    }
}

// Eighth-pixel coordinates for preparation-only 4x4 coverage sampling.
// Retain a radius on the inner keyline as well as the outer cream silhouette.
fn inside_inset(x: usize, y: usize, width: usize, height: usize, inset: usize) -> bool {
    let edge_x = x.min(width * 8 - x);
    let edge_y = y.min(height * 8 - y);
    if edge_x < inset * 8 || edge_y < inset * 8 {
        return false;
    }
    let radius = 8_usize.saturating_sub(inset).max(4) * 8;
    let dx = radius.saturating_sub(edge_x - inset * 8);
    let dy = radius.saturating_sub(edge_y - inset * 8);
    dx * dx + dy * dy <= radius * radius
}

fn framed_surface(
    card: &PreparedCard<'_>,
    base: u16,
    trim: u16,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> u16 {
    // With artwork, all sixteen samples inside the innermost frame read
    // this same source pixel. Keep a conservative pair of rectangular bands:
    // each is wholly inside the inset-8 rounded rectangle, including sample
    // offsets 1..7. Corners and every frame boundary retain the sampled path.
    let edge_x = x.min(width - 1 - x);
    let edge_y = y.min(height - 1 - y);
    if edge_x >= 8
        && edge_y >= 8
        && (edge_x >= 12 || edge_y >= 12)
        && let Some(pixels) = card.artwork
    {
        return pixels[y * width + x].0;
    }
    sampled_framed_surface(card, base, trim, width, height, x, y)
}

fn sampled_framed_surface(
    card: &PreparedCard<'_>,
    base: u16,
    trim: u16,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> u16 {
    // Antialias only boundaries; interiors remain exact solid RGB565 inks.
    // The outer alpha coverage is supplied by Texture::new, not baked black.
    let mut channels = [0_u32; 3];
    for sy in 0..4 {
        for sx in 0..4 {
            let c = framed_sample(
                card,
                base,
                trim,
                width,
                height,
                x * 8 + sx * 2 + 1,
                y * 8 + sy * 2 + 1,
            );
            channels[0] += u32::from(c >> 11);
            channels[1] += u32::from((c >> 5) & 63);
            channels[2] += u32::from(c & 31);
        }
    }
    (((channels[0] + 8) / 16) << 11 | ((channels[1] + 8) / 16) << 5 | ((channels[2] + 8) / 16))
        as u16
}

// Half of the primary RGB565 stroke, baked as an opaque colour. Channel
// least-significant bits are masked before shifting to prevent cross-channel carry.
fn opaque_inner_stroke(primary: u16) -> u16 {
    (primary & 0xf7de) >> 1
}

fn framed_sample(
    card: &PreparedCard<'_>,
    base: u16,
    trim: u16,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> u16 {
    let source = card.artwork.as_ref().map_or_else(
        || surface_sample(base, width, x, y),
        |pixels| {
            let px = (x / 8).min(width - 1);
            let py = (y / 8).min(height - 1);
            pixels[py * width + px].0
        },
    );
    if !inside_inset(x, y, width, height, 3) {
        mix_colour(trim, CREAM, 76)
    } else if !inside_inset(x, y, width, height, 8) {
        // The complete inner stroke is one opaque ink. Artwork must not
        // contribute colour anywhere in the former shoulder or keyline bands.
        opaque_inner_stroke(mix_colour(trim, CREAM, 76))
    } else {
        source
    }
}

#[cfg(test)]
fn raster(face: &crate::launcher_flip::Face, width: usize) -> Raster {
    let height = card_height(width);
    let texture = &face.texture;
    let mut pixels = vec![Rgb565Pixel(0); width * height];
    let mut alpha = vec![0; width * height];
    let mut column = vec![0; face.height];
    let mut reflection = vec![Rgb565Pixel(0); width * 64];
    let mut reflected_column = [0; 64];
    let mut horizontal = vec![0; width * face.height];
    let source_rows: Vec<_> = (0..height)
        .map(|y| source_row_q16(y as u32, face.height as u32, height as u32) as usize)
        .collect();
    for x in 0..width {
        let sx = ((x as i64 * (face.width - 1) as i64 * 65536) / (width - 1) as i64) as i32;
        let filter = texture.filter(sx, (face.width * 65536 / width) as u32);
        texture.prepare_column(filter, &mut column);
        // Reuse the same full-face reduction as the turning card. Do not
        // rebuild a vertical reduction for each of the 35 cached widths.
        face.texture
            .reflection(64)
            .prepare_column(filter, &mut reflected_column);
        for (row, &sample) in reflected_column.iter().enumerate() {
            reflection[row * width + x] = Rgb565Pixel(reflection_colour(
                crate::launcher_texture::over(sample, Rgb565Pixel(0)).0,
                x,
                row,
            ));
        }
        for (y, &sample) in column.iter().enumerate() {
            horizontal[y * width + x] = sample;
        }
    }
    for (y, &sy) in source_rows.iter().enumerate() {
        let a = sy / 65536 * width;
        let b = (sy / 65536 + 1).min(face.height - 1) * width;
        crate::launcher_texture::raster_row(
            &mut pixels[y * width..(y + 1) * width],
            &mut alpha[y * width..(y + 1) * width],
            &horizontal[a..a + width],
            &horizontal[b..b + width],
            ((sy & 65535) >> 8) as u32,
        );
    }
    let opaque = alpha
        .chunks_exact(width)
        .map(|row| {
            let left = row.iter().position(|&a| a == 255).unwrap_or(width);
            let right = row.iter().rposition(|&a| a == 255).map_or(left, |x| x + 1);
            (left as u16, right as u16)
        })
        .collect();
    Raster {
        pixels,
        alpha,
        reflection,
        opaque,
    }
}

#[cfg(test)]
fn source_row_q16(y: u32, source_height: u32, height: u32) -> u32 {
    // 269 * 269 * 65536 exceeds u32::MAX: widening must happen BEFORE
    // multiplication, otherwise ARM wraps the card bottom back to its top.
    (u64::from(y) * u64::from(source_height - 1) * 65536 / u64::from(height - 1)) as u32
}

const REFLECTION_CURVES: [[[u8; 64]; 4]; 64] = {
    const BAYER: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let mut table = [[[0; 64]; 4]; 64];
    let mut row = 0;
    while row < 64 {
        let left = 63 - row;
        let alpha = (150 * left * left / (63 * 63)) as u32;
        let mut x = 0;
        while x < 4 {
            let threshold = BAYER[row % 4][x] * 16 + 8;
            let mut c = 0;
            while c < 64 {
                let v = c as u32 * alpha;
                table[row][x][c] = (v / 256 + if v % 256 > threshold { 1 } else { 0 }) as u8;
                c += 1;
            }
            x += 1;
        }
        row += 1;
    }
    table
};

#[inline]
pub(super) fn reflection_colour(source: u16, x: usize, row: usize) -> u16 {
    let curve = &REFLECTION_CURVES[row.min(63)][x % 4];
    u16::from(curve[(source >> 11) as usize]) << 11
        | u16::from(curve[((source >> 5) & 63) as usize]) << 5
        | u16::from(curve[(source & 31) as usize])
}

fn surface_sample(base: u16, width: usize, x: usize, y: usize) -> u16 {
    // Two solid inks: no gradient or baked dither to distort during filtering.
    if x * 2 + y > width * 16 && x * 2 + y < width * 20 {
        mix_colour(base, CREAM, 36)
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inner_stroke_is_opaque_and_independent_of_artwork() {
        let red = vec![Rgb565Pixel(rgb(255, 0, 0)); 180 * 252];
        let blue = vec![Rgb565Pixel(rgb(0, 0, 255)); 180 * 252];
        let mut card = test_card(rgb(160, 170, 100));
        for artwork in [&red[..], &blue[..]] {
            card.artwork = Some(artwork);
            let expected = opaque_inner_stroke(mix_colour(card.colour, CREAM, 76));
            let prepared = face(&card, 180, false, None);
            // Cover every pixel of the former artwork-blended shoulder AND
            // keyline, on both vertical sides and the top/bottom straight runs.
            for edge in 3..8 {
                for y in [32, 100, 126, 200, 220] {
                    for x in [edge, 179 - edge] {
                        let point =
                            framed_sample(&card, 0, card.colour, 180, 252, x * 8 + 4, y * 8 + 4);
                        assert_eq!(point, expected, "stroke {x},{y}");
                        assert_eq!(
                            prepared.pixels[y * 180 + x].0,
                            expected,
                            "prepared stroke {x},{y}"
                        );
                    }
                }
                for x in [32, 90, 148] {
                    for y in [edge, 251 - edge] {
                        assert_eq!(
                            prepared.pixels[y * 180 + x].0,
                            expected,
                            "prepared stroke {x},{y}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn artwork_interior_matches_subpixel_reference_at_every_pixel() {
        for width in [24, 36, 72, 180] {
            let height = card_height(width);
            let pixels: Vec<_> = (0..width * height)
                .map(
                    |i| Rgb565Pixel((i as u32).wrapping_mul(1103515245).wrapping_add(12345) as u16),
                )
                .collect();
            let mut card = test_card(0xb79a);
            card.artwork = Some(&pixels);
            for y in 0..height {
                for x in 0..width {
                    assert_eq!(
                        framed_surface(&card, 0x8395, card.colour, width, height, x, y),
                        sampled_framed_surface(&card, 0x8395, card.colour, width, height, x, y),
                        "frame/silhouette must retain exact samples: {width} {x},{y}",
                    );
                }
            }
        }
    }

    #[test]
    fn shared_rgb888_preparation_preserves_faces_and_every_mip() {
        use crate::bitmap_text::BitmapGlyph;
        let font = BitmapFont {
            ascent: 7,
            descent: 0,
            glyphs: (32..127)
                .map(|c| BitmapGlyph {
                    code_point: char::from_u32(c).unwrap(),
                    left: 0,
                    top: 7,
                    width: 5,
                    height: 7,
                    advance: 6,
                    alpha: (0..35)
                        .map(|i| if (i + c).is_multiple_of(3) { 128 } else { 255 })
                        .collect(),
                })
                .collect(),
        };
        let fonts = LauncherTypography {
            heading: &font,
            number: &font,
            metadata: &font,
            fallback: &font,
        };
        let artwork: [&[u8]; 6] = [
            include_bytes!("../../../../apps/mister/assets/ui/launcher-cards/01_arcade.rgb888"),
            include_bytes!("../../../../apps/mister/assets/ui/launcher-cards/02_consoles.rgb888"),
            include_bytes!("../../../../apps/mister/assets/ui/launcher-cards/03_computers.rgb888"),
            include_bytes!("../../../../apps/mister/assets/ui/launcher-cards/04_handhelds.rgb888"),
            include_bytes!("../../../../apps/mister/assets/ui/launcher-cards/05_favourites.rgb888"),
            include_bytes!("../../../../apps/mister/assets/ui/launcher-cards/06_settings.rgb888"),
        ];
        for source in artwork {
            let mut card = test_card(0xa472);
            card.rgb888 = Some(source);
            for typography in [None, Some(fonts)] {
                for (index, actual) in faces_rgb888(&card, typography).into_iter().enumerate() {
                    let expected = reference_face_rgb888(&card, index == 1, typography);
                    assert_eq!(actual.pixels, expected.pixels);
                    assert!(
                        actual.texture == expected.texture,
                        "RGBA source precision and all mip levels must match"
                    );
                }
            }
        }
    }

    #[test]
    fn gamma_lookup_matches_reference_for_every_pair_and_real_card_reduction() {
        let transfer = srgb_transfer();
        let reference = |l: f64| {
            let s = if l <= 0.0031308 {
                l * 12.92
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0).round().clamp(0.0, 255.0) as u8
        };
        for a in 0..256 {
            for b in 0..256 {
                let value = (transfer.decode[a] + transfer.decode[b]) / 2.0;
                assert_eq!(transfer.encode(value), reference(value));
            }
        }
        let artwork: [&[u8]; 6] = [
            include_bytes!("../../../../apps/mister/assets/ui/launcher-cards/01_arcade.rgb888"),
            include_bytes!("../../../../apps/mister/assets/ui/launcher-cards/02_consoles.rgb888"),
            include_bytes!("../../../../apps/mister/assets/ui/launcher-cards/03_computers.rgb888"),
            include_bytes!("../../../../apps/mister/assets/ui/launcher-cards/04_handhelds.rgb888"),
            include_bytes!("../../../../apps/mister/assets/ui/launcher-cards/05_favourites.rgb888"),
            include_bytes!("../../../../apps/mister/assets/ui/launcher-cards/06_settings.rgb888"),
        ];
        for source in artwork {
            for y in 0..252 {
                for x in 0..180 {
                    for c in 0..3 {
                        let mut sum = 0.0;
                        for dy in 0..2 {
                            for dx in 0..2 {
                                sum += transfer.decode
                                    [source[((y * 2 + dy) * 360 + x * 2 + dx) * 3 + c] as usize];
                            }
                        }
                        assert_eq!(transfer.encode(sum / 4.0), reference(sum / 4.0));
                    }
                }
            }
        }
    }

    #[test]
    fn reflection_formula_matches_every_baked_curve_entry() {
        const BAYER: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        for row in 0..64 {
            let left = 63 - row;
            let alpha = (150 * left * left / (63 * 63)) as u32;
            for x in 0..4 {
                let threshold = BAYER[row % 4][x] * 16 + 8;
                for (channel, &expected) in REFLECTION_CURVES[row][x].iter().enumerate() {
                    let value = channel as u32 * alpha;
                    let calculated = value / 256 + u32::from(value % 256 > threshold);
                    assert_eq!(calculated as u8, expected);
                }
            }
        }
    }

    fn test_card<'a>(colour: u16) -> PreparedCard<'a> {
        PreparedCard {
            id: LauncherCardId::Handhelds,
            name: "HANDHELDS",
            games: Some(126),
            colour,
            name_mask: text_mask("HANDHELDS"),
            games_mask: text_mask("126 GAMES"),
            artwork: None,
            rgb888: None,
        }
    }

    #[test]
    fn thick_coloured_rim_is_continuous_around_all_four_corners() {
        let (width, height) = (180, 252);
        let card = test_card(0x2c92);
        let rim = mix_colour(card.colour, CREAM, 76);
        for y in 0..height {
            let x = (0..width / 2)
                .find(|&x| rounded_contains(x, y, width, height))
                .unwrap();
            for mirrored_x in [x, width - 1 - x] {
                assert!(rounded_contains(mirrored_x, y, width, height));
                assert_eq!(
                    framed_surface(
                        &card,
                        card.colour,
                        card.colour,
                        width,
                        height,
                        mirrored_x,
                        y,
                    ),
                    rim
                );
            }
        }
    }

    #[test]
    fn faces_have_no_ordinal_dots_or_top_dash() {
        let card = test_card(0x2c92);
        for detail in [false, true] {
            let face = face(&card, 180, detail, None);
            let base = if detail && category_icon(card.id).is_none() {
                card.colour
            } else {
                mix_colour(rgb(12, 22, 30), card.colour, 44)
            };
            let trim = card.colour;
            for (xs, ys) in [(12..30, 14..22), (70..112, 230..235), (80..100, 0..8)] {
                for y in ys {
                    for x in xs.clone() {
                        let expected = if category_icon(card.id).is_some()
                            && inside_inset(x * 8 + 4, y * 8 + 4, 180, 252, 8)
                        {
                            lit_body(card.colour, 180, 252, x, y)
                        } else {
                            framed_surface(&card, base, trim, 180, 252, x, y)
                        };
                        assert_eq!(face.pixels[y * 180 + x].0, expected);
                    }
                }
            }
        }
    }

    #[test]
    fn compact_artwork_and_keyline_keep_their_colours() {
        let source = rgb(220, 34, 78);
        let mut card = test_card(rgb(32, 112, 238));
        let pixels = vec![Rgb565Pixel(source); 180 * 252];
        card.artwork = Some(&pixels);
        let compact = face(&card, 180, false, None);
        let detail = face(&card, 180, true, None);
        let centre = 100 * 180 + 90;
        let rim = 12 * 180;
        assert_eq!(compact.pixels[centre].0, source);
        assert_eq!(detail.pixels[centre].0, source);
        assert_eq!(compact.pixels[rim].0, mix_colour(card.colour, CREAM, 76));
        assert_eq!(detail.pixels[rim].0, mix_colour(card.colour, CREAM, 76));
        assert_eq!(compact.pixels[rim], detail.pixels[rim]);
    }

    #[test]
    fn settings_heading_stays_light_across_compact_and_detail_faces() {
        let card = PreparedCard {
            id: LauncherCardId::Settings,
            name: "SETTINGS",
            games: None,
            colour: 0x8b7f,
            name_mask: text_mask("SETTINGS"),
            games_mask: Vec::new(),
            artwork: None,
            rgb888: None,
        };
        let heading_pixel = 183 * 180 + 21;
        for detail in [false, true] {
            let face = face(&card, 180, detail, None);
            assert_eq!(face.pixels[heading_pixel].0, CREAM);
        }
    }

    #[test]
    fn inner_corners_and_diagonal_edges_have_subpixel_coverage() {
        let base = rgb(222, 35, 52);
        let card = test_card(base);
        let trim = card.colour;
        let mut mixed_corner = false;
        for y in 6..14 {
            for x in 6..14 {
                let mut inks = std::collections::BTreeSet::new();
                for sy in 0..4 {
                    for sx in 0..4 {
                        inks.insert(framed_sample(
                            &card,
                            base,
                            trim,
                            180,
                            252,
                            x * 8 + sx * 2 + 1,
                            y * 8 + sy * 2 + 1,
                        ));
                    }
                }
                if inks.len() > 1 {
                    mixed_corner |=
                        !inks.contains(&framed_surface(&card, base, trim, 180, 252, x, y));
                }
            }
        }
        assert!(
            mixed_corner,
            "inner curved border must have coverage blended pixels"
        );
        assert!(!inside_inset(8 * 8 + 1, 8 * 8 + 1, 180, 252, 8));
        assert!(inside_inset(12 * 8, 8 * 8 + 1, 180, 252, 8));
        let beam = mix_colour(base, CREAM, 36);
        let diagonal = framed_surface(&card, base, trim, 180, 252, 129, 100);
        assert_ne!(diagonal, base);
        assert_ne!(diagonal, beam);
        assert_eq!(framed_surface(&card, base, trim, 180, 252, 90, 100), base);
    }

    #[test]
    fn surface_has_only_solid_base_and_diagonal_beam() {
        let base = rgb(222, 35, 52);
        let beam = mix_colour(base, CREAM, 36);
        assert_ne!(base, beam);
        let mut beam_pixels = 0;
        for y in 0..252 {
            for x in 0..180 {
                let expected = if x * 2 + y > 360 && x * 2 + y < 450 {
                    beam_pixels += 1;
                    beam
                } else {
                    base
                };
                assert_eq!(surface_sample(base, 180, x * 8, y * 8), expected);
            }
        }
        assert!(beam_pixels > 0);
    }
    #[test]
    fn large_card_bottom_never_wraps_to_top_on_32_bit_targets() {
        assert!(269_u64 * 269 * 65536 > u64::from(u32::MAX));
        for height in (168..=270).step_by(3) {
            let mut previous = 0;
            for y in 0..height {
                let row = source_row_q16(y, 270, height);
                assert!(row >= previous && row <= 269 * 65536);
                previous = row;
            }
            assert_eq!(previous, 269 * 65536);
        }
        assert_eq!(source_row_q16(244, 270, 270), 244 * 65536);
        assert_eq!(source_row_q16(269, 270, 270), 269 * 65536);
    }
    #[test]
    fn front_card_and_reflection_keep_bottom_colour_not_top_colour() {
        let pixels = (0..180 * 270)
            .map(|i| {
                Rgb565Pixel(if i / 180 < 26 {
                    0xf800
                } else if i / 180 >= 244 {
                    0x001f
                } else {
                    0x07e0
                })
            })
            .collect();
        let face = crate::launcher_flip::Face::new(pixels, 180, 270);
        let cached = raster(&face, 180);
        assert_eq!(
            cached.pixels[(card_height(180) - 10) * 180 + 90],
            Rgb565Pixel(0x001f)
        );
        assert_eq!(
            cached.reflection[4 * 180 + 90],
            Rgb565Pixel(reflection_colour(0x001f, 90, 4))
        );
        for (y, &(left, right)) in cached.opaque.iter().enumerate() {
            for x in 0..180 {
                assert_eq!(
                    cached.alpha[y * 180 + x] == 255,
                    (usize::from(left)..usize::from(right)).contains(&x)
                );
            }
        }
    }
    #[test]
    fn reflection_is_strong_at_contact_and_fades_to_exact_black() {
        for x in 0..4 {
            assert_eq!(reflection_colour(0, x, 0), 0);
            assert_eq!(reflection_colour(0xffff, x, 63), 0);
            assert!(reflection_colour(0xffff, x, 0) >> 11 >= 17);
            assert!(reflection_colour(0xffff, x, 32) >> 11 < 6);
        }
    }
}
