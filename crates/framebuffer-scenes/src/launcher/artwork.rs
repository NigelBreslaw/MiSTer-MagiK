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
pub(super) fn face(
    card: &PreparedCard,
    width: usize,
    detail: bool,
    typography: Option<LauncherTypography<'_>>,
) -> crate::launcher_flip::Face {
    crate::launcher_flip::Face::new(
        surface(card, width, detail, typography, true),
        width,
        card_height(width),
    )
}

pub(super) fn surface(
    card: &PreparedCard,
    width: usize,
    detail: bool,
    typography: Option<LauncherTypography<'_>>,
    labels: bool,
) -> Vec<Rgb565Pixel> {
    let height = card_height(width);
    let mut canvas = vec![Rgb565Pixel(0); LOGICAL_WIDTH * LOGICAL_HEIGHT];
    let base = if detail {
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
    for y in 0..height {
        for x in 0..width {
            if !rounded_contains(x, y, width, height) {
                continue;
            }
            let colour = framed_surface(card, base, trim, width, height, x, y);
            canvas[y * LOGICAL_WIDTH + x] = Rgb565Pixel(colour);
        }
    }
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
    if !labels {
        // Responsive faces add native-size bitmap labels after artwork resampling.
    } else if let Some(fonts) = typography {
        fonts.font_for(TextRole::Heading, &card.name).draw_centered(
            &mut canvas,
            LOGICAL_WIDTH,
            LOGICAL_HEIGHT,
            (width / 2) as i32,
            (height * 73 / 100) as i32,
            &card.name,
            ink,
        );
        if detail && let Some(game_count) = card.games {
            let games = format_games(game_count);
            fonts.font_for(TextRole::Metadata, &games).draw_centered(
                &mut canvas,
                LOGICAL_WIDTH,
                LOGICAL_HEIGHT,
                (width / 2) as i32,
                (height * 86 / 100) as i32,
                &games,
                ink,
            );
        }
    } else {
        let title_scale = ((width - 24) / (card.name_mask.len().max(1) * 6)).clamp(1, 3) * 256;
        draw_mask_scaled_centered(
            &mut canvas,
            0,
            height * 73 / 100,
            width,
            &card.name_mask,
            ink,
            title_scale,
        );
        if detail && card.games.is_some() {
            draw_mask_scaled_centered(
                &mut canvas,
                0,
                height * 86 / 100,
                width,
                &card.games_mask,
                ink,
                ((width - 24) / (card.games_mask.len().max(1) * 6)).clamp(1, 2) * 256,
            );
        }
    }
    (0..height)
        .flat_map(|y| {
            canvas[y * LOGICAL_WIDTH..y * LOGICAL_WIDTH + width]
                .iter()
                .copied()
        })
        .collect()
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
    card: &PreparedCard,
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

fn framed_sample(
    card: &PreparedCard,
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
    } else if !inside_inset(x, y, width, height, 6) {
        // Discrete inks create the original luminous shoulder without an
        // RGB565 gradient. Compact faces retain the card's trim hue.
        mix_colour(
            source,
            trim,
            if x + y < (width + height) * 4 {
                210
            } else {
                160
            },
        )
    } else if !inside_inset(x, y, width, height, 8) {
        mix_colour(source, BACKGROUND, 205)
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

/// Native face preparation uses one destination-space silhouette for both
/// colour and alpha. Never rescale a baked black/rounded edge and mask it again.
pub(super) fn native_surface(
    card: &PreparedCard,
    width: usize,
    height: usize,
) -> (Vec<Rgb565Pixel>, Vec<u8>) {
    let fallback;
    let (source_w, source_h, rgb888, rgb565) = if let Some(rgb) = &card.rgb888 {
        (360, 504, Some(rgb.as_slice()), None)
    } else {
        fallback = if let Some(rgb) = &card.artwork {
            rgb.clone()
        } else {
            surface(card, 180, true, None, false)
        };
        (180, 252, None, Some(fallback.as_slice()))
    };
    // Decode before the area filter; retain fractional sRGB until final 565
    // quantisation. A fixed spatial threshold avoids temporal sparkle.
    let linear: [u32; 256] = std::array::from_fn(|i| {
        let s = i as f64 / 255.0;
        ((if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }) * 65535.0)
            .round() as u32
    });
    let to_srgb = |value: u64| -> u32 {
        let hi = linear.partition_point(|&v| u64::from(v) < value).min(255);
        if hi == 0 {
            return 0;
        }
        let lo = hi - 1;
        (lo * 256) as u32
            + ((value - u64::from(linear[lo])) * 256 / u64::from(linear[hi] - linear[lo])) as u32
    };
    let mut pixels = vec![Rgb565Pixel(0); width * height];
    let mut alpha = vec![0; width * height];
    let rx = (width * 5 / 100).max(4);
    let ry = (rx * height * 5 / (width * 7)).max(2);
    let bx = (width / 40).max(2);
    let by = (bx * height * 5).div_ceil(width * 7).max(1);
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0u64; 3];
            for sy in y * source_h / height..((y + 1) * source_h).div_ceil(height) {
                let wy =
                    ((y + 1) * source_h).min((sy + 1) * height) - (y * source_h).max(sy * height);
                for sx in x * source_w / width..((x + 1) * source_w).div_ceil(width) {
                    let wx =
                        ((x + 1) * source_w).min((sx + 1) * width) - (x * source_w).max(sx * width);
                    let channels = if let Some(rgb) = rgb888 {
                        let i = (sy * source_w + sx) * 3;
                        [rgb[i], rgb[i + 1], rgb[i + 2]]
                    } else {
                        let p = rgb565.unwrap()[sy * source_w + sx].0;
                        let (r, g, b) = ((p >> 11) as u8, ((p >> 5) & 63) as u8, (p & 31) as u8);
                        [
                            (r << 3) | (r >> 2),
                            (g << 2) | (g >> 4),
                            (b << 3) | (b >> 2),
                        ]
                    };
                    for c in 0..3 {
                        sum[c] += u64::from(linear[usize::from(channels[c])]) * (wx * wy) as u64;
                    }
                }
            }
            let channels = sum.map(|v| to_srgb(v / (source_w * source_h) as u64));
            let mut colour = quantise_native(channels, x, y);
            let outer = ellipse_coverage(x, y, width, height, rx, ry, 0, 0);
            let inner = ellipse_coverage(x, y, width, height, rx, ry, bx, by);
            let keyline = ellipse_coverage(x, y, width, height, rx, ry, bx + 1, by + 1);
            if let Some(weight) = (inner * 256).checked_div(outer) {
                let border = mix_colour(card.colour, CREAM, 48);
                colour = mix_colour(
                    border,
                    mix_colour(BACKGROUND, colour, (keyline * 256 / inner.max(1)) as usize),
                    weight as usize,
                );
            } else {
                colour = card.colour;
            }
            pixels[y * width + x] = Rgb565Pixel(colour);
            alpha[y * width + x] = (outer * 255 / 16) as u8;
        }
    }
    (pixels, alpha)
}

fn quantise_native(channels: [u32; 3], x: usize, y: usize) -> u16 {
    const BAYER: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let threshold = BAYER[y % 4][x % 4] * 16 + 8;
    let channel = |value: u32, levels: u32| {
        let scaled = value * levels;
        let denominator = 255 * 256;
        (scaled / denominator + u32::from((scaled % denominator) * 256 / denominator > threshold))
            .min(levels) as u16
    };
    (channel(channels[0], 31) << 11) | (channel(channels[1], 63) << 5) | channel(channels[2], 31)
}

#[allow(clippy::too_many_arguments)]
fn ellipse_coverage(
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    rx: usize,
    ry: usize,
    ix: usize,
    iy: usize,
) -> u32 {
    let rx = rx.saturating_sub(ix).max(1) as i64 * 8;
    let ry = ry.saturating_sub(iy).max(1) as i64 * 8;
    let mut count = 0;
    for sy in 0..4 {
        for sx in 0..4 {
            let px = (x * 8 + sx * 2 + 1) as i64;
            let py = (y * 8 + sy * 2 + 1) as i64;
            let dx = px.min(w as i64 * 8 - px) - ix as i64 * 8;
            let dy = py.min(h as i64 * 8 - py) - iy as i64 * 8;
            if dx < 0 || dy < 0 {
                continue;
            }
            let a = (rx - dx).max(0);
            let b = (ry - dy).max(0);
            count += u32::from(a * a * ry * ry + b * b * rx * rx <= rx * rx * ry * ry);
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn test_card(colour: u16) -> PreparedCard {
        PreparedCard {
            id: LauncherCardId::Handhelds,
            name: "HANDHELDS".into(),
            games: Some(126),
            colour,
            name_mask: text_mask("HANDHELDS"),
            games_mask: text_mask("126 GAMES"),
            artwork: None,
            rgb888: None,
        }
    }

    #[test]
    fn native_corners_share_symmetric_coverage_and_full_colour_edges() {
        let mut card = test_card(0xf800);
        card.rgb888 = Some(vec![128; 360 * 504 * 3]);
        for (w, h) in [(160, 112), (72, 200), (160, 134)] {
            let (pixels, alpha) = native_surface(&card, w, h);
            assert_eq!(alpha[0], 0);
            assert_eq!(alpha[w / 2], 255);
            assert_eq!(pixels[w / 2].0, mix_colour(card.colour, CREAM, 48));
            assert!(alpha.iter().any(|&a| a > 0 && a < 255));
            for y in 0..h {
                for x in 0..w {
                    assert_eq!(alpha[y * w + x], alpha[y * w + w - 1 - x]);
                    assert_eq!(alpha[y * w + x], alpha[(h - 1 - y) * w + x]);
                }
            }
        }
    }

    #[test]
    fn native_quantisation_preserves_sublevel_gradients_without_bias() {
        let mut previous = 0;
        for value in 0..=255 {
            let mut red = 0u32;
            let mut green = 0u32;
            for y in 0..4 {
                for x in 0..4 {
                    let pixel = quantise_native([value * 256; 3], x, y);
                    red += u32::from(pixel >> 11);
                    green += u32::from((pixel >> 5) & 63);
                }
            }
            assert!(red >= previous);
            assert!((i64::from(red * 255) - i64::from(value * 31 * 16)).abs() <= 255);
            assert!((i64::from(green * 255) - i64::from(value * 63 * 16)).abs() <= 255);
            previous = red;
        }
        // A dark change smaller than one 5-bit step survives spatial averaging.
        let sum = |value| {
            (0..16)
                .map(|i| u32::from(quantise_native([value; 3], i % 4, i / 4) >> 11))
                .sum::<u32>()
        };
        assert!(sum(12 * 256) < sum(14 * 256));
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
            let base = if detail {
                card.colour
            } else {
                mix_colour(rgb(12, 22, 30), card.colour, 44)
            };
            let trim = card.colour;
            for (xs, ys) in [(12..30, 14..22), (70..112, 230..235), (80..100, 0..8)] {
                for y in ys {
                    for x in xs.clone() {
                        assert_eq!(
                            face.pixels[y * 180 + x].0,
                            framed_surface(&card, base, trim, 180, 252, x, y)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn compact_artwork_and_keyline_keep_their_colours() {
        let source = rgb(220, 34, 78);
        let mut card = test_card(rgb(32, 112, 238));
        card.artwork = Some(vec![Rgb565Pixel(source); 180 * 252]);
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
            name: "SETTINGS".into(),
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
