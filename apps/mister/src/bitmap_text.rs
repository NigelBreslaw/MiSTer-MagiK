// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::borrow::Cow;
use std::collections::HashMap;

use crate::framebuffer::mapped::Pixel;

struct ConsoleGlyph {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    advance: i32,
    data: Vec<u8>,
}

struct ConsoleGradientGlyph {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    advance: i32,
    row_colors: Vec<Pixel>,
    mask: Vec<bool>,
}

pub struct TextAlphaMask {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub alpha: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct TextGradient {
    top: u32,
    mid: u32,
    bottom: u32,
}

impl TextGradient {
    pub const fn new(top: Pixel, mid: Pixel, bottom: Pixel) -> Self {
        Self {
            top: top.0,
            mid: mid.0,
            bottom: bottom.0,
        }
    }

    fn color_at(self, row: usize, height: usize) -> Pixel {
        if height <= 1 {
            return Pixel(self.top);
        }
        let denom = (height - 1) as u32;
        let pos = (row as u32) * 2;
        let (from, to, t_num) = if pos <= denom {
            (self.top, self.mid, pos)
        } else {
            (self.mid, self.bottom, pos - denom)
        };
        Pixel(interpolate_rgb(from, to, t_num, denom))
    }
}

pub struct ConsoleFont {
    font: Option<swash::FontRef<'static>>,
    scale_context: swash::scale::ScaleContext,
    glyphs: HashMap<char, ConsoleGlyph>,
    gradient_glyphs: HashMap<(char, TextGradient), ConsoleGradientGlyph>,
    row_filter: ConsoleGlyphRowFilter,
    pixel_size: f32,
    units_per_em: f32,
    ascent: f32,
    descent: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleTypeface {
    PressStart2P,
    Yesterday10Perfect,
    Nocive15,
    Xerxes10,
    Xerxes10Perfect,
    Bacteria12,
    Bacteria12Half,
    Spleen6x12Small,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConsoleGlyphRowFilter {
    #[default]
    Native,
    PairwiseMax,
    PairwiseDominant,
}

impl ConsoleFont {
    pub fn new(pixel_size: f32) -> Self {
        Self::new_with_typeface(pixel_size, ConsoleTypeface::PressStart2P)
    }

    pub fn new_with_typeface(pixel_size: f32, typeface: ConsoleTypeface) -> Self {
        Self::new_with_typeface_and_row_filter(pixel_size, typeface, ConsoleGlyphRowFilter::Native)
    }

    pub fn new_with_typeface_and_row_filter(
        pixel_size: f32,
        typeface: ConsoleTypeface,
        row_filter: ConsoleGlyphRowFilter,
    ) -> Self {
        let bitmap = match typeface {
            ConsoleTypeface::Yesterday10Perfect => {
                assert_eq!(pixel_size, 32.0, "Yesterday 10 has one exact CRT240 size");
                Some(
                    mister_magik_fb::bitmap_font_resource::yesterday_10_crt240_console_bitmap_font(
                    )
                    .expect("valid pixel-perfect Yesterday 10 bitmap font resource"),
                )
            }
            ConsoleTypeface::Nocive15 => {
                assert_eq!(pixel_size, 16.0, "Nocive 15 has one exact renderer size");
                Some(
                    mister_magik_fb::bitmap_font_resource::nocive_15_console_bitmap_font()
                        .expect("valid Nocive 15 bitmap font resource"),
                )
            }
            ConsoleTypeface::Xerxes10 => {
                assert_eq!(pixel_size, 16.0, "Xerxes 10 has one exact renderer size");
                Some(
                    mister_magik_fb::bitmap_font_resource::xerxes_10_console_bitmap_font()
                        .expect("valid Xerxes 10 bitmap font resource"),
                )
            }
            ConsoleTypeface::Xerxes10Perfect => {
                assert_eq!(pixel_size, 32.0, "Xerxes 10 has one exact CRT240 size");
                Some(
                    mister_magik_fb::bitmap_font_resource::xerxes_10_crt240_console_bitmap_font()
                        .expect("valid pixel-perfect Xerxes 10 bitmap font resource"),
                )
            }
            ConsoleTypeface::Bacteria12 => {
                assert_eq!(pixel_size, 32.0, "Bacteria 12 has one exact CRT240 size");
                Some(
                    mister_magik_fb::bitmap_font_resource::bacteria_12_console_bitmap_font()
                        .expect("valid Bacteria 12 bitmap font resource"),
                )
            }
            ConsoleTypeface::Bacteria12Half => {
                assert_eq!(
                    pixel_size, 16.0,
                    "Bacteria 12 native resource is exactly 16px"
                );
                Some(
                    mister_magik_fb::bitmap_font_resource::bacteria_12_native_console_bitmap_font()
                        .expect("valid native-size Bacteria 12 bitmap font resource"),
                )
            }
            ConsoleTypeface::Spleen6x12Small => {
                assert_eq!(pixel_size, 12.0, "Spleen has one exact native size");
                Some(
                    mister_magik_fb::bitmap_font_resource::spleen_6x12_native_console_bitmap_font()
                        .expect("valid native Spleen bitmap font resource"),
                )
            }
            ConsoleTypeface::PressStart2P => None,
        };
        if let Some(bitmap) = bitmap {
            let glyphs = bitmap
                .glyphs
                .into_iter()
                .map(|glyph| {
                    (
                        glyph.code_point,
                        ConsoleGlyph {
                            left: glyph.left,
                            top: glyph.top,
                            width: glyph.width,
                            height: glyph.height,
                            advance: glyph.advance,
                            data: glyph.data,
                        },
                    )
                })
                .collect();
            return Self {
                font: None,
                scale_context: swash::scale::ScaleContext::new(),
                glyphs,
                gradient_glyphs: HashMap::new(),
                row_filter,
                pixel_size,
                units_per_em: 1.0,
                ascent: bitmap.ascent,
                descent: bitmap.descent,
            };
        }

        let (data, name): (&'static [u8], &str) = match typeface {
            ConsoleTypeface::PressStart2P => (
                include_bytes!("../ui/fonts/PressStart2P-Regular.ttf"),
                "PressStart2P-Regular.ttf",
            ),
            ConsoleTypeface::Nocive15
            | ConsoleTypeface::Yesterday10Perfect
            | ConsoleTypeface::Xerxes10
            | ConsoleTypeface::Xerxes10Perfect
            | ConsoleTypeface::Bacteria12
            | ConsoleTypeface::Bacteria12Half
            | ConsoleTypeface::Spleen6x12Small => {
                unreachable!()
            }
        };
        let font = swash::FontRef::from_index(data, 0).unwrap_or_else(|| panic!("{name}"));
        let metrics = font.metrics(&[]);
        let units_per_em = metrics.units_per_em as f32;
        let scale = pixel_size / units_per_em;
        Self {
            font: Some(font),
            scale_context: swash::scale::ScaleContext::new(),
            glyphs: HashMap::new(),
            gradient_glyphs: HashMap::new(),
            row_filter,
            pixel_size,
            units_per_em,
            ascent: metrics.ascent * scale,
            descent: metrics.descent * scale,
        }
    }

    pub fn clipped_text<'a>(&mut self, text: &'a str, max_width: usize) -> Cow<'a, str> {
        if self.text_width(text) <= max_width {
            return Cow::Borrowed(text);
        }
        let ellipsis = "...";
        let ellipsis_width = self.text_width(ellipsis);
        if ellipsis_width > max_width {
            let mut fitted = String::new();
            for ch in ellipsis.chars() {
                let mut candidate = fitted.clone();
                candidate.push(ch);
                if self.text_width(&candidate) > max_width {
                    break;
                }
                fitted = candidate;
            }
            return Cow::Owned(fitted);
        }
        let mut width = 0usize;
        let mut end = 0usize;
        for (index, ch) in text.char_indices() {
            let advance = self
                .glyph(ch)
                .map(|glyph| glyph.advance.max(0) as usize)
                .unwrap_or(0);
            if width.saturating_add(advance).saturating_add(ellipsis_width) > max_width {
                break;
            }
            width = width.saturating_add(advance);
            end = index + ch.len_utf8();
        }
        let mut clipped = String::with_capacity(end + ellipsis.len());
        clipped.push_str(&text[..end]);
        clipped.push_str(ellipsis);
        Cow::Owned(clipped)
    }

    pub fn centered_text_baseline(
        &mut self,
        text: &str,
        container_y: usize,
        container_height: usize,
    ) -> isize {
        let mut ink_bounds: Option<(isize, isize)> = None;
        for ch in text.chars() {
            let Some(glyph) = self.glyph(ch) else {
                continue;
            };
            if glyph.height == 0 {
                continue;
            }
            let top = -(glyph.top as isize);
            let bottom = top + glyph.height as isize;
            ink_bounds = Some(match ink_bounds {
                Some((min_top, max_bottom)) => (min_top.min(top), max_bottom.max(bottom)),
                None => (top, bottom),
            });
        }
        if let Some((ink_top, ink_bottom)) = ink_bounds {
            let ink_height = ink_bottom - ink_top;
            let centered_top =
                container_y as isize + (container_height as isize - ink_height).div_euclid(2);
            return centered_top - ink_top;
        }

        let line_height = (self.ascent - self.descent).max(1.0);
        let baseline = container_y as f32
            + ((container_height as f32 - line_height) * 0.5).max(0.0)
            + self.ascent;
        baseline.round() as isize
    }

    fn text_width(&mut self, text: &str) -> usize {
        let mut width = 0usize;
        for ch in text.chars() {
            if let Some(glyph) = self.glyph(ch) {
                width = width.saturating_add(glyph.advance.max(0) as usize);
            }
        }
        width
    }

    pub fn rasterize_alpha_mask(&mut self, text: &str) -> Option<TextAlphaMask> {
        let mut pen_x = 0i32;
        let mut bounds: Option<(i32, i32, i32, i32)> = None;
        for ch in text.chars() {
            let glyph = self.glyph(ch)?;
            if glyph.width > 0 && glyph.height > 0 {
                let left = pen_x + glyph.left;
                let top = -glyph.top;
                let right = left + glyph.width as i32;
                let bottom = top + glyph.height as i32;
                bounds = Some(match bounds {
                    Some((min_x, min_y, max_x, max_y)) => (
                        min_x.min(left),
                        min_y.min(top),
                        max_x.max(right),
                        max_y.max(bottom),
                    ),
                    None => (left, top, right, bottom),
                });
            }
            pen_x += glyph.advance;
        }
        let (min_x, min_y, max_x, max_y) = bounds?;
        let width = usize::try_from(max_x - min_x).ok()?;
        let height = usize::try_from(max_y - min_y).ok()?;
        let mut alpha = vec![0u8; width.saturating_mul(height)];
        pen_x = 0;
        for ch in text.chars() {
            let glyph = self.glyph(ch)?;
            let left = pen_x + glyph.left - min_x;
            let top = -glyph.top - min_y;
            for gy in 0..glyph.height {
                for gx in 0..glyph.width {
                    let value = glyph.data[gy * glyph.width + gx];
                    if value == 0 {
                        continue;
                    }
                    let x = usize::try_from(left + gx as i32).ok()?;
                    let y = usize::try_from(top + gy as i32).ok()?;
                    let destination = &mut alpha[y * width + x];
                    *destination = (*destination).max(value);
                }
            }
            pen_x += glyph.advance;
        }
        Some(TextAlphaMask {
            width,
            height,
            stride: width,
            alpha,
        })
    }

    fn glyph(&mut self, ch: char) -> Option<&ConsoleGlyph> {
        if !self.glyphs.contains_key(&ch) {
            let font = self.font?;
            let glyph_id = font.charmap().map(ch);
            let advance = if glyph_id == 0 {
                (self.pixel_size * 0.75) as i32
            } else {
                let scale = self.pixel_size / self.units_per_em;
                (font.glyph_metrics(&[]).advance_width(glyph_id) * scale) as i32
            };
            let glyph = if glyph_id == 0 || ch == ' ' {
                ConsoleGlyph {
                    left: 0,
                    top: 0,
                    width: 0,
                    height: 0,
                    advance,
                    data: Vec::new(),
                }
            } else {
                let mut scaler = self
                    .scale_context
                    .builder(font)
                    .size(self.pixel_size)
                    .build();
                let image = swash::scale::Render::new(&[swash::scale::Source::Outline])
                    .format(swash::zeno::Format::Alpha)
                    .render(&mut scaler, glyph_id)?;
                ConsoleGlyph {
                    left: image.placement.left,
                    top: image.placement.top,
                    width: image.placement.width as usize,
                    height: image.placement.height as usize,
                    advance,
                    data: image.data,
                }
            };
            self.glyphs.insert(ch, glyph);
        }
        self.glyphs.get(&ch)
    }

    fn gradient_glyph(
        &mut self,
        ch: char,
        gradient: TextGradient,
    ) -> Option<&ConsoleGradientGlyph> {
        let key = (ch, gradient);
        if !self.gradient_glyphs.contains_key(&key) {
            let glyph = self.glyph(ch)?;
            let left = glyph.left;
            let top = glyph.top;
            let width = glyph.width;
            let height = glyph.height;
            let advance = glyph.advance;
            let data = glyph.data.clone();
            let mut row_colors = Vec::with_capacity(height);
            let mut mask = Vec::with_capacity(data.len());
            for gy in 0..height {
                row_colors.push(gradient.color_at(gy, height));
                for gx in 0..width {
                    let alpha = data[gy * width + gx];
                    mask.push(alpha >= 128);
                }
            }
            self.gradient_glyphs.insert(
                key,
                ConsoleGradientGlyph {
                    left,
                    top,
                    width,
                    height,
                    advance,
                    row_colors,
                    mask,
                },
            );
        }
        self.gradient_glyphs.get(&key)
    }

    pub fn draw_text_clipped(
        &mut self,
        dst: &mut [Pixel],
        stride: usize,
        clip_w: usize,
        clip_y: usize,
        clip_h: usize,
        x: isize,
        baseline_y: isize,
        text: &str,
        color: Pixel,
    ) {
        let row_filter = self.row_filter;
        let mut pen_x = x;
        for ch in text.chars() {
            let Some(glyph) = self.glyph(ch) else {
                continue;
            };
            let gx0 = pen_x + glyph.left as isize;
            let gy0 = baseline_y - glyph.top as isize;
            draw_solid_glyph(
                dst, stride, clip_w, clip_y, clip_h, gx0, gy0, glyph, color, row_filter,
            );
            pen_x += glyph.advance as isize;
        }
    }

    pub fn draw_text_clipped_gradient(
        &mut self,
        dst: &mut [Pixel],
        stride: usize,
        clip_w: usize,
        clip_y: usize,
        clip_h: usize,
        x: isize,
        baseline_y: isize,
        text: &str,
        gradient: TextGradient,
    ) {
        let row_filter = self.row_filter;
        let mut pen_x = x;
        for ch in text.chars() {
            let Some(glyph) = self.gradient_glyph(ch, gradient) else {
                continue;
            };
            let gx0 = pen_x + glyph.left as isize;
            let gy0 = baseline_y - glyph.top as isize;
            draw_gradient_glyph(
                dst, stride, clip_w, clip_y, clip_h, gx0, gy0, glyph, row_filter,
            );
            pen_x += glyph.advance as isize;
        }
    }

    #[cfg(test)]
    fn gradient_glyph_cache_len(&self) -> usize {
        self.gradient_glyphs.len()
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_solid_glyph(
    dst: &mut [Pixel],
    stride: usize,
    clip_w: usize,
    clip_y: usize,
    clip_h: usize,
    gx0: isize,
    gy0: isize,
    glyph: &ConsoleGlyph,
    color: Pixel,
    row_filter: ConsoleGlyphRowFilter,
) {
    match row_filter {
        ConsoleGlyphRowFilter::Native => {
            for gy in 0..glyph.height {
                let dy = gy0 + gy as isize;
                if dy < clip_y as isize || dy >= (clip_y + clip_h) as isize {
                    continue;
                }
                for gx in 0..glyph.width {
                    let dx = gx0 + gx as isize;
                    if dx < 0 || dx >= clip_w as isize {
                        continue;
                    }
                    if glyph.data[gy * glyph.width + gx] >= 128 {
                        dst[dy as usize * stride + dx as usize] = color;
                    }
                }
            }
        }
        ConsoleGlyphRowFilter::PairwiseMax => {
            let glyph_y1 = gy0 + glyph.height as isize;
            let mut pair_y = gy0.div_euclid(2) * 2;
            while pair_y < glyph_y1 {
                for gx in 0..glyph.width {
                    let dx = gx0 + gx as isize;
                    if dx < 0 || dx >= clip_w as isize {
                        continue;
                    }
                    let alpha = [pair_y, pair_y + 1]
                        .into_iter()
                        .filter_map(|dy| {
                            let gy = dy - gy0;
                            (gy >= 0 && gy < glyph.height as isize)
                                .then(|| glyph.data[gy as usize * glyph.width + gx])
                        })
                        .max()
                        .unwrap_or(0);
                    if alpha < 128 {
                        continue;
                    }
                    for dy in [pair_y, pair_y + 1] {
                        if dy >= clip_y as isize && dy < (clip_y + clip_h) as isize {
                            dst[dy as usize * stride + dx as usize] = color;
                        }
                    }
                }
                pair_y += 2;
            }
        }
        ConsoleGlyphRowFilter::PairwiseDominant => {
            let glyph_y1 = gy0 + glyph.height as isize;
            let mut pair_y = gy0.div_euclid(2) * 2;
            while pair_y < glyph_y1 {
                let Some(source_gy) = dominant_pair_row(pair_y, gy0, glyph.height, |gy| {
                    glyph.data[gy * glyph.width..(gy + 1) * glyph.width]
                        .iter()
                        .map(|alpha| u32::from(*alpha))
                        .sum()
                }) else {
                    pair_y += 2;
                    continue;
                };
                for gx in 0..glyph.width {
                    let dx = gx0 + gx as isize;
                    if dx < 0 || dx >= clip_w as isize {
                        continue;
                    }
                    if glyph.data[source_gy * glyph.width + gx] < 128 {
                        continue;
                    }
                    for dy in [pair_y, pair_y + 1] {
                        if dy >= clip_y as isize && dy < (clip_y + clip_h) as isize {
                            dst[dy as usize * stride + dx as usize] = color;
                        }
                    }
                }
                pair_y += 2;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_gradient_glyph(
    dst: &mut [Pixel],
    stride: usize,
    clip_w: usize,
    clip_y: usize,
    clip_h: usize,
    gx0: isize,
    gy0: isize,
    glyph: &ConsoleGradientGlyph,
    row_filter: ConsoleGlyphRowFilter,
) {
    match row_filter {
        ConsoleGlyphRowFilter::Native => {
            for gy in 0..glyph.height {
                let dy = gy0 + gy as isize;
                if dy < clip_y as isize || dy >= (clip_y + clip_h) as isize {
                    continue;
                }
                for gx in 0..glyph.width {
                    let dx = gx0 + gx as isize;
                    if dx < 0 || dx >= clip_w as isize {
                        continue;
                    }
                    if glyph.mask[gy * glyph.width + gx] {
                        dst[dy as usize * stride + dx as usize] = glyph.row_colors[gy];
                    }
                }
            }
        }
        ConsoleGlyphRowFilter::PairwiseMax => {
            let glyph_y1 = gy0 + glyph.height as isize;
            let mut pair_y = gy0.div_euclid(2) * 2;
            while pair_y < glyph_y1 {
                for gx in 0..glyph.width {
                    let dx = gx0 + gx as isize;
                    if dx < 0 || dx >= clip_w as isize {
                        continue;
                    }
                    let color = [pair_y, pair_y + 1].into_iter().find_map(|dy| {
                        let gy = dy - gy0;
                        (gy >= 0 && gy < glyph.height as isize)
                            .then_some(gy as usize)
                            .filter(|gy| glyph.mask[*gy * glyph.width + gx])
                            .map(|gy| glyph.row_colors[gy])
                    });
                    let Some(color) = color else {
                        continue;
                    };
                    for dy in [pair_y, pair_y + 1] {
                        if dy >= clip_y as isize && dy < (clip_y + clip_h) as isize {
                            dst[dy as usize * stride + dx as usize] = color;
                        }
                    }
                }
                pair_y += 2;
            }
        }
        ConsoleGlyphRowFilter::PairwiseDominant => {
            let glyph_y1 = gy0 + glyph.height as isize;
            let mut pair_y = gy0.div_euclid(2) * 2;
            while pair_y < glyph_y1 {
                let Some(source_gy) = dominant_pair_row(pair_y, gy0, glyph.height, |gy| {
                    glyph.mask[gy * glyph.width..(gy + 1) * glyph.width]
                        .iter()
                        .filter(|ink| **ink)
                        .count() as u32
                }) else {
                    pair_y += 2;
                    continue;
                };
                for gx in 0..glyph.width {
                    let dx = gx0 + gx as isize;
                    if dx < 0 || dx >= clip_w as isize {
                        continue;
                    }
                    if !glyph.mask[source_gy * glyph.width + gx] {
                        continue;
                    }
                    let color = glyph.row_colors[source_gy];
                    for dy in [pair_y, pair_y + 1] {
                        if dy >= clip_y as isize && dy < (clip_y + clip_h) as isize {
                            dst[dy as usize * stride + dx as usize] = color;
                        }
                    }
                }
                pair_y += 2;
            }
        }
    }
}

fn dominant_pair_row(
    pair_y: isize,
    gy0: isize,
    glyph_height: usize,
    mut coverage: impl FnMut(usize) -> u32,
) -> Option<usize> {
    [pair_y, pair_y + 1]
        .into_iter()
        .filter_map(|dy| {
            let gy = dy - gy0;
            (gy >= 0 && gy < glyph_height as isize).then_some((dy, gy as usize))
        })
        .max_by_key(|(dy, gy)| (coverage(*gy), *dy))
        .map(|(_, gy)| gy)
}

fn interpolate_rgb(from: u32, to: u32, t_num: u32, t_den: u32) -> u32 {
    let t_den = t_den.max(1);
    let fr = (from >> 16) & 0xff;
    let fg = (from >> 8) & 0xff;
    let fb = from & 0xff;
    let tr = (to >> 16) & 0xff;
    let tg = (to >> 8) & 0xff;
    let tb = to & 0xff;
    let r = interpolate_channel(fr, tr, t_num, t_den);
    let g = interpolate_channel(fg, tg, t_num, t_den);
    let b = interpolate_channel(fb, tb, t_num, t_den);
    (r << 16) | (g << 8) | b
}

fn interpolate_channel(from: u32, to: u32, t_num: u32, t_den: u32) -> u32 {
    (from * (t_den - t_num) + to * t_num + t_den / 2) / t_den
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_GRADIENT: TextGradient =
        TextGradient::new(Pixel(0x00fff6ff), Pixel(0x00dbd1e6), Pixel(0x00938a9b));
    const ALT_TEST_GRADIENT: TextGradient =
        TextGradient::new(Pixel(0x00ffffff), Pixel(0x00c8bfd8), Pixel(0x00887f90));

    #[test]
    fn clipping_uses_measured_advances_and_fits_the_requested_width() {
        let mut font = ConsoleFont::new_with_typeface(16.0, ConsoleTypeface::PressStart2P);
        let clipped = font
            .clipped_text("Cadillacs and Dinosaurs", 80)
            .into_owned();

        assert!(clipped.ends_with("..."));
        assert!(font.text_width(&clipped) <= 80);
    }

    #[test]
    fn clipping_never_returns_an_ellipsis_wider_than_the_requested_width() {
        let mut font = ConsoleFont::new_with_typeface(16.0, ConsoleTypeface::PressStart2P);

        for max_width in 0..font.text_width("...") {
            let clipped = font.clipped_text("Arcade", max_width).into_owned();
            assert!(font.text_width(&clipped) <= max_width);
        }
    }

    #[test]
    fn gradient_glyph_cache_reuses_colored_glyphs_for_repeated_draws() {
        let mut font = ConsoleFont::new(16.0);
        let mut dst = vec![Pixel(0); 160 * 40];

        font.draw_text_clipped_gradient(&mut dst, 160, 160, 0, 40, 0, 24, "AA", TEST_GRADIENT);
        assert_eq!(font.gradient_glyph_cache_len(), 1);

        font.draw_text_clipped_gradient(&mut dst, 160, 160, 0, 40, 32, 24, "A", TEST_GRADIENT);
        assert_eq!(font.gradient_glyph_cache_len(), 1);
    }

    #[test]
    fn gradient_glyph_cache_is_keyed_by_character_and_palette() {
        let mut font = ConsoleFont::new(16.0);
        let mut dst = vec![Pixel(0); 160 * 40];

        font.draw_text_clipped_gradient(&mut dst, 160, 160, 0, 40, 0, 24, "ABBA", TEST_GRADIENT);
        assert_eq!(font.gradient_glyph_cache_len(), 2);

        font.draw_text_clipped_gradient(&mut dst, 160, 160, 0, 40, 0, 24, "AB", ALT_TEST_GRADIENT);
        assert_eq!(font.gradient_glyph_cache_len(), 4);
    }

    #[test]
    fn alpha_mask_tightly_contains_press_start_text() {
        let mut font = ConsoleFont::new_with_typeface(128.0, ConsoleTypeface::PressStart2P);
        let mask = font.rasterize_alpha_mask("MagiK").unwrap();
        let alpha_signature = mask.alpha.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
        });

        assert_eq!((mask.width, mask.height, mask.stride), (624, 128, 624));
        assert_eq!(alpha_signature, 0x2654_8c31_ed92_3025);
        assert!(mask.alpha.iter().any(|alpha| *alpha >= 128));
        assert!(mask.alpha.iter().any(|alpha| *alpha == 0));
    }

    #[test]
    fn press_start_font_reports_expected_metrics_and_line_pitch() {
        let cases = [(ConsoleTypeface::PressStart2P, 8.0, 1000, 0, 8)];
        for (typeface, pixel_size, expected_ascent, expected_descent, expected_pitch) in cases {
            let mut font = ConsoleFont::new_with_typeface(pixel_size, typeface);
            let metrics = font.font.expect("outline font").metrics(&[]);
            assert_eq!(metrics.ascent.round() as i32, expected_ascent);
            assert_eq!(metrics.descent.round() as i32, expected_descent);
            assert_eq!(metrics.leading, 0.0);
            let metric_height =
                (metrics.ascent - metrics.descent) * pixel_size / f32::from(metrics.units_per_em);
            let line_pitch = (pixel_size.max(metric_height) / 8.0).ceil() as i32 * 8;
            assert_eq!(line_pitch, expected_pitch);
            assert!(font.rasterize_alpha_mask("MagiK").is_some());
        }
    }

    #[test]
    fn centered_text_baseline_balances_crt_title_and_metadata_ink() {
        for (typeface, pixel_size, row_height, text) in [
            (ConsoleTypeface::Nocive15, 16.0, 32, "MagiK 1984"),
            (ConsoleTypeface::Nocive15, 16.0, 19, "MagiK 1984"),
            (ConsoleTypeface::Nocive15, 16.0, 39, "MagiK 1984"),
            (ConsoleTypeface::Xerxes10, 16.0, 32, "MagiK 1984"),
            (ConsoleTypeface::Xerxes10, 16.0, 19, "MagiK 1984"),
            (ConsoleTypeface::Xerxes10, 16.0, 39, "MagiK 1984"),
            (ConsoleTypeface::Yesterday10Perfect, 32.0, 32, "MagiK 1984"),
            (ConsoleTypeface::Xerxes10Perfect, 32.0, 32, "MagiK 1984"),
            (ConsoleTypeface::Bacteria12, 32.0, 32, "MagiK 1984"),
            (ConsoleTypeface::Bacteria12Half, 16.0, 32, "MagiK 1984"),
            (ConsoleTypeface::Spleen6x12Small, 12.0, 32, "MagiK 1984"),
            (ConsoleTypeface::PressStart2P, 8.0, 32, "128"),
            (ConsoleTypeface::PressStart2P, 8.0, 19, "128"),
            (ConsoleTypeface::PressStart2P, 8.0, 39, "128"),
        ] {
            let mut font = ConsoleFont::new_with_typeface(pixel_size, typeface);
            let width = 220;
            let background = Pixel(0x00112233);
            let foreground = Pixel(0x00ffffff);
            let mut pixels = vec![background; width * row_height];
            let baseline = font.centered_text_baseline(text, 0, row_height);
            font.draw_text_clipped(
                &mut pixels,
                width,
                width,
                0,
                row_height,
                0,
                baseline,
                text,
                foreground,
            );
            let ink_rows = pixels
                .chunks(width)
                .enumerate()
                .filter(|(_, row)| row.iter().any(|pixel| pixel.0 == foreground.0))
                .map(|(y, _)| y)
                .collect::<Vec<_>>();
            let top = *ink_rows.first().expect("text has top ink");
            let bottom = *ink_rows.last().expect("text has bottom ink");
            let top_padding = top;
            let bottom_padding = row_height - bottom - 1;
            assert!(top_padding.abs_diff(bottom_padding) <= 1);
        }
    }

    #[test]
    fn nocive_15_uses_only_the_precompiled_exact_size_resource() {
        let mut font = ConsoleFont::new_with_typeface(16.0, ConsoleTypeface::Nocive15);
        assert!(font.font.is_none());
        let mask = font.rasterize_alpha_mask("ARCADE").unwrap();
        assert_eq!(mask.height, 15);
        assert!(mask.alpha.iter().all(|alpha| matches!(alpha, 0 | 255)));
    }

    #[test]
    fn xerxes_10_uses_only_the_precompiled_exact_size_resource() {
        let mut font = ConsoleFont::new_with_typeface(16.0, ConsoleTypeface::Xerxes10);
        assert!(font.font.is_none());
        let mask = font.rasterize_alpha_mask("ARCADE").unwrap();
        assert_eq!(mask.height, 10);
        assert!(mask.alpha.iter().all(|alpha| matches!(alpha, 0 | 255)));
    }

    #[test]
    fn yesterday_10_perfect_uses_exact_2_by_2_crt240_cells() {
        let mut font = ConsoleFont::new_with_typeface(32.0, ConsoleTypeface::Yesterday10Perfect);
        assert!(font.font.is_none());
        let mask = font.rasterize_alpha_mask("ARCADE").unwrap();
        assert_eq!(mask.height, 20);
        assert_eq!(mask.width % 2, 0);
        assert!(mask.alpha.iter().all(|alpha| matches!(alpha, 0 | 255)));
        for rows in mask.alpha.chunks_exact(mask.stride * 2) {
            for x in (0..mask.width).step_by(2) {
                let cell = rows[x];
                assert_eq!(rows[x + 1], cell);
                assert_eq!(rows[mask.stride + x], cell);
                assert_eq!(rows[mask.stride + x + 1], cell);
            }
        }
    }

    #[test]
    fn xerxes_10_perfect_uses_exact_2_by_2_crt240_cells() {
        let mut font = ConsoleFont::new_with_typeface(32.0, ConsoleTypeface::Xerxes10Perfect);
        assert!(font.font.is_none());
        let mask = font.rasterize_alpha_mask("ARCADE").unwrap();
        assert_eq!(mask.height, 20);
        assert_eq!(mask.width % 2, 0);
        assert!(mask.alpha.iter().all(|alpha| matches!(alpha, 0 | 255)));
        for rows in mask.alpha.chunks_exact(mask.stride * 2) {
            for x in (0..mask.width).step_by(2) {
                let cell = rows[x];
                assert_eq!(rows[x + 1], cell);
                assert_eq!(rows[mask.stride + x], cell);
                assert_eq!(rows[mask.stride + x + 1], cell);
            }
        }
    }

    #[test]
    fn bacteria_12_uses_the_pixel_perfect_crt240_resource() {
        let mut font = ConsoleFont::new_with_typeface(32.0, ConsoleTypeface::Bacteria12);
        assert!(font.font.is_none());
        let mask = font.rasterize_alpha_mask("ARCADE").unwrap();
        assert_eq!(mask.height, 24);
        assert_eq!(mask.width % 2, 0);
        assert!(mask.alpha.iter().all(|alpha| matches!(alpha, 0 | 255)));
        for rows in mask.alpha.chunks_exact(mask.stride * 2) {
            for x in (0..mask.width).step_by(2) {
                let cell = rows[x];
                assert_eq!(rows[x + 1], cell);
                assert_eq!(rows[mask.stride + x], cell);
                assert_eq!(rows[mask.stride + x + 1], cell);
            }
        }
    }

    #[test]
    fn bacteria_12_half_uses_the_native_16px_resource() {
        let mut font = ConsoleFont::new_with_typeface(16.0, ConsoleTypeface::Bacteria12Half);
        assert!(font.font.is_none());
        let mask = font.rasterize_alpha_mask("ARCADE").unwrap();
        assert_eq!(mask.height, 12);
        assert!(mask.alpha.iter().all(|alpha| matches!(alpha, 0 | 255)));
    }

    #[test]
    fn terminus_small_uses_the_native_bitmap_resource() {
        let mut font = ConsoleFont::new_with_typeface(12.0, ConsoleTypeface::Spleen6x12Small);
        assert!(font.font.is_none());
        let mask = font.rasterize_alpha_mask("ARCADE").unwrap();
        assert_eq!(mask.height, 10);
        assert!(mask.alpha.iter().all(|alpha| matches!(alpha, 0 | 255)));
    }

    #[test]
    fn gradient_text_preserves_solid_text_footprint() {
        let mut solid_font = ConsoleFont::new(16.0);
        let mut gradient_font = ConsoleFont::new(16.0);
        let bg = Pixel(0x00112233);
        let mut solid = vec![bg; 220 * 48];
        let mut gradient = vec![bg; 220 * 48];

        solid_font.draw_text_clipped(
            &mut solid,
            220,
            220,
            0,
            48,
            0,
            30,
            "MAGIK",
            Pixel(0x00e8e0f0),
        );
        gradient_font.draw_text_clipped_gradient(
            &mut gradient,
            220,
            220,
            0,
            48,
            0,
            30,
            "MAGIK",
            TEST_GRADIENT,
        );

        let solid_mask = solid.iter().map(|px| px.0 != bg.0).collect::<Vec<_>>();
        let gradient_mask = gradient.iter().map(|px| px.0 != bg.0).collect::<Vec<_>>();
        assert_eq!(gradient_mask, solid_mask);
    }

    #[test]
    fn pairwise_max_locks_glyph_coverage_to_absolute_row_pairs() {
        let width = 220;
        let height = 40;
        let background = Pixel(0x00112233);
        let gradient = TextGradient::new(Pixel(0x00aaa5ff), Pixel(0x00aaa5ff), Pixel(0x00aaa5ff));
        let mut native = ConsoleFont::new_with_typeface_and_row_filter(
            16.0,
            ConsoleTypeface::Nocive15,
            ConsoleGlyphRowFilter::Native,
        );
        let mut filtered = ConsoleFont::new_with_typeface_and_row_filter(
            16.0,
            ConsoleTypeface::Nocive15,
            ConsoleGlyphRowFilter::PairwiseMax,
        );
        let mut native_pixels = vec![background; width * height];
        let mut filtered_pixels = vec![background; width * height];
        let baseline = native.centered_text_baseline("MAGIK", 0, height);
        native.draw_text_clipped_gradient(
            &mut native_pixels,
            width,
            width,
            0,
            height,
            8,
            baseline,
            "MAGIK",
            gradient,
        );
        filtered.draw_text_clipped_gradient(
            &mut filtered_pixels,
            width,
            width,
            0,
            height,
            8,
            baseline,
            "MAGIK",
            gradient,
        );

        assert_ne!(native_pixels, filtered_pixels);
        assert!(
            native_pixels
                .chunks_exact(width * 2)
                .any(|rows| { rows[..width] != rows[width..] })
        );
        for rows in filtered_pixels.chunks_exact(width * 2) {
            assert_eq!(rows[..width], rows[width..]);
        }

        let ink_columns = |pixels: &[Pixel]| {
            (0..width)
                .map(|x| (0..height).any(|y| pixels[y * width + x] != background))
                .collect::<Vec<_>>()
        };
        assert_eq!(ink_columns(&native_pixels), ink_columns(&filtered_pixels));
    }

    #[test]
    fn pairwise_dominant_repeats_the_more_covered_whole_glyph_row() {
        let width = 80;
        let height = 40;
        let background = Pixel(0x00112233);
        let color = Pixel(0x00e8e0f0);
        let mut native = ConsoleFont::new_with_typeface_and_row_filter(
            16.0,
            ConsoleTypeface::Nocive15,
            ConsoleGlyphRowFilter::Native,
        );
        let mut filtered = ConsoleFont::new_with_typeface_and_row_filter(
            16.0,
            ConsoleTypeface::Nocive15,
            ConsoleGlyphRowFilter::PairwiseDominant,
        );
        let mut native_pixels = vec![background; width * height];
        let mut filtered_pixels = vec![background; width * height];
        let baseline = native.centered_text_baseline("M", 0, height);
        native.draw_text_clipped(
            &mut native_pixels,
            width,
            width,
            0,
            height,
            8,
            baseline,
            "M",
            color,
        );
        filtered.draw_text_clipped(
            &mut filtered_pixels,
            width,
            width,
            0,
            height,
            8,
            baseline,
            "M",
            color,
        );

        let ink_count = |row: &[Pixel]| row.iter().filter(|pixel| **pixel != background).count();
        assert_ne!(native_pixels, filtered_pixels);
        for (native_rows, filtered_rows) in native_pixels
            .chunks_exact(width * 2)
            .zip(filtered_pixels.chunks_exact(width * 2))
        {
            assert_eq!(filtered_rows[..width], filtered_rows[width..]);
            assert_eq!(
                ink_count(&filtered_rows[..width]),
                ink_count(&native_rows[..width]).max(ink_count(&native_rows[width..]))
            );
        }

        let ink_columns = |pixels: &[Pixel]| {
            (0..width)
                .map(|x| (0..height).any(|y| pixels[y * width + x] != background))
                .collect::<Vec<_>>()
        };
        assert_eq!(ink_columns(&native_pixels), ink_columns(&filtered_pixels));
    }

    #[test]
    fn gradient_text_top_pixels_are_lighter_than_lower_pixels() {
        let mut font = ConsoleFont::new(16.0);
        let bg = Pixel(0x00112233);
        let stride = 80;
        let mut dst = vec![bg; stride * 48];

        font.draw_text_clipped_gradient(&mut dst, stride, stride, 0, 48, 0, 30, "H", TEST_GRADIENT);

        let rows = dst
            .chunks(stride)
            .enumerate()
            .filter(|(_, row)| row.iter().any(|px| px.0 != bg.0))
            .map(|(row, _)| row)
            .collect::<Vec<_>>();
        let top = *rows.first().expect("glyph has top pixels");
        let bottom = *rows.last().expect("glyph has bottom pixels");
        let top_px = dst[top * stride..(top + 1) * stride]
            .iter()
            .find(|px| px.0 != bg.0)
            .copied()
            .expect("top colored pixel");
        let bottom_px = dst[bottom * stride..(bottom + 1) * stride]
            .iter()
            .find(|px| px.0 != bg.0)
            .copied()
            .expect("bottom colored pixel");

        let top_luma = luma(top_px);
        let bottom_luma = luma(bottom_px);
        let delta = top_luma.saturating_sub(bottom_luma);

        assert!(top_luma > bottom_luma);
        assert!(
            bottom_luma >= 12_000,
            "bottom luma {bottom_luma} should stay readable"
        );
        assert!(
            (7_000..=15_000).contains(&delta),
            "gradient delta {delta} should stay subtle"
        );
    }

    fn luma(pixel: Pixel) -> u32 {
        let r = (pixel.0 >> 16) & 0xff;
        let g = (pixel.0 >> 8) & 0xff;
        let b = pixel.0 & 0xff;
        r * 30 + g * 59 + b * 11
    }
}
