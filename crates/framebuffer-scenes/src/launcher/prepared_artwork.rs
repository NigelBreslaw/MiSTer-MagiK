// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Versioned native-landscape artwork, without fonts, labels or game counts.
use super::*;
use crate::launcher_flip::Face;
use crate::launcher_texture::{Level, Texture};

const MAGIC: &[u8; 8] = b"MGCART01";
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
const W: usize = 180;
const H: usize = 252;

pub struct PreparedArtwork {
    rgb: Vec<[u8; 3]>,
    surfaces: [Vec<Rgb565Pixel>; 2],
    faces: [Face; 2],
}

fn code(id: LauncherCardId) -> u32 {
    match id {
        LauncherCardId::Arcade => 0,
        LauncherCardId::Consoles => 1,
        LauncherCardId::Computers => 2,
        LauncherCardId::Handhelds => 3,
        LauncherCardId::Favourites => 4,
        LauncherCardId::Settings => 5,
    }
}
fn reference(rgb: &[[u8; 3]]) -> Vec<Rgb565Pixel> {
    rgb.iter()
        .map(|&[r, g, b]| {
            Rgb565Pixel((u16::from(r) >> 3) << 11 | (u16::from(g) >> 2) << 5 | (u16::from(b) >> 3))
        })
        .collect()
}
fn word(bytes: &mut &[u8]) -> Result<u32, String> {
    let head = bytes.get(..4).ok_or("truncated prepared artwork")?;
    let value = u32::from_le_bytes(head.try_into().unwrap());
    *bytes = &bytes[4..];
    Ok(value)
}
fn put(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn compress(bytes: &mut Vec<u8>, raw: &[u8]) {
    let block = lz4::block::compress(raw, None, false).expect("bounded host artwork");
    put(bytes, block.len() as u32);
    bytes.extend(block);
}
fn decode<T: Copy + Default>(bytes: &mut &[u8], count: usize) -> Result<Vec<T>, String> {
    let size = word(bytes)? as usize;
    let block = bytes.get(..size).ok_or("truncated artwork block")?;
    *bytes = &bytes[size..];
    let mut pixels = vec![T::default(); count];
    let length = std::mem::size_of_val(pixels.as_slice());
    // SAFETY: initialized POD allocation below spans exactly length bytes.
    let out = unsafe { std::slice::from_raw_parts_mut(pixels.as_mut_ptr().cast::<u8>(), length) };
    let actual = lz4::block::decompress_to_buffer(block, Some(length as i32), out)
        .map_err(|e| e.to_string())?;
    if actual != length {
        return Err("prepared artwork block length mismatch".into());
    }
    Ok(pixels)
}

impl PreparedArtwork {
    /// Host-only generation. Published packs are little endian.
    pub fn encode(source: &[u8], id: LauncherCardId, colour: u16) -> Vec<u8> {
        let rgb = artwork::reduce_rgb888(source);
        let reference = reference(&rgb);
        let card = PreparedCard {
            id,
            name: "",
            games: None,
            colour,
            name_mask: Vec::new(),
            games_mask: Vec::new(),
            artwork: Some(&reference),
            rgb888: None,
        };
        let mut bytes = MAGIC.to_vec();
        put(&mut bytes, code(id));
        put(&mut bytes, u32::from(colour));
        compress(
            &mut bytes,
            &rgb.iter().flatten().copied().collect::<Vec<_>>(),
        );
        for detail in [false, true] {
            let surface = artwork::surface(&card, W, detail, None, false);
            compress(
                &mut bytes,
                &surface
                    .iter()
                    .flat_map(|p| p.0.to_le_bytes())
                    .collect::<Vec<_>>(),
            );
            let face = Face::with_rgb8(surface, &rgb, &reference, W, H);
            for level in &face.texture.levels {
                compress(
                    &mut bytes,
                    &level
                        .pixels
                        .iter()
                        .flat_map(|p| p.to_le_bytes())
                        .collect::<Vec<_>>(),
                );
            }
        }
        bytes
    }
    pub fn decode(payload: &[u8], id: LauncherCardId, colour: u16) -> Result<Self, String> {
        if !payload.starts_with(MAGIC) || payload.len() > MAX_BYTES {
            return Err("unsupported prepared artwork version/size".into());
        }
        let mut bytes = &payload[8..];
        if word(&mut bytes)? != code(id) || word(&mut bytes)? != u32::from(colour) {
            return Err("prepared artwork style mismatch".into());
        }
        let rgb = decode(&mut bytes, W * H)?;
        let mut surfaces = Vec::new();
        let mut faces = Vec::new();
        for _ in 0..2 {
            #[allow(unused_mut)]
            let mut surface: Vec<Rgb565Pixel> = decode(&mut bytes, W * H)?;
            #[cfg(target_endian = "big")]
            for pixel in &mut surface {
                pixel.0 = pixel.0.swap_bytes();
            }
            let mut levels = Vec::new();
            let mut width = W;
            for _ in 0..9 {
                #[allow(unused_mut)]
                let mut pixels: Vec<u32> = decode(&mut bytes, (width + 2) * H)?;
                #[cfg(target_endian = "big")]
                for pixel in &mut pixels {
                    *pixel = pixel.swap_bytes();
                }
                levels.push(Level {
                    pixels,
                    width,
                    height: H,
                });
                width = width.div_ceil(2);
            }
            faces.push(Face {
                #[cfg(test)]
                pixels: surface.clone(),
                width: W,
                height: H,
                reflection_fade_rows: 64,
                dithered: true,
                texture: Texture { levels },
            });
            surfaces.push(surface);
        }
        if !bytes.is_empty() {
            return Err("trailing prepared artwork bytes".into());
        }
        Ok(Self {
            rgb,
            surfaces: surfaces.try_into().unwrap(),
            faces: faces.try_into().ok().unwrap(),
        })
    }
    pub(super) fn faces(
        self,
        card: &PreparedCard<'_>,
        fonts: Option<LauncherTypography<'_>>,
    ) -> [Face; 2] {
        let reference = reference(&self.rgb);
        let mut surfaces = self.surfaces;
        let mut faces = self.faces;
        for (index, (surface, face)) in surfaces.iter_mut().zip(&mut faces).enumerate() {
            if card.name.is_empty() && (index == 0 || card.games.is_none()) {
                continue;
            }
            let before = surface.clone();
            artwork::draw_face_labels(surface, card, W, index == 1, fonts);
            face.texture
                .apply_labels(&before, surface, &self.rgb, &reference);
            #[cfg(test)]
            {
                face.pixels = surface.clone();
            }
        }
        faces
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prepared_labels_and_every_mip_match_raw_faces() {
        let source =
            include_bytes!("../../../../apps/mister/assets/ui/launcher-cards/console-nes.rgb888");
        let bytes = PreparedArtwork::encode(source, LauncherCardId::Consoles, 0x2a7f);
        use crate::bitmap_text::{BitmapFont, BitmapGlyph};
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
        for name in ["NES", "LONG 日本語 NAME", ""] {
            for games in [None, Some(0), Some(123), Some(u32::MAX)] {
                let card = PreparedCard {
                    id: LauncherCardId::Consoles,
                    colour: 0x2a7f,
                    name,
                    games,
                    name_mask: text_mask(name),
                    games_mask: games.map_or_else(Vec::new, |n| text_mask(&format_games(n))),
                    artwork: None,
                    rgb888: Some(source),
                };
                for typography in [None, Some(fonts)] {
                    let raw = artwork::faces_rgb888(&card, typography);
                    let packed = PreparedArtwork::decode(&bytes, card.id, card.colour)
                        .unwrap()
                        .faces(&card, typography);
                    for (raw, packed) in raw.iter().zip(&packed) {
                        assert_eq!(raw.pixels, packed.pixels);
                        assert!(
                            raw.texture == packed.texture,
                            "all mip rows must match exactly"
                        );
                    }
                }
            }
        }
        assert!(
            PreparedArtwork::decode(&bytes[..bytes.len() - 1], LauncherCardId::Consoles, 0x2a7f)
                .is_err()
        );
        assert!(PreparedArtwork::decode(&bytes, LauncherCardId::Computers, 0x2a7f).is_err());
        let mut extra = bytes;
        extra.push(0);
        assert!(PreparedArtwork::decode(&extra, LauncherCardId::Consoles, 0x2a7f).is_err());
    }
}
