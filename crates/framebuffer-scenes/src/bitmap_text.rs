// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Renderer-neutral bitmap fonts for custom RGB565 scenes.

use crate::Rgb565Pixel;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitmapGlyph {
    pub code_point: char,
    pub left: i32,
    pub top: i32,
    pub width: usize,
    pub height: usize,
    pub advance: i32,
    pub alpha: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitmapFont {
    pub ascent: i32,
    pub descent: i32,
    pub glyphs: Vec<BitmapGlyph>,
}

impl BitmapFont {
    #[must_use]
    pub fn glyph(&self, character: char) -> Option<&BitmapGlyph> {
        self.glyphs
            .binary_search_by_key(&character, |glyph| glyph.code_point)
            .ok()
            .and_then(|index| self.glyphs.get(index))
    }

    #[must_use]
    pub fn measure(&self, text: &str) -> usize {
        text.chars()
            .filter_map(|character| self.glyph(character))
            .map(|glyph| glyph.advance.max(0) as usize)
            .sum()
    }

    pub fn draw(
        &self,
        output: &mut [Rgb565Pixel],
        stride: usize,
        height: usize,
        x: i32,
        y: i32,
        text: &str,
        colour: u16,
    ) {
        let mut cursor = x;
        for character in text.chars() {
            let Some(glyph) = self.glyph(character) else {
                continue;
            };
            let glyph_x = cursor + glyph.left;
            let glyph_y = y + self.ascent - glyph.top;
            for row in 0..glyph.height {
                let destination_y = glyph_y + row as i32;
                if destination_y < 0 || destination_y >= height as i32 {
                    continue;
                }
                for column in 0..glyph.width {
                    if glyph.alpha[row * glyph.width + column] < 128 {
                        continue;
                    }
                    let destination_x = glyph_x + column as i32;
                    if destination_x >= 0 && destination_x < stride as i32 {
                        output[destination_y as usize * stride + destination_x as usize] =
                            Rgb565Pixel(colour);
                    }
                }
            }
            cursor += glyph.advance;
        }
    }

    pub fn draw_centered(
        &self,
        output: &mut [Rgb565Pixel],
        stride: usize,
        height: usize,
        centre_x: i32,
        y: i32,
        text: &str,
        colour: u16,
    ) {
        self.draw(
            output,
            stride,
            height,
            centre_x - self.measure(text) as i32 / 2,
            y,
            text,
            colour,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> BitmapFont {
        BitmapFont {
            ascent: 2,
            descent: 0,
            glyphs: vec![BitmapGlyph {
                code_point: 'A',
                left: 0,
                top: 2,
                width: 2,
                height: 2,
                advance: 3,
                alpha: vec![255, 0, 255, 255],
            }],
        }
    }

    #[test]
    fn measurement_and_clipped_draw_are_deterministic() {
        let font = font();
        assert_eq!(font.measure("AAA"), 9);
        let mut pixels = vec![Rgb565Pixel(0); 12];
        font.draw(&mut pixels, 4, 3, -1, 0, "AA", 0xffff);
        assert_eq!(pixels.iter().filter(|pixel| pixel.0 == 0xffff).count(), 4);
    }
}
