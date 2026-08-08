// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! External SNES hub artwork in a native RGB565 colour plane plus alpha plane.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"MM565A1\0";
const HEADER_LEN: usize = 24;
pub const SNES_ARTWORK_WIDTH: usize = 185;
pub const SNES_ARTWORK_HEIGHT: usize = 82;
pub const SNES_ARTWORK_RELATIVE_PATH: &str = "assets/snes/snes-small-v1.rgb565a";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnesArtwork {
    pub width: usize,
    pub height: usize,
    pub colours: Vec<u16>,
    pub alpha: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnesArtworkError {
    Io(String),
    Truncated,
    InvalidMagic,
    InvalidDimensions { width: usize, height: usize },
    InvalidStride,
    InvalidLength,
    ChecksumMismatch,
}

impl fmt::Display for SnesArtworkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "read SNES artwork: {error}"),
            Self::Truncated => formatter.write_str("SNES artwork header is truncated"),
            Self::InvalidMagic => formatter.write_str("SNES artwork magic is invalid"),
            Self::InvalidDimensions { width, height } => {
                write!(formatter, "SNES artwork dimensions are {width}x{height}")
            }
            Self::InvalidStride => formatter.write_str("SNES artwork stride is invalid"),
            Self::InvalidLength => formatter.write_str("SNES artwork payload length is invalid"),
            Self::ChecksumMismatch => formatter.write_str("SNES artwork checksum does not match"),
        }
    }
}

impl std::error::Error for SnesArtworkError {}

impl SnesArtwork {
    pub fn load(path: &Path) -> Result<Self, SnesArtworkError> {
        let bytes = fs::read(path).map_err(|error| SnesArtworkError::Io(error.to_string()))?;
        Self::decode(&bytes)
    }

    pub fn load_from_active_app() -> Result<Self, SnesArtworkError> {
        Self::load(&active_asset_path())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, SnesArtworkError> {
        if bytes.len() < HEADER_LEN {
            return Err(SnesArtworkError::Truncated);
        }
        if &bytes[..8] != MAGIC {
            return Err(SnesArtworkError::InvalidMagic);
        }
        let width = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        let height = u16::from_le_bytes([bytes[10], bytes[11]]) as usize;
        if width != SNES_ARTWORK_WIDTH || height != SNES_ARTWORK_HEIGHT {
            return Err(SnesArtworkError::InvalidDimensions { width, height });
        }
        let rgb_stride = read_u32(bytes, 12)? as usize;
        let alpha_stride = read_u32(bytes, 16)? as usize;
        if rgb_stride != width * 2 || alpha_stride != width {
            return Err(SnesArtworkError::InvalidStride);
        }
        let expected_payload = rgb_stride
            .checked_mul(height)
            .and_then(|rgb| {
                alpha_stride
                    .checked_mul(height)
                    .and_then(|alpha| rgb.checked_add(alpha))
            })
            .ok_or(SnesArtworkError::InvalidLength)?;
        if bytes.len() != HEADER_LEN + expected_payload {
            return Err(SnesArtworkError::InvalidLength);
        }
        let expected_crc = read_u32(bytes, 20)?;
        let payload = &bytes[HEADER_LEN..];
        if crc32(payload) != expected_crc {
            return Err(SnesArtworkError::ChecksumMismatch);
        }
        let colour_len = rgb_stride * height;
        let colours = payload[..colour_len]
            .chunks_exact(2)
            .map(|pixel| u16::from_le_bytes([pixel[0], pixel[1]]))
            .collect();
        Ok(Self {
            width,
            height,
            colours,
            alpha: payload[colour_len..].to_vec(),
        })
    }

    pub fn composite_pixel(&self, index: usize, background: u16) -> Option<u16> {
        let foreground = *self.colours.get(index)?;
        let alpha = u16::from(*self.alpha.get(index)?);
        Some(blend_rgb565(foreground, background, alpha))
    }
    #[cfg(any(feature = "ui", feature = "ui-preview"))]
    pub fn rgba8_bytes(&self) -> Vec<u8> {
        self.colours
            .iter()
            .zip(&self.alpha)
            .flat_map(|(colour, alpha)| {
                let red = ((colour >> 11) & 0x1f) as u8;
                let green = ((colour >> 5) & 0x3f) as u8;
                let blue = (colour & 0x1f) as u8;
                [
                    (red << 3) | (red >> 2),
                    (green << 2) | (green >> 4),
                    (blue << 3) | (blue >> 2),
                    *alpha,
                ]
            })
            .collect()
    }
}

pub fn active_asset_path() -> PathBuf {
    mister_magik_catalog::device_layout::current_app_path(SNES_ARTWORK_RELATIVE_PATH)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, SnesArtworkError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(SnesArtworkError::Truncated)?;
    Ok(u32::from_le_bytes(
        value.try_into().expect("four-byte slice"),
    ))
}

fn blend_rgb565(foreground: u16, background: u16, alpha: u16) -> u16 {
    if alpha == 255 {
        return foreground;
    }
    if alpha == 0 {
        return background;
    }
    let inverse = 255 - alpha;
    let blend = |shift: u16, mask: u16| {
        let front = (foreground >> shift) & mask;
        let back = (background >> shift) & mask;
        (front * alpha + back * inverse + 127) / 255
    };
    (blend(11, 0x1f) << 11) | (blend(5, 0x3f) << 5) | blend(0, 0x1f)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository_asset() -> Vec<u8> {
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/snes/snes-small-v1.rgb565a"))
            .expect("repository SNES artwork")
    }

    #[test]
    fn decodes_authoritative_native_pixel_asset() {
        let artwork = SnesArtwork::decode(&repository_asset()).unwrap();
        assert_eq!((artwork.width, artwork.height), (185, 82));
        assert_eq!(artwork.colours.len(), 185 * 82);
        assert_eq!(artwork.alpha.len(), 185 * 82);
        assert!(artwork.alpha.contains(&0));
        assert!(artwork.alpha.contains(&255));
        assert!(artwork.alpha.iter().any(|alpha| (1..255).contains(alpha)));
    }

    #[test]
    fn rejects_corrupt_payload() {
        let mut bytes = repository_asset();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert_eq!(
            SnesArtwork::decode(&bytes),
            Err(SnesArtworkError::ChecksumMismatch)
        );
    }

    #[test]
    fn alpha_compositing_preserves_transparent_and_opaque_pixels() {
        let artwork = SnesArtwork::decode(&repository_asset()).unwrap();
        let transparent = artwork.alpha.iter().position(|alpha| *alpha == 0).unwrap();
        let opaque = artwork
            .alpha
            .iter()
            .position(|alpha| *alpha == 255)
            .unwrap();
        assert_eq!(artwork.composite_pixel(transparent, 0x1234), Some(0x1234));
        assert_eq!(
            artwork.composite_pixel(opaque, 0x1234),
            Some(artwork.colours[opaque])
        );
    }
}
