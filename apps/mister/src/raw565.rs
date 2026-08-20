// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared decoder for MiSTer MagiK raw RGB565 preview cache files.

const RAW565_MAGIC: &[u8; 8] = b"MM56501\0";
const RAW565_HEADER_LEN: usize = 20;
const MAX_DIMENSION: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raw565Image {
    pub words: Vec<u16>,
    pub width: usize,
    pub height: usize,
    pub stride_words: usize,
}

pub fn decode_raw565(data: &[u8]) -> Result<Raw565Image, String> {
    if data.len() < RAW565_HEADER_LEN || &data[..8] != RAW565_MAGIC {
        return Err("bad raw565 header".into());
    }
    let width = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
    let height = u32::from_le_bytes(data[12..16].try_into().unwrap()) as usize;
    let stride_bytes = u32::from_le_bytes(data[16..20].try_into().unwrap()) as usize;
    validate_geometry(width, height, stride_bytes)?;

    let expected_len = RAW565_HEADER_LEN
        + stride_bytes
            .checked_mul(height)
            .ok_or_else(|| "raw565 length overflow".to_string())?;
    if data.len() != expected_len {
        return Err(format!(
            "raw565 length mismatch got={} expected={expected_len}",
            data.len()
        ));
    }

    let mut words = Vec::with_capacity(stride_bytes / 2 * height);
    for chunk in data[RAW565_HEADER_LEN..].as_chunks::<2>().0 {
        words.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }
    Ok(Raw565Image {
        words,
        width,
        height,
        stride_words: stride_bytes / 2,
    })
}

fn validate_geometry(width: usize, height: usize, stride_bytes: usize) -> Result<(), String> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(format!("bad raw565 geometry width={width} height={height}"));
    }
    let min_stride = width
        .checked_mul(2)
        .ok_or_else(|| "raw565 stride overflow".to_string())?;
    if stride_bytes < min_stride || !stride_bytes.is_multiple_of(16) {
        return Err(format!(
            "bad raw565 geometry width={width} height={height} stride={stride_bytes}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw565_bytes(width: u32, height: u32, stride_bytes: u32, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(RAW565_MAGIC);
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&stride_bytes.to_le_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn decodes_current_raw565_header_and_pixels() {
        let mut payload = Vec::new();
        for word in [0x1234u16, 0xabcd, 0x0001, 0xf00d] {
            payload.extend_from_slice(&word.to_le_bytes());
        }
        payload.resize(16, 0);
        let decoded = decode_raw565(&raw565_bytes(2, 1, 16, &payload)).unwrap();
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 1);
        assert_eq!(decoded.stride_words, 8);
        assert_eq!(&decoded.words[..4], &[0x1234, 0xabcd, 0x0001, 0xf00d]);
    }

    #[test]
    fn accepts_stride_padding() {
        let payload = vec![0u8; 16 * 2];
        let decoded = decode_raw565(&raw565_bytes(3, 2, 16, &payload)).unwrap();
        assert_eq!(decoded.width, 3);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.stride_words, 8);
        assert_eq!(decoded.words.len(), 16);
    }

    #[test]
    fn rejects_malformed_magic() {
        let mut bytes = raw565_bytes(1, 1, 16, &[0; 16]);
        bytes[0] = b'X';
        assert!(decode_raw565(&bytes).unwrap_err().contains("header"));
    }

    #[test]
    fn rejects_bad_geometry() {
        assert!(
            decode_raw565(&raw565_bytes(0, 1, 16, &[0; 16]))
                .unwrap_err()
                .contains("geometry")
        );
        assert!(
            decode_raw565(&raw565_bytes(9, 1, 16, &[0; 16]))
                .unwrap_err()
                .contains("geometry")
        );
    }

    #[test]
    fn rejects_truncated_payload() {
        let err = decode_raw565(&raw565_bytes(1, 2, 16, &[0; 16])).unwrap_err();
        assert!(err.contains("length mismatch"));
    }
}
