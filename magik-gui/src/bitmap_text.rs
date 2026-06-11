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

pub(crate) struct ConsoleFont {
    font: swash::FontRef<'static>,
    scale_context: swash::scale::ScaleContext,
    glyphs: HashMap<char, ConsoleGlyph>,
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
}
