// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-side inspection views for authoritative 15 kHz CRT framebuffers.
//!
//! The device PNG remains the authoritative capture. These helpers only create
//! square-pixel views for human inspection, preserving the source scanlines and
//! using a nearest-scanline aspect correction.

use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use std::io::{Read, Write};

const CRT_VIEW_WIDTH: usize = 640;
const CRT_VIEW_HEIGHT: usize = 480;

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct CrtPreviewImages {
    pub(crate) raw_letterbox_png: Vec<u8>,
    pub(crate) display_4x3_png: Vec<u8>,
}

pub(crate) fn derive_15khz_views(
    raw_png: &[u8],
    width: usize,
    height: usize,
) -> super::Result<Option<CrtPreviewImages>> {
    if width != CRT_VIEW_WIDTH || !matches!(height, 240 | 288) {
        return Ok(None);
    }

    let source = decode_rgb8(raw_png, width, height)?;
    let raw_letterbox = letterbox_rgb8(&source, width, height);
    let display_4x3 = display_4x3_rgb8(&source, width, height);

    Ok(Some(CrtPreviewImages {
        raw_letterbox_png: encode_rgb8(&raw_letterbox, CRT_VIEW_WIDTH, CRT_VIEW_HEIGHT)?,
        display_4x3_png: encode_rgb8(&display_4x3, CRT_VIEW_WIDTH, CRT_VIEW_HEIGHT)?,
    }))
}

fn decode_rgb8(raw_png: &[u8], width: usize, height: usize) -> super::Result<Vec<u8>> {
    let expected_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or("capture PNG geometry overflows host memory")?;
    let (png_width, png_height, compressed) = parse_png(raw_png)?;
    if png_width != width || png_height != height {
        return Err(format!(
            "capture PNG geometry {}x{} does not match metadata {}x{}",
            png_width, png_height, width, height
        )
        .into());
    }

    let scanline_len = width
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(1))
        .ok_or("capture PNG scanline size overflows host memory")?;
    let expected_scanlines = scanline_len
        .checked_mul(height)
        .ok_or("capture PNG scanline geometry overflows host memory")?;
    let mut decoder = ZlibDecoder::new(compressed.as_slice());
    let mut scanlines = Vec::with_capacity(expected_scanlines);
    decoder.read_to_end(&mut scanlines)?;
    if scanlines.len() != expected_scanlines {
        return Err(format!(
            "capture PNG decoded size {} does not match expected {}",
            scanlines.len(),
            expected_scanlines
        )
        .into());
    }

    let mut decoded = vec![0; expected_len];
    for y in 0..height {
        let source_start = y * scanline_len;
        if scanlines[source_start] != 0 {
            return Err("capture PNG uses an unsupported nonzero row filter".into());
        }
        let source = &scanlines[source_start + 1..source_start + scanline_len];
        let destination_start = y * width * 3;
        decoded[destination_start..destination_start + width * 3].copy_from_slice(source);
    }
    Ok(decoded)
}

fn letterbox_rgb8(source: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut destination = vec![0; CRT_VIEW_WIDTH * CRT_VIEW_HEIGHT * 3];
    let top = (CRT_VIEW_HEIGHT - height) / 2;
    let row_bytes = width * 3;
    for source_y in 0..height {
        let source_start = source_y * row_bytes;
        let destination_start = (top + source_y) * CRT_VIEW_WIDTH * 3;
        destination[destination_start..destination_start + row_bytes]
            .copy_from_slice(&source[source_start..source_start + row_bytes]);
    }
    destination
}

fn display_4x3_rgb8(source: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut destination = vec![0; CRT_VIEW_WIDTH * CRT_VIEW_HEIGHT * 3];
    let row_bytes = width * 3;
    for destination_y in 0..CRT_VIEW_HEIGHT {
        let source_y = ((destination_y * 2 + 1) * height / (CRT_VIEW_HEIGHT * 2)).min(height - 1);
        let source_start = source_y * row_bytes;
        let destination_start = destination_y * CRT_VIEW_WIDTH * 3;
        destination[destination_start..destination_start + row_bytes]
            .copy_from_slice(&source[source_start..source_start + row_bytes]);
    }
    destination
}

fn encode_rgb8(rgb: &[u8], width: usize, height: usize) -> super::Result<Vec<u8>> {
    let expected_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or("CRT preview geometry overflows host memory")?;
    if rgb.len() != expected_len {
        return Err(format!(
            "CRT preview RGB size {} does not match expected {}",
            rgb.len(),
            expected_len
        )
        .into());
    }

    let row_len = width * 3 + 1;
    let mut scanlines = Vec::with_capacity(row_len * height);
    for row in rgb.chunks_exact(width * 3) {
        scanlines.push(0);
        scanlines.extend_from_slice(row);
    }
    let mut compressor = ZlibEncoder::new(Vec::new(), Compression::fast());
    compressor.write_all(&scanlines)?;
    let idat = compressor.finish()?;

    let mut encoded = Vec::new();
    encoded.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&u32::try_from(width)?.to_be_bytes());
    ihdr.extend_from_slice(&u32::try_from(height)?.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    append_png_chunk(&mut encoded, b"IHDR", &ihdr);
    append_png_chunk(&mut encoded, b"IDAT", &idat);
    append_png_chunk(&mut encoded, b"IEND", &[]);
    Ok(encoded)
}

fn parse_png(raw_png: &[u8]) -> super::Result<(usize, usize, Vec<u8>)> {
    if !raw_png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("capture PNG has an invalid signature".into());
    }
    let mut offset = 8;
    let mut dimensions = None;
    let mut compressed = Vec::new();
    let mut saw_iend = false;
    while offset < raw_png.len() {
        let length_end = offset
            .checked_add(4)
            .ok_or("capture PNG chunk length overflows")?;
        if length_end > raw_png.len() {
            return Err("capture PNG has a truncated chunk length".into());
        }
        let length = usize::try_from(u32::from_be_bytes(raw_png[offset..length_end].try_into()?))?;
        let chunk_end = offset
            .checked_add(12)
            .and_then(|value| value.checked_add(length))
            .ok_or("capture PNG chunk overflows host memory")?;
        if chunk_end > raw_png.len() {
            return Err("capture PNG has a truncated chunk".into());
        }
        let tag: [u8; 4] = raw_png[offset + 4..offset + 8].try_into()?;
        let data_start = offset + 8;
        let data_end = data_start + length;
        let data = &raw_png[data_start..data_end];
        let expected_crc = u32::from_be_bytes(raw_png[data_end..chunk_end].try_into()?);
        if png_crc(&tag, data) != expected_crc {
            return Err(format!("capture PNG has an invalid {:?} CRC", tag).into());
        }
        match &tag {
            b"IHDR" => {
                if data.len() != 13 {
                    return Err("capture PNG IHDR has an invalid length".into());
                }
                let width = usize::try_from(u32::from_be_bytes(data[0..4].try_into()?))?;
                let height = usize::try_from(u32::from_be_bytes(data[4..8].try_into()?))?;
                if data[8..] != [8, 2, 0, 0, 0] {
                    return Err("capture PNG must be non-interlaced RGB8".into());
                }
                dimensions = Some((width, height));
            }
            b"IDAT" => compressed.extend_from_slice(data),
            b"IEND" => {
                if !data.is_empty() {
                    return Err("capture PNG IEND has an invalid payload".into());
                }
                saw_iend = true;
                break;
            }
            _ => {}
        }
        offset = chunk_end;
    }
    if !saw_iend {
        return Err("capture PNG is missing IEND".into());
    }
    let dimensions = dimensions.ok_or("capture PNG is missing IHDR")?;
    if compressed.is_empty() {
        return Err("capture PNG is missing IDAT".into());
    }
    Ok((dimensions.0, dimensions.1, compressed))
}

fn append_png_chunk(output: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    output.extend_from_slice(&(u32::try_from(data.len()).unwrap()).to_be_bytes());
    output.extend_from_slice(tag);
    output.extend_from_slice(data);
    output.extend_from_slice(&png_crc(tag, data).to_be_bytes());
}

fn png_crc(tag: &[u8; 4], data: &[u8]) -> u32 {
    !crc32_extend(crc32_extend(0xffff_ffff, tag), data)
}

fn crc32_extend(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_fixture(width: usize, height: usize) -> Vec<u8> {
        let mut rgb = vec![0; width * height * 3];
        for y in 0..height {
            let value = u8::try_from(y % 251).unwrap();
            for pixel in rgb[y * width * 3..(y + 1) * width * 3]
                .as_chunks_mut::<3>()
                .0
            {
                pixel.copy_from_slice(&[value, value.wrapping_add(1), value.wrapping_add(2)]);
            }
        }
        encode_rgb8(&rgb, width, height).unwrap()
    }

    fn decode_fixture(png: &[u8]) -> (usize, usize, Vec<u8>) {
        (640, 480, decode_rgb8(png, 640, 480).unwrap())
    }

    #[test]
    fn rejects_non_15khz_geometry_without_decoding() {
        assert_eq!(derive_15khz_views(&[1, 2, 3], 640, 480).unwrap(), None);
        assert_eq!(derive_15khz_views(&[1, 2, 3], 320, 240).unwrap(), None);
    }

    #[test]
    fn crt_240p_letterbox_preserves_source_rows_and_black_bars() {
        let raw = encode_fixture(640, 240);
        let views = derive_15khz_views(&raw, 640, 240).unwrap().unwrap();
        let (width, height, pixels) = decode_fixture(&views.raw_letterbox_png);
        assert_eq!((width, height), (640, 480));
        assert!(pixels[..640 * 120 * 3].iter().all(|&pixel| pixel == 0));
        assert!(pixels[640 * 360 * 3..].iter().all(|&pixel| pixel == 0));
        assert_eq!(pixels[640 * 120 * 3], 0);
        assert_eq!(pixels[640 * 359 * 3], 239);
    }

    #[test]
    fn crt_288p_letterbox_uses_96_row_bars() {
        let raw = encode_fixture(640, 288);
        let views = derive_15khz_views(&raw, 640, 288).unwrap().unwrap();
        let (width, height, pixels) = decode_fixture(&views.raw_letterbox_png);
        assert_eq!((width, height), (640, 480));
        assert!(pixels[..640 * 96 * 3].iter().all(|&pixel| pixel == 0));
        assert!(pixels[640 * 384 * 3..].iter().all(|&pixel| pixel == 0));
        assert_eq!(pixels[640 * 96 * 3], 0);
        assert_eq!(pixels[640 * 383 * 3], 36);
    }

    #[test]
    fn display_preview_uses_centered_nearest_scanline_mapping() {
        let raw = encode_fixture(640, 288);
        let views = derive_15khz_views(&raw, 640, 288).unwrap().unwrap();
        let (width, height, pixels) = decode_fixture(&views.display_4x3_png);
        assert_eq!((width, height), (640, 480));
        assert_eq!(pixels[0], 0);
        assert_eq!(pixels[640 * 3], 0);
        assert_eq!(pixels[640 * 2 * 3], 1);
        assert_eq!(pixels[640 * 479 * 3], 36);
    }

    #[test]
    fn rejects_png_metadata_geometry_mismatch() {
        let raw = encode_fixture(640, 240);
        let error = derive_15khz_views(&raw, 640, 288).unwrap_err();
        assert!(error.to_string().contains("does not match metadata"));
    }
}
