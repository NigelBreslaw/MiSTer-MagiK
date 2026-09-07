// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Static text-and-colour launcher scene used by the Mini-MagiK visual probe.
//!
//! The scene deliberately has no Slint or runtime dependency. It renders a
//! packed RGB565 frame at the requested output size, using a 960x540 logical
//! design with nearest-neighbour letterboxing.

use crate::Rgb565Pixel;
use crate::launcher_navigation::{BrowseDirection, BrowseFrame};

pub const LOGICAL_WIDTH: usize = 960;
pub const LOGICAL_HEIGHT: usize = 540;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LauncherCard<'a> {
    pub name: &'a str,
    pub games: u32,
    pub colour: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LauncherData<'a> {
    pub cards: &'a [LauncherCard<'a>],
    pub selected: usize,
    pub library_games: u32,
    pub collections: u32,
    pub favourites: u32,
    pub clock: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LauncherScene {
    pub width: usize,
    pub height: usize,
}

impl LauncherScene {
    #[must_use]
    pub const fn new(width: usize, height: usize) -> Self {
        Self { width, height }
    }

    #[must_use]
    pub fn render(self, data: LauncherData<'_>) -> Vec<Rgb565Pixel> {
        self.render_browse(data, None)
    }

    #[must_use]
    pub fn render_browse(
        self,
        data: LauncherData<'_>,
        motion: Option<BrowseFrame>,
    ) -> Vec<Rgb565Pixel> {
        let mut logical = vec![Rgb565Pixel(BACKGROUND); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        render_logical(&mut logical, data, motion);
        scale_letterboxed(&logical, self.width, self.height)
    }

    #[must_use]
    pub fn prepare(self, data: LauncherData<'_>) -> PreparedLauncher {
        PreparedLauncher::new(self, data)
    }
}

/// Reusable launcher composition. All owned strings and working buffers are
/// created during preparation; `render_into` is allocation-free.
pub struct PreparedLauncher {
    scene: LauncherScene,
    chrome: Vec<Rgb565Pixel>,
    logical: Vec<Rgb565Pixel>,
    cards: Vec<PreparedCard>,
    ordinals: Vec<String>,
    collection_labels: Vec<String>,
}

struct PreparedCard {
    name: String,
    games: u32,
    games_label: String,
    colour: u16,
}

impl PreparedLauncher {
    fn new(scene: LauncherScene, data: LauncherData<'_>) -> Self {
        let cards: Vec<_> = data
            .cards
            .iter()
            .map(|card| PreparedCard {
                name: card.name.to_owned(),
                games: card.games,
                games_label: format_games(card.games),
                colour: card.colour,
            })
            .collect();
        let ordinals = (0..cards.len())
            .map(|index| ordinal(index, cards.len()))
            .collect();
        let collection_labels = (0..cards.len())
            .map(|index| format!("{:02} / {:02}", index + 1, cards.len()))
            .collect();
        let borrowed: Vec<_> = cards
            .iter()
            .map(|card| LauncherCard {
                name: &card.name,
                games: card.games,
                colour: card.colour,
            })
            .collect();
        let source = LauncherData {
            cards: &borrowed,
            selected: data.selected,
            library_games: data.library_games,
            collections: data.collections,
            favourites: data.favourites,
            clock: data.clock,
        };
        let mut chrome = vec![Rgb565Pixel(BACKGROUND); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        render_logical(&mut chrome, source, None);
        draw_rect(
            &mut chrome,
            296,
            CARD_TOP,
            638,
            REFLECTION_TOP + REFLECTION_HEIGHT - CARD_TOP,
            BACKGROUND,
        );
        draw_rect(&mut chrome, 880, 95, 54, 16, BACKGROUND);
        Self {
            scene,
            chrome,
            logical: vec![Rgb565Pixel(BACKGROUND); LOGICAL_WIDTH * LOGICAL_HEIGHT],
            cards,
            ordinals,
            collection_labels,
        }
    }

    pub fn render_into(&mut self, frame: BrowseFrame, output: &mut [Rgb565Pixel]) {
        self.logical.copy_from_slice(&self.chrome);
        if let Some(label) = self.collection_labels.get(frame.selected) {
            draw_text(&mut self.logical, 888, 101, label, MUTED, 1);
        }
        if self.cards.is_empty() {
            scale_into(&self.logical, self.scene.width, self.scene.height, output);
            return;
        }
        if frame.phase == crate::launcher_navigation::BrowsePhase::Settled {
            draw_cached_carousel(
                &mut self.logical,
                &self.cards,
                &self.ordinals,
                frame.selected,
            );
        } else {
            draw_cached_motion(&mut self.logical, &self.cards, &self.ordinals, frame);
        }
        scale_into(&self.logical, self.scene.width, self.scene.height, output);
    }
}

const BACKGROUND: u16 = rgb(0, 0, 0);
const CREAM: u16 = rgb(238, 232, 213);
const MUTED: u16 = rgb(143, 151, 150);
const RULE: u16 = rgb(48, 61, 63);
const WHITE: u16 = rgb(250, 247, 232);
const DARK_TEXT: u16 = rgb(10, 20, 24);
const CARD_TOP: usize = 135;
const CARD_BOTTOM: usize = 428;
const REFLECTION_TOP: usize = 433;
const REFLECTION_HEIGHT: usize = 32;

const fn rgb(red: u16, green: u16, blue: u16) -> u16 {
    ((red >> 3) << 11) | ((green >> 2) << 5) | (blue >> 3)
}

fn render_logical(pixels: &mut [Rgb565Pixel], data: LauncherData<'_>, motion: Option<BrowseFrame>) {
    draw_rect(pixels, 0, 0, LOGICAL_WIDTH, LOGICAL_HEIGHT, BACKGROUND);
    draw_text(pixels, 26, 20, "MISTER MAGIK", CREAM, 3);
    draw_text(pixels, 875, 22, data.clock, CREAM, 2);
    draw_line(pixels, 26, 76, 934, 76, RULE);

    draw_line(pixels, 265, 95, 265, 478, RULE);
    draw_text(pixels, 29, 101, "YOUR LIBRARY", MUTED, 1);
    draw_number(pixels, 28, 142, data.library_games, CREAM, 5);
    draw_text(pixels, 29, 205, "GAMES READY TO PLAY", MUTED, 1);
    draw_line(pixels, 28, 239, 240, 239, RULE);
    draw_number(pixels, 30, 265, data.collections, CREAM, 3);
    draw_number(pixels, 150, 265, data.favourites, CREAM, 3);
    draw_text(pixels, 30, 310, "COLLECTIONS", MUTED, 1);
    draw_text(pixels, 150, 310, "FAVOURITES", MUTED, 1);
    draw_line(pixels, 28, 340, 240, 340, RULE);
    for (index, colour) in [
        rgb(226, 52, 67),
        rgb(237, 193, 54),
        rgb(85, 170, 91),
        rgb(41, 145, 196),
    ]
    .iter()
    .enumerate()
    {
        draw_rect(pixels, 29 + index * 54, 436, 48, 7, *colour);
    }

    draw_text(pixels, 296, 101, "COLLECTIONS", MUTED, 1);
    let selected = if data.cards.is_empty() {
        0
    } else {
        data.selected % data.cards.len()
    };
    draw_text(
        pixels,
        888,
        101,
        &format!("{:02} / {:02}", selected + 1, data.cards.len()),
        MUTED,
        1,
    );
    if let Some(motion) = motion {
        draw_carousel_motion(pixels, data, motion);
    } else {
        draw_carousel(pixels, data);
    }
    draw_line(pixels, 26, 500, 934, 500, RULE);
    draw_text(pixels, 30, 516, "A  OPEN", CREAM, 1);
    draw_text(pixels, 130, 516, "B  BACK", CREAM, 1);
    draw_text(pixels, 586, 516, "←  →   BROWSE CARDS", CREAM, 1);
}

fn draw_carousel_motion(pixels: &mut [Rgb565Pixel], data: LauncherData<'_>, motion: BrowseFrame) {
    if data.cards.is_empty() || motion.direction.is_none() {
        draw_carousel(pixels, data);
        return;
    }
    draw_rect(
        pixels,
        296,
        CARD_TOP,
        638,
        REFLECTION_TOP + REFLECTION_HEIGHT - CARD_TOP,
        BACKGROUND,
    );
    let selected = motion.selected % data.cards.len();
    let progress = motion.progress_millis.min(motion.duration_millis.max(1)) as u32;
    let duration = motion.duration_millis.max(1);
    let right = motion.direction == Some(BrowseDirection::Right);
    let t = |value: i32| value * progress as i32 / duration as i32;
    let slot_x = |relative: isize| match relative {
        -3 => 187,
        -2 => 296,
        -1 => 405,
        0 => 514,
        1 => 702,
        2 => 813,
        _ => 934,
    };
    let slot_width = |relative: isize| match relative {
        0 => 188,
        1 => 111,
        2 => 121,
        _ => 109,
    };
    for relative in [-3_isize, -2, -1, 1, 2, 3, 0] {
        let index = (selected as isize + relative).rem_euclid(data.cards.len() as isize) as usize;
        let destination_relative = if right { relative - 1 } else { relative + 1 };
        let source_x = slot_x(relative);
        let destination_x = slot_x(destination_relative);
        let source_width = slot_width(relative);
        let destination_width = slot_width(destination_relative);
        let x = source_x + t(destination_x - source_x);
        let width = source_width + t(destination_width - source_width);
        if x >= 0 && width > 0 {
            draw_card(
                pixels,
                x as usize,
                width as usize,
                data.cards[index],
                relative == 0,
                index,
                data.cards.len(),
            );
        }
    }
}

fn draw_carousel(pixels: &mut [Rgb565Pixel], data: LauncherData<'_>) {
    if data.cards.is_empty() {
        return;
    }
    let selected = data.selected % data.cards.len();
    let positions = [296_usize, 405, 514, 702, 813];
    let widths = [109_usize, 109, 188, 111, 121];
    for slot in [4_usize, 3, 1, 0] {
        let relative = slot as isize - 2;
        let index = (selected as isize + relative).rem_euclid(data.cards.len() as isize) as usize;
        let card = data.cards[index];
        let selected_card = slot == 2;
        draw_card(
            pixels,
            positions[slot],
            widths[slot],
            card,
            selected_card,
            index,
            data.cards.len(),
        );
    }
    let card = data.cards[selected];
    draw_card(
        pixels,
        positions[2],
        widths[2],
        card,
        true,
        selected,
        data.cards.len(),
    );
}

fn draw_cached_carousel(
    pixels: &mut [Rgb565Pixel],
    cards: &[PreparedCard],
    ordinals: &[String],
    selected: usize,
) {
    let positions = [296_usize, 405, 514, 702, 813];
    let widths = [109_usize, 109, 188, 111, 121];
    for slot in [4_usize, 3, 1, 0] {
        let relative = slot as isize - 2;
        let index = (selected as isize + relative).rem_euclid(cards.len() as isize) as usize;
        draw_cached_card(
            pixels,
            positions[slot],
            widths[slot],
            &cards[index],
            false,
            &ordinals[index],
        );
    }
    draw_cached_card(
        pixels,
        positions[2],
        widths[2],
        &cards[selected % cards.len()],
        true,
        "",
    );
}

fn draw_cached_motion(
    pixels: &mut [Rgb565Pixel],
    cards: &[PreparedCard],
    ordinals: &[String],
    motion: BrowseFrame,
) {
    let selected = motion.selected % cards.len();
    let progress = motion.progress_millis.min(motion.duration_millis.max(1)) as i32;
    let duration = motion.duration_millis.max(1) as i32;
    let right = motion.direction == Some(BrowseDirection::Right);
    let t = |value: i32| value * progress / duration;
    let slot_x = |relative: isize| match relative {
        -3 => 187,
        -2 => 296,
        -1 => 405,
        0 => 514,
        1 => 702,
        2 => 813,
        _ => 934,
    };
    let slot_width = |relative: isize| match relative {
        0 => 188,
        1 => 111,
        2 => 121,
        _ => 109,
    };
    for relative in [-3_isize, -2, -1, 1, 2, 3, 0] {
        let index = (selected as isize + relative).rem_euclid(cards.len() as isize) as usize;
        let destination = if right { relative - 1 } else { relative + 1 };
        let x = slot_x(relative) + t(slot_x(destination) - slot_x(relative));
        let width = slot_width(relative) + t(slot_width(destination) - slot_width(relative));
        if x >= 0 && width > 0 {
            draw_cached_card(
                pixels,
                x as usize,
                width as usize,
                &cards[index],
                relative == 0,
                &ordinals[index],
            );
        }
    }
}

fn draw_cached_card(
    pixels: &mut [Rgb565Pixel],
    x: usize,
    width: usize,
    card: &PreparedCard,
    selected: bool,
    ordinal_label: &str,
) {
    let top = if selected { CARD_TOP } else { CARD_TOP + 8 };
    let bottom = CARD_BOTTOM;
    draw_rect(pixels, x, top, width, bottom - top, card.colour);
    let foreground = if is_light_card(card.colour) {
        DARK_TEXT
    } else if selected {
        WHITE
    } else {
        CREAM
    };
    if selected {
        draw_rect_outline(pixels, x, top, width, bottom - top, CREAM);
        if width > 8 && bottom - top > 8 {
            draw_rect_outline(pixels, x + 4, top + 4, width - 8, bottom - top - 8, CREAM);
        }
        draw_text_centered(pixels, x, 220, width, &card.name, foreground, 3);
        draw_text_centered(pixels, x, 385, width, &card.games_label, foreground, 1);
    } else {
        draw_text(pixels, x + 15, 151, ordinal_label, foreground, 1);
        draw_text_centered(pixels, x, 353, width, &card.name, foreground, 1);
    }
    draw_reflection(pixels, x, width, bottom);
}

fn draw_card(
    pixels: &mut [Rgb565Pixel],
    x: usize,
    width: usize,
    card: LauncherCard<'_>,
    selected: bool,
    index: usize,
    card_count: usize,
) {
    let top = if selected { CARD_TOP } else { CARD_TOP + 8 };
    let bottom = CARD_BOTTOM;
    draw_rect(pixels, x, top, width, bottom - top, card.colour);
    if selected {
        draw_rect_outline(pixels, x, top, width, bottom - top, CREAM);
        draw_rect_outline(pixels, x + 4, top + 4, width - 8, bottom - top - 8, CREAM);
        let foreground = if is_light_card(card.colour) {
            DARK_TEXT
        } else {
            WHITE
        };
        draw_text_centered(pixels, x, 220, width, card.name, foreground, 3);
        draw_text_centered(
            pixels,
            x,
            385,
            width,
            &format_games(card.games),
            foreground,
            1,
        );
    } else {
        let foreground = if is_light_card(card.colour) {
            DARK_TEXT
        } else {
            CREAM
        };
        draw_text(
            pixels,
            x + 15,
            151,
            &ordinal(index, card_count),
            foreground,
            1,
        );
        draw_text_centered(pixels, x, 353, width, card.name, foreground, 1);
    }
    draw_reflection(pixels, x, width, bottom);
}

fn is_light_card(colour: u16) -> bool {
    let red = ((colour >> 11) & 31) * 255 / 31;
    let green = ((colour >> 5) & 63) * 255 / 63;
    let blue = (colour & 31) * 255 / 31;
    red + green + blue > 480
}

fn format_games(games: u32) -> String {
    format!("{} GAMES", games)
}

fn ordinal(index: usize, count: usize) -> String {
    format!("{:02}", (index + 1).min(count))
}

fn draw_reflection(pixels: &mut [Rgb565Pixel], x: usize, width: usize, bottom: usize) {
    for row in 0..REFLECTION_HEIGHT {
        let source_y = bottom.saturating_sub(1 + row);
        for column in x..x + width {
            let target_y = REFLECTION_TOP + row;
            if target_y < LOGICAL_HEIGHT {
                let source = pixels[source_y * LOGICAL_WIDTH + column].0;
                pixels[target_y * LOGICAL_WIDTH + column] =
                    Rgb565Pixel(reflection_pixel(source, column, row));
            }
        }
    }
}

fn reflection_pixel(source: u16, x: usize, row: usize) -> u16 {
    // A quiet quadratic fade, quantized with a stationary 4x4 Bayer pattern.
    // Dither only the fractional channel level: never add noise to black.
    const BAYER: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let remaining = (REFLECTION_HEIGHT - 1 - row) as u32;
    let span = (REFLECTION_HEIGHT - 1) as u32;
    let alpha = 56 * remaining * remaining / (span * span);
    let threshold = BAYER[row % 4][x % 4] * 16 + 8;
    let fade = |channel: u16| -> u16 {
        let value = u32::from(channel) * alpha;
        (value / 256 + u32::from(value % 256 > threshold)) as u16
    };
    (fade((source >> 11) & 31) << 11) | (fade((source >> 5) & 63) << 5) | fade(source & 31)
}

fn scale_letterboxed(logical: &[Rgb565Pixel], width: usize, height: usize) -> Vec<Rgb565Pixel> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let mut output = vec![Rgb565Pixel(BACKGROUND); width * height];
    scale_into(logical, width, height, &mut output);
    output
}

fn scale_into(logical: &[Rgb565Pixel], width: usize, height: usize, output: &mut [Rgb565Pixel]) {
    if width == 0 || height == 0 || output.len() < width.saturating_mul(height) {
        return;
    }
    output.fill(Rgb565Pixel(BACKGROUND));
    let (scaled_width, scaled_height) =
        if width.saturating_mul(LOGICAL_HEIGHT) <= height.saturating_mul(LOGICAL_WIDTH) {
            (width, width * LOGICAL_HEIGHT / LOGICAL_WIDTH)
        } else {
            (height * LOGICAL_WIDTH / LOGICAL_HEIGHT, height)
        };
    if scaled_width == 0 || scaled_height == 0 {
        return;
    }
    let x_offset = (width - scaled_width) / 2;
    let y_offset = (height - scaled_height) / 2;
    for y in 0..scaled_height {
        let source_y = y * LOGICAL_HEIGHT / scaled_height;
        for x in 0..scaled_width {
            let source_x = x * LOGICAL_WIDTH / scaled_width;
            output[(y + y_offset) * width + x + x_offset] =
                logical[source_y * LOGICAL_WIDTH + source_x];
        }
    }
}

fn draw_line(pixels: &mut [Rgb565Pixel], x0: usize, y0: usize, x1: usize, y1: usize, colour: u16) {
    if x0 == x1 {
        for y in y0..=y1 {
            set_pixel(pixels, x0, y, colour);
        }
    } else {
        for x in x0..=x1 {
            set_pixel(pixels, x, y0, colour);
        }
    }
}

fn draw_rect(
    pixels: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    colour: u16,
) {
    for row in y..y.saturating_add(height).min(LOGICAL_HEIGHT) {
        for column in x..x.saturating_add(width).min(LOGICAL_WIDTH) {
            set_pixel(pixels, column, row, colour);
        }
    }
}

fn draw_rect_outline(
    pixels: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    colour: u16,
) {
    draw_line(pixels, x, y, x + width - 1, y, colour);
    draw_line(
        pixels,
        x,
        y + height - 1,
        x + width - 1,
        y + height - 1,
        colour,
    );
    for row in y..y + height {
        set_pixel(pixels, x, row, colour);
        set_pixel(pixels, x + width - 1, row, colour);
    }
}

fn draw_text_centered(
    pixels: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    width: usize,
    text: &str,
    colour: u16,
    scale: usize,
) {
    let text_width = text.chars().count() * 6 * scale;
    draw_text(
        pixels,
        x + width.saturating_sub(text_width) / 2,
        y,
        text,
        colour,
        scale,
    );
}

fn draw_number(
    pixels: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    value: u32,
    colour: u16,
    scale: usize,
) {
    draw_text(pixels, x, y, &value.to_string(), colour, scale);
}

fn draw_text(
    pixels: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    text: &str,
    colour: u16,
    scale: usize,
) {
    let mut cursor = x;
    for character in text.chars() {
        draw_glyph(pixels, cursor, y, character, colour, scale);
        cursor += 6 * scale;
    }
}

fn draw_glyph(
    pixels: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    character: char,
    colour: u16,
    scale: usize,
) {
    let glyph = glyph(character);
    for (row, bits) in glyph.iter().enumerate() {
        for column in 0..5 {
            if bits & (1 << (4 - column)) != 0 {
                draw_rect(
                    pixels,
                    x + column * scale,
                    y + row * scale,
                    scale,
                    scale,
                    colour,
                );
            }
        }
    }
}

fn glyph(character: char) -> [u8; 7] {
    match character.to_ascii_uppercase() {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [15, 16, 16, 16, 16, 16, 15],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [15, 16, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [31, 4, 4, 4, 4, 4, 31],
        'J' => [7, 2, 2, 2, 2, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 17, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 27, 17],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        ':' => [0, 6, 6, 0, 6, 6, 0],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        '←' => [4, 2, 31, 2, 4, 0, 0],
        '→' => [4, 8, 31, 8, 4, 0, 0],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '.' => [0, 0, 0, 0, 0, 6, 6],
        _ => [0, 0, 0, 0, 0, 0, 0],
    }
}

fn set_pixel(pixels: &mut [Rgb565Pixel], x: usize, y: usize, colour: u16) {
    if x < LOGICAL_WIDTH && y < LOGICAL_HEIGHT {
        pixels[y * LOGICAL_WIDTH + x] = Rgb565Pixel(colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARDS: [LauncherCard<'static>; 5] = [
        LauncherCard {
            name: "ARCADE",
            games: 1752,
            colour: rgb(142, 27, 48),
        },
        LauncherCard {
            name: "SNK",
            games: 324,
            colour: rgb(30, 75, 125),
        },
        LauncherCard {
            name: "CONSOLES",
            games: 842,
            colour: rgb(199, 190, 167),
        },
        LauncherCard {
            name: "HANDHELDS",
            games: 126,
            colour: rgb(45, 90, 150),
        },
        LauncherCard {
            name: "COMPUTERS",
            games: 86,
            colour: rgb(190, 34, 55),
        },
    ];

    fn data() -> LauncherData<'static> {
        LauncherData {
            cards: &CARDS,
            selected: 0,
            library_games: 6842,
            collections: 18,
            favourites: 126,
            clock: "21:37",
        }
    }

    #[test]
    fn simplified_chrome_leaves_removed_copy_regions_pure_black() {
        let frame = LauncherScene::new(960, 540).render(data());
        assert_eq!(BACKGROUND, 0);
        for (left, top, right, bottom) in [
            (26, 53, 934, 60),
            (29, 366, 240, 407),
            (29, 459, 240, 466),
            (846, 516, 934, 523),
        ] {
            for y in top..bottom {
                assert!(
                    frame[y * 960 + left..y * 960 + right]
                        .iter()
                        .all(|pixel| pixel.0 == 0)
                );
            }
        }
    }

    #[test]
    fn output_is_deterministic_and_packed() {
        let scene = LauncherScene::new(960, 540);
        assert_eq!(scene.render(data()), scene.render(data()));
        assert_eq!(scene.render(data()).len(), 960 * 540);
    }

    #[test]
    fn fits_requested_sizes_with_exact_letterbox() {
        let frame = LauncherScene::new(640, 480).render(data());
        assert_eq!(frame.len(), 640 * 480);
        assert!(frame[..640 * 60].iter().all(|pixel| pixel.0 == BACKGROUND));
        assert!(frame[420 * 640..].iter().all(|pixel| pixel.0 == BACKGROUND));
        assert!(
            frame[60 * 640..420 * 640]
                .iter()
                .any(|pixel| pixel.0 != BACKGROUND)
        );
        assert_eq!(LauncherScene::new(1, 1).render(data()).len(), 1);
        assert!(LauncherScene::new(0, 0).render(data()).is_empty());
    }

    #[test]
    fn reflection_reverses_rows_and_fades_to_background() {
        let mut pixels = vec![Rgb565Pixel(BACKGROUND); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        for row in 0..REFLECTION_HEIGHT {
            pixels[(CARD_BOTTOM - 1 - row) * LOGICAL_WIDTH + 520] = Rgb565Pixel(if row % 2 == 0 {
                rgb(240, 40, 80)
            } else {
                rgb(20, 180, 220)
            });
        }
        draw_reflection(&mut pixels, 520, 1, CARD_BOTTOM);
        assert_eq!(
            pixels[REFLECTION_TOP * LOGICAL_WIDTH + 520].0,
            reflection_pixel(rgb(240, 40, 80), 520, 0)
        );
        assert_ne!(
            pixels[(REFLECTION_TOP + 1) * LOGICAL_WIDTH + 520].0,
            pixels[REFLECTION_TOP * LOGICAL_WIDTH + 520].0
        );
        assert_ne!(
            pixels[(REFLECTION_TOP + 11) * LOGICAL_WIDTH + 520].0,
            pixels[(REFLECTION_TOP + 1) * LOGICAL_WIDTH + 520].0
        );
        assert_eq!(
            pixels[(REFLECTION_TOP + REFLECTION_HEIGHT - 1) * LOGICAL_WIDTH + 520].0,
            BACKGROUND
        );
    }

    #[test]
    fn reflection_is_dim_dithered_and_preserves_black() {
        for row in 0..REFLECTION_HEIGHT {
            for x in 0..4 {
                assert_eq!(reflection_pixel(0, x, row), 0);
                let pixel = reflection_pixel(0xffff, x, row);
                assert!((pixel >> 11) <= 7);
                assert!(((pixel >> 5) & 63) <= 14);
                assert!((pixel & 31) <= 7);
            }
        }
        assert_ne!(
            reflection_pixel(0xffff, 0, 1),
            reflection_pixel(0xffff, 1, 1)
        );
    }

    #[test]
    fn cards_are_clipped_to_carousel_region() {
        let mut pixels = vec![Rgb565Pixel(BACKGROUND); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        draw_carousel(&mut pixels, data());
        for row in CARD_TOP..REFLECTION_TOP + REFLECTION_HEIGHT {
            assert_eq!(pixels[row * LOGICAL_WIDTH + 295].0, BACKGROUND);
            assert_eq!(pixels[row * LOGICAL_WIDTH + 934].0, BACKGROUND);
        }
    }

    #[test]
    fn prepared_resting_frame_matches_cold_render() {
        let scene = LauncherScene::new(960, 540);
        let mut prepared = scene.prepare(data());
        let mut output = vec![Rgb565Pixel(0); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        let frame = BrowseFrame {
            selected: 0,
            target: 0,
            phase: crate::launcher_navigation::BrowsePhase::Settled,
            direction: None,
            progress_millis: 0,
            duration_millis: 180,
        };
        prepared.render_into(frame, &mut output);
        assert_eq!(output, scene.render(data()));
    }

    #[test]
    fn prepared_motion_changes_continuously_and_keeps_fixed_duration() {
        let scene = LauncherScene::new(960, 540);
        let mut prepared = scene.prepare(data());
        let mut start = vec![Rgb565Pixel(0); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        let mut middle = vec![Rgb565Pixel(0); LOGICAL_WIDTH * LOGICAL_HEIGHT];
        let frame = |progress_millis| BrowseFrame {
            selected: 0,
            target: 1,
            phase: crate::launcher_navigation::BrowsePhase::Sliding,
            direction: Some(BrowseDirection::Right),
            progress_millis,
            duration_millis: 180,
        };
        prepared.render_into(frame(0), &mut start);
        prepared.render_into(frame(90), &mut middle);
        assert_ne!(start, middle);
        assert_eq!(frame(90).duration_millis, 180);
    }
}
