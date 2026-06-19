use std::collections::HashMap;

use crate::fb::Pixel;

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

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) struct TextGradient {
    top: u32,
    mid: u32,
    bottom: u32,
}

impl TextGradient {
    pub(crate) const fn new(top: Pixel, mid: Pixel, bottom: Pixel) -> Self {
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

pub(crate) struct ConsoleFont {
    font: swash::FontRef<'static>,
    scale_context: swash::scale::ScaleContext,
    glyphs: HashMap<char, ConsoleGlyph>,
    gradient_glyphs: HashMap<(char, TextGradient), ConsoleGradientGlyph>,
    pixel_size: f32,
    units_per_em: f32,
}

impl ConsoleFont {
    pub(crate) fn new(pixel_size: f32) -> Self {
        let data = include_bytes!("../ui/fonts/PressStart2P-Regular.ttf");
        let font = swash::FontRef::from_index(data, 0).expect("PressStart2P-Regular.ttf");
        let units_per_em = font.metrics(&[]).units_per_em as f32;
        Self {
            font,
            scale_context: swash::scale::ScaleContext::new(),
            glyphs: HashMap::new(),
            gradient_glyphs: HashMap::new(),
            pixel_size,
            units_per_em,
        }
    }

    fn glyph(&mut self, ch: char) -> Option<&ConsoleGlyph> {
        if !self.glyphs.contains_key(&ch) {
            let glyph_id = self.font.charmap().map(ch);
            let advance = if glyph_id == 0 {
                (self.pixel_size * 0.75) as i32
            } else {
                let scale = self.pixel_size / self.units_per_em;
                (self.font.glyph_metrics(&[]).advance_width(glyph_id) * scale) as i32
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
                    .builder(self.font)
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

    pub(crate) fn draw_text_clipped(
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
        let mut pen_x = x;
        for ch in text.chars() {
            let Some(glyph) = self.glyph(ch) else {
                continue;
            };
            let gx0 = pen_x + glyph.left as isize;
            let gy0 = baseline_y - glyph.top as isize;
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
                    let alpha = glyph.data[gy * glyph.width + gx];
                    if alpha >= 128 {
                        dst[dy as usize * stride + dx as usize] = color;
                    }
                }
            }
            pen_x += glyph.advance as isize;
        }
    }

    pub(crate) fn draw_text_clipped_gradient(
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
        let mut pen_x = x;
        for ch in text.chars() {
            let Some(glyph) = self.gradient_glyph(ch, gradient) else {
                continue;
            };
            let gx0 = pen_x + glyph.left as isize;
            let gy0 = baseline_y - glyph.top as isize;
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
                    let src = gy * glyph.width + gx;
                    if glyph.mask[src] {
                        dst[dy as usize * stride + dx as usize] = glyph.row_colors[gy];
                    }
                }
            }
            pen_x += glyph.advance as isize;
        }
    }

    #[cfg(test)]
    fn gradient_glyph_cache_len(&self) -> usize {
        self.gradient_glyphs.len()
    }
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
