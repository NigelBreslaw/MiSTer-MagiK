// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#![cfg_attr(not(any(feature = "ui", feature = "ui-preview")), allow(dead_code))]

//! Deterministic monochrome bitmap fonts for the Slint software renderer.

const MAGIC: &[u8; 8] = b"MAGIKBMF";
const VERSION: u16 = 1;
const HEADER_LEN: usize = 48;
const GLYPH_RECORD_LEN: usize = 24;
const MAX_RESOURCE_LEN: usize = 1024 * 1024;
const MAX_FAMILY_LEN: usize = 128;
const MAX_GLYPHS: usize = 2048;
const MAX_GLYPH_DIMENSION: usize = 256;

const YESTERDAY_10_RESOURCE: &[u8] =
    include_bytes!("../../../private/magik-assets/fonts/yesterday-10/yesterday10-16px.mmbf");
const YESTERDAY_10_CRT240_RESOURCE: &[u8] =
    include_bytes!("../../../private/magik-assets/fonts/yesterday-10/yesterday10-32px.mmbf");
const XERXES_10_RESOURCE: &[u8] =
    include_bytes!("../../../private/magik-assets/fonts/xerxes-10/xerxes10-16px.mmbf");
const XERXES_10_CRT240_RESOURCE: &[u8] =
    include_bytes!("../../../private/magik-assets/fonts/xerxes-10/xerxes10-32px.mmbf");
const NOCIVE_15_RESOURCE: &[u8] =
    include_bytes!("../../../private/magik-assets/fonts/nocive-15/nocive15-16px.mmbf");
const BACTERIA_12_RESOURCE: &[u8] =
    include_bytes!("../../../private/magik-assets/fonts/bacteria-12/bacteria12-32px.mmbf");
const BACTERIA_12_NATIVE_RESOURCE: &[u8] =
    include_bytes!("../../../private/magik-assets/fonts/bacteria-12/bacteria12-16px.mmbf");
const JERSEY_15_RESOURCE: &[u8] = include_bytes!("../assets/fonts/jersey15-27px.mmbf");
const JERSEY_25_RESOURCE: &[u8] = include_bytes!("../assets/fonts/jersey25-41px.mmbf");
#[cfg(not(feature = "asset-tools"))]
const TERMINUS_8X14_NATIVE_RESOURCE: &[u8] =
    include_bytes!("../assets/fonts/terminus-8x14/terminus-8x14-normal-1x.mmbf");
#[cfg(feature = "asset-tools")]
const TERMINUS_8X14_NATIVE_RESOURCE: &[u8] = &[];
#[cfg(not(feature = "asset-tools"))]
const TERMINUS_8X14_NORMAL_RESOURCE: &[u8] =
    include_bytes!("../assets/fonts/terminus-8x14/terminus-8x14-normal-2x.mmbf");
#[cfg(feature = "asset-tools")]
const TERMINUS_8X14_NORMAL_RESOURCE: &[u8] = &[];
#[cfg(not(feature = "asset-tools"))]
const TERMINUS_8X14_BOLD_RESOURCE: &[u8] =
    include_bytes!("../assets/fonts/terminus-8x14/terminus-8x14-bold-2x.mmbf");
#[cfg(feature = "asset-tools")]
const TERMINUS_8X14_BOLD_RESOURCE: &[u8] = &[];
#[cfg(not(feature = "asset-tools"))]
const SPLEEN_5X8_NATIVE_RESOURCE: &[u8] =
    include_bytes!("../assets/fonts/spleen/spleen-5x8-1x.mmbf");
#[cfg(feature = "asset-tools")]
const SPLEEN_5X8_NATIVE_RESOURCE: &[u8] = &[];
#[cfg(not(feature = "asset-tools"))]
const SPLEEN_5X8_DOUBLED_RESOURCE: &[u8] =
    include_bytes!("../assets/fonts/spleen/spleen-5x8-2x.mmbf");
#[cfg(feature = "asset-tools")]
const SPLEEN_5X8_DOUBLED_RESOURCE: &[u8] = &[];
#[cfg(not(feature = "asset-tools"))]
const SPLEEN_6X12_NATIVE_RESOURCE: &[u8] =
    include_bytes!("../assets/fonts/spleen/spleen-6x12-1x.mmbf");
#[cfg(feature = "asset-tools")]
const SPLEEN_6X12_NATIVE_RESOURCE: &[u8] = &[];
#[cfg(not(feature = "asset-tools"))]
const SPLEEN_6X12_DOUBLED_RESOURCE: &[u8] =
    include_bytes!("../assets/fonts/spleen/spleen-6x12-2x.mmbf");
#[cfg(feature = "asset-tools")]
const SPLEEN_6X12_DOUBLED_RESOURCE: &[u8] = &[];

#[derive(Clone, Debug, PartialEq)]
struct DecodedGlyph {
    code_point: char,
    x: i16,
    y: i16,
    width: i16,
    height: i16,
    x_advance: i16,
    stride: u16,
    packed: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
struct DecodedFont {
    family_name: String,
    pixel_size: i16,
    weight: u16,
    italic: bool,
    units_per_em: f32,
    ascent: f32,
    descent: f32,
    x_height: f32,
    cap_height: f32,
    glyphs: Vec<DecodedGlyph>,
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "truncated u16 field".to_string())?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_i16(bytes: &[u8], offset: usize) -> Result<i16, String> {
    Ok(read_u16(bytes, offset)? as i16)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "truncated u32 field".to_string())?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_f32(bytes: &[u8], offset: usize) -> Result<f32, String> {
    Ok(f32::from_bits(read_u32(bytes, offset)?))
}

fn decode_resource(bytes: &[u8]) -> Result<DecodedFont, String> {
    if bytes.len() < HEADER_LEN || bytes.len() > MAX_RESOURCE_LEN {
        return Err("bitmap font resource has an invalid length".to_string());
    }
    if bytes.get(..MAGIC.len()) != Some(MAGIC) {
        return Err("bitmap font resource has invalid magic".to_string());
    }
    if read_u16(bytes, 8)? != VERSION {
        return Err("bitmap font resource has an unsupported version".to_string());
    }

    let pixel_size = read_u16(bytes, 12)?;
    if pixel_size == 0 || pixel_size > MAX_GLYPH_DIMENSION as u16 {
        return Err("bitmap font resource has an invalid pixel size".to_string());
    }
    let weight = read_u16(bytes, 14)?;
    let italic = match bytes[16] {
        0 => false,
        1 => true,
        _ => return Err("bitmap font resource has an invalid italic flag".to_string()),
    };
    let glyph_count = usize::from(read_u16(bytes, 18)?);
    if glyph_count == 0 || glyph_count > MAX_GLYPHS {
        return Err("bitmap font resource has an invalid glyph count".to_string());
    }
    let family_len = usize::from(read_u16(bytes, 20)?);
    if family_len == 0 || family_len > MAX_FAMILY_LEN {
        return Err("bitmap font resource has an invalid family length".to_string());
    }
    let units_per_em = read_f32(bytes, 22)?;
    let ascent = read_f32(bytes, 26)?;
    let descent = read_f32(bytes, 30)?;
    let x_height = read_f32(bytes, 34)?;
    let cap_height = read_f32(bytes, 38)?;
    if ![units_per_em, ascent, descent, x_height, cap_height]
        .iter()
        .all(|metric| metric.is_finite())
        || units_per_em <= 0.0
    {
        return Err("bitmap font resource has invalid metrics".to_string());
    }
    let expected_crc = read_u32(bytes, 42)?;
    if crc32fast::hash(&bytes[HEADER_LEN..]) != expected_crc {
        return Err("bitmap font resource checksum mismatch".to_string());
    }

    let family_end = HEADER_LEN
        .checked_add(family_len)
        .ok_or_else(|| "bitmap font family length overflow".to_string())?;
    let directory_len = glyph_count
        .checked_mul(GLYPH_RECORD_LEN)
        .ok_or_else(|| "bitmap font directory length overflow".to_string())?;
    let directory_end = family_end
        .checked_add(directory_len)
        .ok_or_else(|| "bitmap font directory end overflow".to_string())?;
    if directory_end > bytes.len() {
        return Err("bitmap font resource has a truncated directory".to_string());
    }
    let family_name = std::str::from_utf8(&bytes[HEADER_LEN..family_end])
        .map_err(|_| "bitmap font family is not UTF-8".to_string())?
        .to_string();
    let bitmap_payload = &bytes[directory_end..];
    let mut glyphs = Vec::with_capacity(glyph_count);
    let mut previous_code_point = None;

    for index in 0..glyph_count {
        let record = family_end + index * GLYPH_RECORD_LEN;
        let raw_code_point = read_u32(bytes, record)?;
        if previous_code_point.is_some_and(|previous| raw_code_point <= previous) {
            return Err("bitmap font character map is not strictly sorted".to_string());
        }
        previous_code_point = Some(raw_code_point);
        let code_point = char::from_u32(raw_code_point)
            .ok_or_else(|| "bitmap font contains an invalid Unicode scalar".to_string())?;
        let x = read_i16(bytes, record + 4)?;
        let y = read_i16(bytes, record + 6)?;
        let width = read_i16(bytes, record + 8)?;
        let height = read_i16(bytes, record + 10)?;
        let x_advance = read_i16(bytes, record + 12)?;
        let stride = read_u16(bytes, record + 14)?;
        let data_offset = usize::try_from(read_u32(bytes, record + 16)?)
            .map_err(|_| "bitmap font glyph offset overflow".to_string())?;
        let data_len = usize::try_from(read_u32(bytes, record + 20)?)
            .map_err(|_| "bitmap font glyph length overflow".to_string())?;

        if width < 0
            || height < 0
            || usize::try_from(width).unwrap_or_default() > MAX_GLYPH_DIMENSION
            || usize::try_from(height).unwrap_or_default() > MAX_GLYPH_DIMENSION
        {
            return Err("bitmap font glyph dimensions are invalid".to_string());
        }
        let expected_stride = usize::try_from(width).unwrap_or_default().div_ceil(8);
        let expected_len = expected_stride
            .checked_mul(usize::try_from(height).unwrap_or_default())
            .ok_or_else(|| "bitmap font glyph size overflow".to_string())?;
        if usize::from(stride) != expected_stride || data_len != expected_len {
            return Err("bitmap font glyph storage dimensions do not match".to_string());
        }
        let data_end = data_offset
            .checked_add(data_len)
            .ok_or_else(|| "bitmap font glyph data overflow".to_string())?;
        let packed = bitmap_payload
            .get(data_offset..data_end)
            .ok_or_else(|| "bitmap font glyph data is out of range".to_string())?
            .to_vec();
        glyphs.push(DecodedGlyph {
            code_point,
            x,
            y,
            width,
            height,
            x_advance,
            stride,
            packed,
        });
    }

    Ok(DecodedFont {
        family_name,
        pixel_size: pixel_size as i16,
        weight,
        italic,
        units_per_em,
        ascent,
        descent,
        x_height,
        cap_height,
        glyphs,
    })
}

fn unpack_glyph(glyph: &DecodedGlyph) -> Vec<u8> {
    let width = usize::try_from(glyph.width).unwrap_or_default();
    let height = usize::try_from(glyph.height).unwrap_or_default();
    let stride = usize::from(glyph.stride);
    let mut alpha = vec![0; width * height];
    for y in 0..height {
        for x in 0..width {
            let mask = 0x80 >> (x & 7);
            if glyph.packed[y * stride + x / 8] & mask != 0 {
                alpha[y * width + x] = 255;
            }
        }
    }
    alpha
}

#[derive(Debug)]
pub struct ConsoleBitmapGlyph {
    pub code_point: char,
    pub left: i32,
    pub top: i32,
    pub width: usize,
    pub height: usize,
    pub advance: i32,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct ConsoleBitmapFont {
    pub ascent: f32,
    pub descent: f32,
    pub glyphs: Vec<ConsoleBitmapGlyph>,
}

pub fn nocive_15_console_bitmap_font() -> Result<ConsoleBitmapFont, String> {
    console_bitmap_font(NOCIVE_15_RESOURCE)
}

pub fn yesterday_10_console_bitmap_font() -> Result<ConsoleBitmapFont, String> {
    console_bitmap_font(YESTERDAY_10_RESOURCE)
}

pub fn yesterday_10_crt240_console_bitmap_font() -> Result<ConsoleBitmapFont, String> {
    console_bitmap_font(YESTERDAY_10_CRT240_RESOURCE)
}

pub fn xerxes_10_console_bitmap_font() -> Result<ConsoleBitmapFont, String> {
    console_bitmap_font(XERXES_10_RESOURCE)
}

pub fn xerxes_10_crt240_console_bitmap_font() -> Result<ConsoleBitmapFont, String> {
    console_bitmap_font(XERXES_10_CRT240_RESOURCE)
}

pub fn bacteria_12_console_bitmap_font() -> Result<ConsoleBitmapFont, String> {
    console_bitmap_font(BACTERIA_12_RESOURCE)
}

pub fn bacteria_12_native_console_bitmap_font() -> Result<ConsoleBitmapFont, String> {
    console_bitmap_font(BACTERIA_12_NATIVE_RESOURCE)
}

pub fn terminus_8x14_normal_console_bitmap_font() -> Result<ConsoleBitmapFont, String> {
    console_bitmap_font(TERMINUS_8X14_NORMAL_RESOURCE)
}

pub fn terminus_8x14_native_console_bitmap_font() -> Result<ConsoleBitmapFont, String> {
    console_bitmap_font(TERMINUS_8X14_NATIVE_RESOURCE)
}

pub fn terminus_8x14_bold_console_bitmap_font() -> Result<ConsoleBitmapFont, String> {
    console_bitmap_font(TERMINUS_8X14_BOLD_RESOURCE)
}

pub fn spleen_6x12_native_console_bitmap_font() -> Result<ConsoleBitmapFont, String> {
    console_bitmap_font(SPLEEN_6X12_NATIVE_RESOURCE)
}

fn console_bitmap_font(resource: &[u8]) -> Result<ConsoleBitmapFont, String> {
    let decoded = decode_resource(resource)?;
    let scale = f32::from(decoded.pixel_size) / decoded.units_per_em;
    let glyphs = decoded
        .glyphs
        .iter()
        .map(|glyph| {
            let height = i32::from(glyph.height);
            ConsoleBitmapGlyph {
                code_point: glyph.code_point,
                left: i32::from(glyph.x) / 64,
                top: i32::from(glyph.y) / 64 + height,
                width: usize::try_from(glyph.width).unwrap_or_default(),
                height: usize::try_from(glyph.height).unwrap_or_default(),
                advance: i32::from(glyph.x_advance) / 64,
                data: unpack_glyph(glyph),
            }
        })
        .collect();
    Ok(ConsoleBitmapFont {
        ascent: decoded.ascent * scale,
        descent: decoded.descent * scale,
        glyphs,
    })
}

#[cfg(any(feature = "ui", feature = "ui-preview"))]
fn leak_font(decoded: DecodedFont) -> &'static i_slint_core::graphics::BitmapFont {
    leak_font_family(vec![decoded])
}

#[cfg(any(feature = "ui", feature = "ui-preview"))]
fn leak_font_family(
    mut decoded_fonts: Vec<DecodedFont>,
) -> &'static i_slint_core::graphics::BitmapFont {
    use i_slint_core::graphics::{BitmapFont, BitmapGlyph, BitmapGlyphs, CharacterMapEntry};
    use i_slint_core::slice::Slice;

    decoded_fonts.sort_by_key(|font| font.pixel_size);
    let decoded = decoded_fonts
        .first()
        .expect("bitmap font family must contain at least one size");
    for candidate in &decoded_fonts[1..] {
        assert_eq!(candidate.family_name, decoded.family_name);
        assert_eq!(candidate.weight, decoded.weight);
        assert_eq!(candidate.italic, decoded.italic);
        assert_eq!(candidate.glyphs.len(), decoded.glyphs.len());
        assert!(
            candidate
                .glyphs
                .iter()
                .zip(&decoded.glyphs)
                .all(|(candidate, base)| candidate.code_point == base.code_point)
        );
    }
    let family_name = Box::leak(decoded.family_name.clone().into_bytes().into_boxed_slice());
    let character_map = decoded
        .glyphs
        .iter()
        .enumerate()
        .map(|(glyph_index, glyph)| CharacterMapEntry {
            code_point: glyph.code_point,
            glyph_index: glyph_index as u16,
        })
        .collect::<Vec<_>>();
    let (units_per_em, ascent, descent, x_height, cap_height, weight, italic) = (
        decoded.units_per_em,
        decoded.ascent,
        decoded.descent,
        decoded.x_height,
        decoded.cap_height,
        decoded.weight,
        decoded.italic,
    );
    let glyph_sets = Box::leak(
        decoded_fonts
            .into_iter()
            .map(|font| {
                let glyph_data = font
                    .glyphs
                    .iter()
                    .map(|glyph| {
                        let alpha = Box::leak(unpack_glyph(glyph).into_boxed_slice());
                        BitmapGlyph {
                            x: glyph.x,
                            y: glyph.y,
                            width: glyph.width,
                            height: glyph.height,
                            x_advance: glyph.x_advance,
                            data: Slice::from_slice(alpha),
                        }
                    })
                    .collect::<Vec<_>>();
                BitmapGlyphs {
                    pixel_size: font.pixel_size,
                    glyph_data: Slice::from_slice(Box::leak(glyph_data.into_boxed_slice())),
                }
            })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );

    Box::leak(Box::new(BitmapFont {
        family_name: Slice::from_slice(family_name),
        character_map: Slice::from_slice(Box::leak(character_map.into_boxed_slice())),
        units_per_em,
        ascent,
        descent,
        x_height,
        cap_height,
        glyphs: Slice::from_slice(glyph_sets),
        weight,
        italic,
        sdf: false,
    }))
}

#[cfg(any(feature = "ui", feature = "ui-preview"))]
pub fn register_bitmap_fonts(renderer: &slint::platform::software_renderer::SoftwareRenderer) {
    use i_slint_core::renderer::RendererSealed;
    use std::cell::Cell;
    use std::sync::OnceLock;

    static FONTS: OnceLock<[&'static i_slint_core::graphics::BitmapFont; 9]> = OnceLock::new();
    thread_local! {
        static REGISTERED: Cell<bool> = const { Cell::new(false) };
    }

    let fonts = FONTS.get_or_init(|| {
        [
            leak_font(
                decode_resource(YESTERDAY_10_RESOURCE).expect("valid Yesterday 10 bitmap font"),
            ),
            leak_font(decode_resource(XERXES_10_RESOURCE).expect("valid Xerxes 10 bitmap font")),
            leak_font(decode_resource(NOCIVE_15_RESOURCE).expect("valid Nocive 15 bitmap font")),
            leak_font(
                decode_resource(BACTERIA_12_RESOURCE).expect("valid Bacteria 12 bitmap font"),
            ),
            leak_font(decode_resource(JERSEY_15_RESOURCE).expect("valid Jersey 15 bitmap font")),
            leak_font(decode_resource(JERSEY_25_RESOURCE).expect("valid Jersey 25 bitmap font")),
            leak_font_family(vec![
                decode_resource(TERMINUS_8X14_NATIVE_RESOURCE)
                    .expect("valid native Terminus bitmap font"),
                decode_resource(TERMINUS_8X14_NORMAL_RESOURCE)
                    .expect("valid doubled Terminus bitmap font"),
            ]),
            leak_font_family(vec![
                decode_resource(SPLEEN_5X8_NATIVE_RESOURCE)
                    .expect("valid native Spleen 5x8 bitmap font"),
                decode_resource(SPLEEN_5X8_DOUBLED_RESOURCE)
                    .expect("valid doubled Spleen 5x8 bitmap font"),
            ]),
            leak_font_family(vec![
                decode_resource(SPLEEN_6X12_NATIVE_RESOURCE)
                    .expect("valid native Spleen 6x12 bitmap font"),
                decode_resource(SPLEEN_6X12_DOUBLED_RESOURCE)
                    .expect("valid doubled Spleen 6x12 bitmap font"),
            ]),
        ]
    });
    REGISTERED.with(|registered| {
        if !registered.replace(true) {
            for font in fonts {
                renderer.register_bitmap_font(font);
            }
        }
    });
}

#[cfg(any(test, feature = "asset-tools"))]
#[derive(Clone, Copy)]
enum Coverage {
    FullCharmap,
}

#[cfg(any(test, feature = "asset-tools"))]
#[derive(Clone, Copy)]
struct GeneratorSpec {
    family: &'static str,
    pixel_size: u16,
    weight: u16,
    hint: bool,
    threshold: u8,
    coverage: Coverage,
}

#[cfg(any(test, feature = "asset-tools"))]
const YESTERDAY_10_SPEC: GeneratorSpec = GeneratorSpec {
    family: "Yesterday 10",
    pixel_size: 16,
    weight: 400,
    hint: false,
    threshold: 128,
    coverage: Coverage::FullCharmap,
};

#[cfg(any(test, feature = "asset-tools"))]
const YESTERDAY_10_CRT240_SPEC: GeneratorSpec = GeneratorSpec {
    family: "Yesterday 10",
    pixel_size: 32,
    weight: 400,
    hint: false,
    threshold: 128,
    coverage: Coverage::FullCharmap,
};

#[cfg(any(test, feature = "asset-tools"))]
const XERXES_10_SPEC: GeneratorSpec = GeneratorSpec {
    family: "Xerxes 10",
    pixel_size: 16,
    weight: 400,
    hint: false,
    threshold: 128,
    coverage: Coverage::FullCharmap,
};

#[cfg(any(test, feature = "asset-tools"))]
const XERXES_10_CRT240_SPEC: GeneratorSpec = GeneratorSpec {
    family: "Xerxes 10",
    pixel_size: 32,
    weight: 400,
    hint: false,
    threshold: 128,
    coverage: Coverage::FullCharmap,
};

#[cfg(any(test, feature = "asset-tools"))]
const NOCIVE_15_SPEC: GeneratorSpec = GeneratorSpec {
    family: "Nocive 15",
    pixel_size: 16,
    weight: 400,
    hint: false,
    threshold: 128,
    coverage: Coverage::FullCharmap,
};

#[cfg(any(test, feature = "asset-tools"))]
const BACTERIA_12_SPEC: GeneratorSpec = GeneratorSpec {
    family: "Bacteria 12",
    pixel_size: 32,
    weight: 400,
    hint: false,
    threshold: 128,
    coverage: Coverage::FullCharmap,
};

#[cfg(any(test, feature = "asset-tools"))]
const BACTERIA_12_NATIVE_SPEC: GeneratorSpec = GeneratorSpec {
    family: "Bacteria 12",
    pixel_size: 16,
    weight: 400,
    hint: false,
    threshold: 128,
    coverage: Coverage::FullCharmap,
};

#[cfg(any(test, feature = "asset-tools"))]
const JERSEY_25_SPEC: GeneratorSpec = GeneratorSpec {
    family: "Jersey 25",
    pixel_size: 41,
    weight: 400,
    hint: true,
    threshold: 128,
    coverage: Coverage::FullCharmap,
};

#[cfg(any(test, feature = "asset-tools"))]
const JERSEY_15_SPEC: GeneratorSpec = GeneratorSpec {
    family: "Jersey 15",
    // Jersey 15's 750/1350 cap-height ratio needs a 27px source raster for
    // exact 15px physical capitals.
    pixel_size: 27,
    weight: 400,
    hint: true,
    threshold: 128,
    coverage: Coverage::FullCharmap,
};

#[cfg(any(test, feature = "asset-tools"))]
fn generate_resource(font_bytes: &[u8], spec: GeneratorSpec) -> Result<Vec<u8>, String> {
    use std::collections::BTreeSet;
    use swash::FontRef;
    use swash::scale::{Render, ScaleContext, Source};
    use swash::zeno::Format;

    let font = FontRef::from_index(font_bytes, 0)
        .ok_or_else(|| "unable to parse source font".to_string())?;
    let charmap = font.charmap();
    let mut code_points = BTreeSet::new();
    debug_assert!(matches!(spec.coverage, Coverage::FullCharmap));
    charmap.enumerate(|code_point, glyph_id| {
        if glyph_id != 0 && char::from_u32(code_point).is_some() {
            code_points.insert(code_point);
        }
    });
    if code_points.len() > MAX_GLYPHS {
        return Err("source font contains too many glyphs".to_string());
    }

    let metrics = font.metrics(&[]);
    let glyph_metrics = font.glyph_metrics(&[]);
    let scale = f32::from(spec.pixel_size) / f32::from(metrics.units_per_em);
    let mut scale_context = ScaleContext::new();
    let mut glyphs = Vec::with_capacity(code_points.len());

    for code_point in code_points {
        let glyph_id = charmap.map(code_point);
        let advance = (glyph_metrics.advance_width(glyph_id) * scale * 64.0).round();
        let x_advance = i16::try_from(advance as i32)
            .map_err(|_| "glyph advance does not fit resource format".to_string())?;
        let mut scaler = scale_context
            .builder(font)
            .size(f32::from(spec.pixel_size))
            .hint(spec.hint)
            .build();
        let image = Render::new(&[Source::Outline])
            .format(Format::Alpha)
            .render(&mut scaler, glyph_id);
        let Some(image) = image else {
            glyphs.push(DecodedGlyph {
                code_point: char::from_u32(code_point).unwrap(),
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                x_advance,
                stride: 0,
                packed: Vec::new(),
            });
            continue;
        };
        let placement = image.placement;
        let width = usize::try_from(placement.width)
            .map_err(|_| "glyph width does not fit host address space".to_string())?;
        let height = usize::try_from(placement.height)
            .map_err(|_| "glyph height does not fit host address space".to_string())?;
        if width > MAX_GLYPH_DIMENSION || height > MAX_GLYPH_DIMENSION {
            return Err("rasterized glyph exceeds resource bounds".to_string());
        }
        let stride = (width + 7) / 8;
        let mut packed = vec![0; stride * height];
        for y in 0..height {
            for x in 0..width {
                if image.data[y * width + x] >= spec.threshold {
                    packed[y * stride + x / 8] |= 0x80 >> (x & 7);
                }
            }
        }
        glyphs.push(DecodedGlyph {
            code_point: char::from_u32(code_point).unwrap(),
            x: i16::try_from(placement.left * 64)
                .map_err(|_| "glyph x position does not fit resource format".to_string())?,
            y: i16::try_from(
                (placement.top
                    - i32::try_from(placement.height)
                        .map_err(|_| "glyph height does not fit resource format".to_string())?)
                    * 64,
            )
            .map_err(|_| "glyph y position does not fit resource format".to_string())?,
            width: placement.width as i16,
            height: placement.height as i16,
            x_advance,
            stride: stride as u16,
            packed,
        });
    }

    encode_resource(
        &DecodedFont {
            family_name: spec.family.to_string(),
            pixel_size: spec.pixel_size as i16,
            weight: spec.weight,
            italic: false,
            units_per_em: f32::from(metrics.units_per_em),
            ascent: metrics.ascent,
            // Swash reports descent as a positive distance below the baseline,
            // while Slint's BitmapFont contract requires a negative value.
            descent: -metrics.descent,
            x_height: metrics.x_height,
            cap_height: metrics.cap_height,
            glyphs,
        },
        spec.hint,
        spec.threshold,
    )
}

#[cfg(any(test, feature = "asset-tools"))]
fn encode_resource(font: &DecodedFont, hint: bool, threshold: u8) -> Result<Vec<u8>, String> {
    let family = font.family_name.as_bytes();
    let glyph_count = u16::try_from(font.glyphs.len())
        .map_err(|_| "too many glyphs for resource format".to_string())?;
    let mut payload = Vec::new();
    let mut directory = Vec::with_capacity(font.glyphs.len() * GLYPH_RECORD_LEN);
    for glyph in &font.glyphs {
        directory.extend_from_slice(&u32::from(glyph.code_point).to_le_bytes());
        directory.extend_from_slice(&glyph.x.to_le_bytes());
        directory.extend_from_slice(&glyph.y.to_le_bytes());
        directory.extend_from_slice(&glyph.width.to_le_bytes());
        directory.extend_from_slice(&glyph.height.to_le_bytes());
        directory.extend_from_slice(&glyph.x_advance.to_le_bytes());
        directory.extend_from_slice(&glyph.stride.to_le_bytes());
        directory.extend_from_slice(
            &u32::try_from(payload.len())
                .map_err(|_| "bitmap payload offset overflow".to_string())?
                .to_le_bytes(),
        );
        directory.extend_from_slice(
            &u32::try_from(glyph.packed.len())
                .map_err(|_| "bitmap payload length overflow".to_string())?
                .to_le_bytes(),
        );
        payload.extend_from_slice(&glyph.packed);
    }

    let mut body = Vec::with_capacity(family.len() + directory.len() + payload.len());
    body.extend_from_slice(family);
    body.extend_from_slice(&directory);
    body.extend_from_slice(&payload);
    let mut output = Vec::with_capacity(HEADER_LEN + body.len());
    output.extend_from_slice(MAGIC);
    output.extend_from_slice(&VERSION.to_le_bytes());
    output.extend_from_slice(&(u16::from(hint)).to_le_bytes());
    output.extend_from_slice(&(font.pixel_size as u16).to_le_bytes());
    output.extend_from_slice(&font.weight.to_le_bytes());
    output.push(u8::from(font.italic));
    output.push(threshold);
    output.extend_from_slice(&glyph_count.to_le_bytes());
    output.extend_from_slice(&(family.len() as u16).to_le_bytes());
    output.extend_from_slice(&font.units_per_em.to_bits().to_le_bytes());
    output.extend_from_slice(&font.ascent.to_bits().to_le_bytes());
    output.extend_from_slice(&font.descent.to_bits().to_le_bytes());
    output.extend_from_slice(&font.x_height.to_bits().to_le_bytes());
    output.extend_from_slice(&font.cap_height.to_bits().to_le_bytes());
    output.extend_from_slice(&crc32fast::hash(&body).to_le_bytes());
    output.extend_from_slice(&[0, 0]);
    debug_assert_eq!(output.len(), HEADER_LEN);
    output.extend_from_slice(&body);
    Ok(output)
}

#[cfg(any(test, feature = "asset-tools"))]
fn generate_bdf_resource(
    source: &str,
    family: &str,
    weight: u16,
    pixel_scale: usize,
    source_x_height: f32,
    source_cap_height: f32,
) -> Result<Vec<u8>, String> {
    #[derive(Default)]
    struct SourceGlyph {
        encoding: Option<u32>,
        advance: Option<i32>,
        bounds: Option<(usize, usize, i32, i32)>,
        bitmap: Vec<String>,
    }

    fn parse_property_i32(line: &str, name: &str) -> Result<Option<i32>, String> {
        let Some(value) = line.strip_prefix(name) else {
            return Ok(None);
        };
        value
            .trim()
            .parse::<i32>()
            .map(Some)
            .map_err(|_| format!("invalid BDF {name}"))
    }

    if pixel_scale == 0 {
        return Err("BDF pixel scale must be positive".to_string());
    }
    let mut font_bounds = None;
    let mut font_ascent = None;
    let mut font_descent = None;
    let mut source_glyph = None::<SourceGlyph>;
    let mut reading_bitmap = false;
    let mut source_glyphs = Vec::new();

    for line in source.lines().map(str::trim) {
        if let Some(glyph) = source_glyph.as_mut() {
            if line == "ENDCHAR" {
                source_glyphs.push(source_glyph.take().unwrap());
                reading_bitmap = false;
            } else if reading_bitmap {
                glyph.bitmap.push(line.to_string());
            } else if let Some(value) = parse_property_i32(line, "ENCODING ")? {
                glyph.encoding = u32::try_from(value).ok();
            } else if let Some(values) = line.strip_prefix("DWIDTH ") {
                let mut values = values.split_whitespace();
                glyph.advance = Some(
                    values
                        .next()
                        .ok_or_else(|| "missing BDF DWIDTH".to_string())?
                        .parse()
                        .map_err(|_| "invalid BDF DWIDTH".to_string())?,
                );
            } else if let Some(values) = line.strip_prefix("BBX ") {
                let values = values
                    .split_whitespace()
                    .map(str::parse::<i32>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| "invalid BDF BBX".to_string())?;
                if values.len() != 4 || values[0] < 0 || values[1] < 0 {
                    return Err("invalid BDF BBX".to_string());
                }
                glyph.bounds = Some((values[0] as usize, values[1] as usize, values[2], values[3]));
            } else if line == "BITMAP" {
                reading_bitmap = true;
            }
            continue;
        }

        if line.starts_with("STARTCHAR ") {
            source_glyph = Some(SourceGlyph::default());
        } else if let Some(values) = line.strip_prefix("FONTBOUNDINGBOX ") {
            let values = values
                .split_whitespace()
                .map(str::parse::<i32>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "invalid BDF FONTBOUNDINGBOX".to_string())?;
            if values.len() != 4 || values[0] <= 0 || values[1] <= 0 {
                return Err("invalid BDF FONTBOUNDINGBOX".to_string());
            }
            font_bounds = Some((values[0] as usize, values[1] as usize));
        } else if let Some(value) = parse_property_i32(line, "FONT_ASCENT ")? {
            font_ascent = Some(value);
        } else if let Some(value) = parse_property_i32(line, "FONT_DESCENT ")? {
            font_descent = Some(value);
        }
    }
    if source_glyph.is_some() {
        return Err("unterminated BDF glyph".to_string());
    }

    let (cell_width, cell_height) = font_bounds.ok_or_else(|| "missing BDF bounds".to_string())?;
    let ascent = font_ascent.ok_or_else(|| "missing BDF ascent".to_string())?;
    let descent = font_descent.ok_or_else(|| "missing BDF descent".to_string())?;
    let scaled_cell_height = cell_height
        .checked_mul(pixel_scale)
        .ok_or_else(|| "scaled BDF height overflow".to_string())?;
    if cell_width * pixel_scale > MAX_GLYPH_DIMENSION
        || scaled_cell_height > MAX_GLYPH_DIMENSION
        || source_glyphs.len() > MAX_GLYPHS
    {
        return Err("scaled BDF exceeds resource bounds".to_string());
    }

    let mut glyphs = Vec::with_capacity(source_glyphs.len());
    for source_glyph in source_glyphs {
        let Some(code_point) = source_glyph.encoding.and_then(char::from_u32) else {
            continue;
        };
        let advance = source_glyph
            .advance
            .ok_or_else(|| format!("missing BDF advance for U+{:04X}", u32::from(code_point)))?;
        let (source_width, source_height, source_x, source_y) = source_glyph
            .bounds
            .ok_or_else(|| format!("missing BDF bounds for U+{:04X}", u32::from(code_point)))?;
        if source_glyph.bitmap.len() != source_height {
            return Err(format!(
                "BDF bitmap height mismatch for U+{:04X}",
                u32::from(code_point)
            ));
        }
        let width = source_width
            .checked_mul(pixel_scale)
            .ok_or_else(|| "scaled BDF width overflow".to_string())?;
        let height = source_height
            .checked_mul(pixel_scale)
            .ok_or_else(|| "scaled BDF height overflow".to_string())?;
        let stride = width.div_ceil(8);
        let mut packed = vec![0; stride * height];
        for (source_row, row) in source_glyph.bitmap.iter().enumerate() {
            let row_bytes = (0..row.len())
                .step_by(2)
                .map(|offset| {
                    u8::from_str_radix(
                        row.get(offset..offset + 2)
                            .ok_or_else(|| "odd BDF bitmap row length".to_string())?,
                        16,
                    )
                    .map_err(|_| "invalid BDF bitmap row".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            if row_bytes.len() * 8 < source_width {
                return Err("short BDF bitmap row".to_string());
            }
            for source_x_bit in 0..source_width {
                if row_bytes[source_x_bit / 8] & (0x80 >> (source_x_bit & 7)) == 0 {
                    continue;
                }
                for y_repeat in 0..pixel_scale {
                    let y = source_row * pixel_scale + y_repeat;
                    for x_repeat in 0..pixel_scale {
                        let x = source_x_bit * pixel_scale + x_repeat;
                        packed[y * stride + x / 8] |= 0x80 >> (x & 7);
                    }
                }
            }
        }
        let scaled_26_6 = |value: i32| -> Result<i16, String> {
            i16::try_from(value * pixel_scale as i32 * 64)
                .map_err(|_| "scaled BDF metric exceeds resource format".to_string())
        };
        glyphs.push(DecodedGlyph {
            code_point,
            x: scaled_26_6(source_x)?,
            y: scaled_26_6(source_y)?,
            width: width as i16,
            height: height as i16,
            x_advance: scaled_26_6(advance)?,
            stride: stride as u16,
            packed,
        });
    }
    glyphs.sort_by_key(|glyph| glyph.code_point);
    glyphs.dedup_by_key(|glyph| glyph.code_point);

    let metric_scale = pixel_scale as f32;
    encode_resource(
        &DecodedFont {
            family_name: family.to_string(),
            pixel_size: scaled_cell_height as i16,
            weight,
            italic: false,
            units_per_em: scaled_cell_height as f32,
            ascent: ascent as f32 * metric_scale,
            descent: -(descent as f32 * metric_scale),
            x_height: source_x_height * metric_scale,
            cap_height: source_cap_height * metric_scale,
            glyphs,
        },
        false,
        128,
    )
}

#[cfg(feature = "asset-tools")]
pub fn generate_yesterday_10(font_bytes: &[u8]) -> Result<Vec<u8>, String> {
    generate_resource(font_bytes, YESTERDAY_10_SPEC)
}

#[cfg(feature = "asset-tools")]
pub fn generate_yesterday_10_crt240(font_bytes: &[u8]) -> Result<Vec<u8>, String> {
    generate_resource(font_bytes, YESTERDAY_10_CRT240_SPEC)
}

#[cfg(feature = "asset-tools")]
pub fn generate_xerxes_10(font_bytes: &[u8]) -> Result<Vec<u8>, String> {
    generate_resource(font_bytes, XERXES_10_SPEC)
}

#[cfg(feature = "asset-tools")]
pub fn generate_xerxes_10_crt240(font_bytes: &[u8]) -> Result<Vec<u8>, String> {
    generate_resource(font_bytes, XERXES_10_CRT240_SPEC)
}

#[cfg(feature = "asset-tools")]
pub fn generate_nocive_15(font_bytes: &[u8]) -> Result<Vec<u8>, String> {
    generate_resource(font_bytes, NOCIVE_15_SPEC)
}

#[cfg(feature = "asset-tools")]
pub fn generate_bacteria_12(font_bytes: &[u8]) -> Result<Vec<u8>, String> {
    generate_resource(font_bytes, BACTERIA_12_SPEC)
}

#[cfg(feature = "asset-tools")]
pub fn generate_bacteria_12_native(font_bytes: &[u8]) -> Result<Vec<u8>, String> {
    generate_resource(font_bytes, BACTERIA_12_NATIVE_SPEC)
}

#[cfg(feature = "asset-tools")]
pub fn generate_jersey_25(font_bytes: &[u8]) -> Result<Vec<u8>, String> {
    generate_resource(font_bytes, JERSEY_25_SPEC)
}

#[cfg(feature = "asset-tools")]
pub fn generate_jersey_15(font_bytes: &[u8]) -> Result<Vec<u8>, String> {
    generate_resource(font_bytes, JERSEY_15_SPEC)
}

#[cfg(feature = "asset-tools")]
pub fn generate_terminus_8x14_normal(source: &str) -> Result<Vec<u8>, String> {
    generate_bdf_resource(source, "Terminus 8x14", 400, 2, 7.0, 10.0)
}

#[cfg(feature = "asset-tools")]
pub fn generate_terminus_8x14_native(source: &str) -> Result<Vec<u8>, String> {
    generate_bdf_resource(source, "Terminus 8x14", 400, 1, 7.0, 10.0)
}

#[cfg(feature = "asset-tools")]
pub fn generate_terminus_8x14_bold(source: &str) -> Result<Vec<u8>, String> {
    generate_bdf_resource(source, "Terminus 8x14 Bold", 700, 2, 7.0, 10.0)
}

#[cfg(feature = "asset-tools")]
pub fn generate_spleen_5x8_native(source: &str) -> Result<Vec<u8>, String> {
    generate_bdf_resource(source, "Spleen 5x8", 400, 1, 5.0, 7.0)
}

#[cfg(feature = "asset-tools")]
pub fn generate_spleen_5x8_doubled(source: &str) -> Result<Vec<u8>, String> {
    generate_bdf_resource(source, "Spleen 5x8", 400, 2, 5.0, 7.0)
}

#[cfg(feature = "asset-tools")]
pub fn generate_spleen_6x12_native(source: &str) -> Result<Vec<u8>, String> {
    generate_bdf_resource(source, "Spleen 6x12", 400, 1, 6.0, 8.0)
}

#[cfg(feature = "asset-tools")]
pub fn generate_spleen_6x12_doubled(source: &str) -> Result<Vec<u8>, String> {
    generate_bdf_resource(source, "Spleen 6x12", 400, 2, 6.0, 8.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const YESTERDAY_10_TTF: &[u8] =
        include_bytes!("../../../private/magik-assets/fonts/yesterday-10/Yesterday 10.ttf");
    const XERXES_10_TTF: &[u8] =
        include_bytes!("../../../private/magik-assets/fonts/xerxes-10/Xerxes 10.ttf");
    const NOCIVE_15_TTF: &[u8] =
        include_bytes!("../../../private/magik-assets/fonts/nocive-15/Nocive 15.ttf");
    const BACTERIA_12_TTF: &[u8] =
        include_bytes!("../../../private/magik-assets/fonts/bacteria-12/Bacteria 12.ttf");
    const JERSEY_15_TTF: &[u8] = include_bytes!("../ui/fonts/Jersey15-Regular.ttf");
    const JERSEY_25_TTF: &[u8] = include_bytes!("../ui/fonts/Jersey25-Regular.ttf");
    const TERMINUS_8X14_NORMAL_BDF: &str =
        include_str!("../assets/fonts/terminus-8x14/ter-u14n.bdf");
    const TERMINUS_8X14_BOLD_BDF: &str = include_str!("../assets/fonts/terminus-8x14/ter-u14b.bdf");
    const SPLEEN_5X8_BDF: &str = include_str!("../assets/fonts/spleen/spleen-5x8.bdf");
    const SPLEEN_6X12_BDF: &str = include_str!("../assets/fonts/spleen/spleen-6x12.bdf");

    fn glyph<'a>(font: &'a DecodedFont, code_point: char) -> &'a DecodedGlyph {
        font.glyphs
            .iter()
            .find(|glyph| glyph.code_point == code_point)
            .unwrap()
    }

    fn rewrite_crc(bytes: &mut [u8]) {
        let crc = crc32fast::hash(&bytes[HEADER_LEN..]);
        bytes[42..46].copy_from_slice(&crc.to_le_bytes());
    }

    #[test]
    fn checked_in_resources_are_deterministic() {
        assert_eq!(
            generate_resource(YESTERDAY_10_TTF, YESTERDAY_10_SPEC).unwrap(),
            YESTERDAY_10_RESOURCE
        );
        assert_eq!(
            generate_resource(YESTERDAY_10_TTF, YESTERDAY_10_CRT240_SPEC).unwrap(),
            YESTERDAY_10_CRT240_RESOURCE
        );
        assert_eq!(
            generate_resource(XERXES_10_TTF, XERXES_10_SPEC).unwrap(),
            XERXES_10_RESOURCE
        );
        assert_eq!(
            generate_resource(XERXES_10_TTF, XERXES_10_CRT240_SPEC).unwrap(),
            XERXES_10_CRT240_RESOURCE
        );
        assert_eq!(
            generate_resource(NOCIVE_15_TTF, NOCIVE_15_SPEC).unwrap(),
            NOCIVE_15_RESOURCE
        );
        assert_eq!(
            generate_resource(BACTERIA_12_TTF, BACTERIA_12_SPEC).unwrap(),
            BACTERIA_12_RESOURCE
        );
        assert_eq!(
            generate_resource(BACTERIA_12_TTF, BACTERIA_12_NATIVE_SPEC).unwrap(),
            BACTERIA_12_NATIVE_RESOURCE
        );
        assert_eq!(
            generate_resource(JERSEY_25_TTF, JERSEY_25_SPEC).unwrap(),
            JERSEY_25_RESOURCE
        );
        assert_eq!(
            generate_resource(JERSEY_15_TTF, JERSEY_15_SPEC).unwrap(),
            JERSEY_15_RESOURCE
        );
        assert_eq!(
            generate_bdf_resource(TERMINUS_8X14_NORMAL_BDF, "Terminus 8x14", 400, 1, 7.0, 10.0,)
                .unwrap(),
            TERMINUS_8X14_NATIVE_RESOURCE
        );
        assert_eq!(
            generate_bdf_resource(TERMINUS_8X14_NORMAL_BDF, "Terminus 8x14", 400, 2, 7.0, 10.0,)
                .unwrap(),
            TERMINUS_8X14_NORMAL_RESOURCE
        );
        assert_eq!(
            generate_bdf_resource(
                TERMINUS_8X14_BOLD_BDF,
                "Terminus 8x14 Bold",
                700,
                2,
                7.0,
                10.0,
            )
            .unwrap(),
            TERMINUS_8X14_BOLD_RESOURCE
        );
        assert_eq!(
            generate_bdf_resource(SPLEEN_5X8_BDF, "Spleen 5x8", 400, 1, 5.0, 7.0).unwrap(),
            SPLEEN_5X8_NATIVE_RESOURCE
        );
        assert_eq!(
            generate_bdf_resource(SPLEEN_5X8_BDF, "Spleen 5x8", 400, 2, 5.0, 7.0).unwrap(),
            SPLEEN_5X8_DOUBLED_RESOURCE
        );
        assert_eq!(
            generate_bdf_resource(SPLEEN_6X12_BDF, "Spleen 6x12", 400, 1, 6.0, 8.0).unwrap(),
            SPLEEN_6X12_NATIVE_RESOURCE
        );
        assert_eq!(
            generate_bdf_resource(SPLEEN_6X12_BDF, "Spleen 6x12", 400, 2, 6.0, 8.0).unwrap(),
            SPLEEN_6X12_DOUBLED_RESOURCE
        );
    }

    #[test]
    fn bitmap_font_cap_heights_are_exact() {
        let yesterday_10 = decode_resource(YESTERDAY_10_RESOURCE).unwrap();
        let yesterday_10_crt240 = decode_resource(YESTERDAY_10_CRT240_RESOURCE).unwrap();
        let xerxes_10 = decode_resource(XERXES_10_RESOURCE).unwrap();
        let xerxes_10_crt240 = decode_resource(XERXES_10_CRT240_RESOURCE).unwrap();
        let nocive_15 = decode_resource(NOCIVE_15_RESOURCE).unwrap();
        let bacteria_12 = decode_resource(BACTERIA_12_RESOURCE).unwrap();
        let bacteria_12_native = decode_resource(BACTERIA_12_NATIVE_RESOURCE).unwrap();
        let jersey_15 = decode_resource(JERSEY_15_RESOURCE).unwrap();
        let jersey_25 = decode_resource(JERSEY_25_RESOURCE).unwrap();
        for code_point in ['A', 'H', 'M', 'S'] {
            assert_eq!(glyph(&yesterday_10, code_point).height, 10);
            assert_eq!(glyph(&yesterday_10_crt240, code_point).height, 20);
            assert_eq!(glyph(&xerxes_10, code_point).height, 10);
            assert_eq!(glyph(&xerxes_10_crt240, code_point).height, 20);
            assert_eq!(glyph(&nocive_15, code_point).height, 15);
            assert_eq!(glyph(&bacteria_12, code_point).height, 24);
            assert_eq!(glyph(&bacteria_12_native, code_point).height, 12);
            assert_eq!(glyph(&jersey_15, code_point).height, 15);
            assert_eq!(glyph(&jersey_25, code_point).height, 25);
        }
    }

    #[test]
    fn descents_follow_slints_negative_metric_contract() {
        for resource in [
            YESTERDAY_10_RESOURCE,
            YESTERDAY_10_CRT240_RESOURCE,
            XERXES_10_RESOURCE,
            XERXES_10_CRT240_RESOURCE,
            NOCIVE_15_RESOURCE,
            BACTERIA_12_RESOURCE,
            BACTERIA_12_NATIVE_RESOURCE,
            JERSEY_15_RESOURCE,
            JERSEY_25_RESOURCE,
            TERMINUS_8X14_NATIVE_RESOURCE,
            TERMINUS_8X14_NORMAL_RESOURCE,
            TERMINUS_8X14_BOLD_RESOURCE,
            SPLEEN_5X8_NATIVE_RESOURCE,
            SPLEEN_5X8_DOUBLED_RESOURCE,
            SPLEEN_6X12_NATIVE_RESOURCE,
            SPLEEN_6X12_DOUBLED_RESOURCE,
        ] {
            assert!(decode_resource(resource).unwrap().descent < 0.0);
        }
    }

    #[test]
    fn terminus_sources_expand_to_exact_two_by_two_blocks() {
        for resource in [TERMINUS_8X14_NORMAL_RESOURCE, TERMINUS_8X14_BOLD_RESOURCE] {
            let font = decode_resource(resource).unwrap();
            assert_eq!(font.pixel_size, 28);
            assert_eq!(font.ascent, 24.0);
            assert_eq!(font.descent, -4.0);
            let glyph = glyph(&font, 'A');
            assert_eq!((glyph.width, glyph.height, glyph.x_advance), (16, 28, 1024));
            let alpha = unpack_glyph(glyph);
            for y in (0..glyph.height as usize).step_by(2) {
                for x in (0..glyph.width as usize).step_by(2) {
                    let value = alpha[y * glyph.width as usize + x];
                    assert_eq!(alpha[y * glyph.width as usize + x + 1], value);
                    assert_eq!(alpha[(y + 1) * glyph.width as usize + x], value);
                    assert_eq!(alpha[(y + 1) * glyph.width as usize + x + 1], value);
                }
            }
        }
        let normal = decode_resource(TERMINUS_8X14_NORMAL_RESOURCE).unwrap();
        let bold = decode_resource(TERMINUS_8X14_BOLD_RESOURCE).unwrap();
        let normal_ink = unpack_glyph(glyph(&normal, 'A'))
            .into_iter()
            .filter(|value| *value != 0)
            .count();
        let bold_ink = unpack_glyph(glyph(&bold, 'A'))
            .into_iter()
            .filter(|value| *value != 0)
            .count();
        assert!(bold_ink > normal_ink);
    }

    #[test]
    fn terminus_native_resource_preserves_the_eight_by_fourteen_cell() {
        let font = decode_resource(TERMINUS_8X14_NATIVE_RESOURCE).unwrap();
        assert_eq!(font.family_name, "Terminus 8x14");
        assert_eq!(font.pixel_size, 14);
        assert_eq!(font.ascent, 12.0);
        assert_eq!(font.descent, -2.0);
        let glyph = glyph(&font, 'A');
        assert_eq!((glyph.width, glyph.height, glyph.x_advance), (8, 14, 512));
    }

    #[test]
    fn spleen_resources_preserve_native_and_doubled_cells() {
        for (resource, family, pixel_size, width, height, advance) in [
            (SPLEEN_5X8_NATIVE_RESOURCE, "Spleen 5x8", 8, 5, 8, 320),
            (SPLEEN_5X8_DOUBLED_RESOURCE, "Spleen 5x8", 16, 10, 16, 640),
            (SPLEEN_6X12_NATIVE_RESOURCE, "Spleen 6x12", 12, 6, 12, 384),
            (SPLEEN_6X12_DOUBLED_RESOURCE, "Spleen 6x12", 24, 12, 24, 768),
        ] {
            let font = decode_resource(resource).unwrap();
            assert_eq!(font.family_name, family);
            assert_eq!(font.pixel_size, pixel_size);
            let glyph = glyph(&font, 'A');
            assert_eq!(
                (glyph.width, glyph.height, glyph.x_advance),
                (width, height, advance)
            );
        }
    }

    #[test]
    fn unpacked_coverage_is_binary() {
        for resource in [
            YESTERDAY_10_RESOURCE,
            YESTERDAY_10_CRT240_RESOURCE,
            XERXES_10_RESOURCE,
            XERXES_10_CRT240_RESOURCE,
            NOCIVE_15_RESOURCE,
            BACTERIA_12_RESOURCE,
            BACTERIA_12_NATIVE_RESOURCE,
            JERSEY_25_RESOURCE,
            SPLEEN_5X8_NATIVE_RESOURCE,
            SPLEEN_5X8_DOUBLED_RESOURCE,
            SPLEEN_6X12_NATIVE_RESOURCE,
            SPLEEN_6X12_DOUBLED_RESOURCE,
        ] {
            let font = decode_resource(resource).unwrap();
            for glyph in &font.glyphs {
                assert!(
                    unpack_glyph(glyph)
                        .into_iter()
                        .all(|alpha| alpha == 0 || alpha == 255)
                );
            }
        }
    }

    #[test]
    fn decoder_rejects_corrupt_resources() {
        let mut bad_magic = YESTERDAY_10_RESOURCE.to_vec();
        bad_magic[0] ^= 1;
        assert!(decode_resource(&bad_magic).unwrap_err().contains("magic"));
        assert!(
            decode_resource(&YESTERDAY_10_RESOURCE[..20])
                .unwrap_err()
                .contains("length")
        );

        let mut bad_checksum = YESTERDAY_10_RESOURCE.to_vec();
        *bad_checksum.last_mut().unwrap() ^= 1;
        assert!(
            decode_resource(&bad_checksum)
                .unwrap_err()
                .contains("checksum")
        );

        let mut bad_code_point = JERSEY_25_RESOURCE.to_vec();
        let family_len = usize::from(read_u16(&bad_code_point, 20).unwrap());
        let record = HEADER_LEN + family_len;
        bad_code_point[record..record + 4].copy_from_slice(&0x11_0000u32.to_le_bytes());
        rewrite_crc(&mut bad_code_point);
        assert!(
            decode_resource(&bad_code_point)
                .unwrap_err()
                .contains("Unicode")
        );

        let mut bad_offset = JERSEY_25_RESOURCE.to_vec();
        bad_offset[record + 16..record + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        rewrite_crc(&mut bad_offset);
        assert!(
            decode_resource(&bad_offset)
                .unwrap_err()
                .contains("out of range")
        );

        let mut unsorted = JERSEY_25_RESOURCE.to_vec();
        let second = record + GLYPH_RECORD_LEN;
        unsorted[second..second + 4].copy_from_slice(&0u32.to_le_bytes());
        rewrite_crc(&mut unsorted);
        assert!(decode_resource(&unsorted).unwrap_err().contains("sorted"));
    }
}
