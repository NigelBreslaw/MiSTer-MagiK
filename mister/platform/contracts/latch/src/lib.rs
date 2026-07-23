// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

pub const SET_FBUF_LATCH: u16 = 0x57;
pub const GET_FBUF_LATCH: u16 = 0x58;
pub const GET_FBUF_LATCH_CAPS: u16 = 0x59;
pub const LATCH_MAGIC: u16 = 0x4d47;
pub const STATUS_MAGIC: u16 = 0x4d48;
pub const CAPS_MAGIC: u16 = 0x4d49;
pub const PROTOCOL_VERSION: u16 = 2;
pub const CAP_RGB565: u16 = 1 << 0;
pub const CAP_DOUBLE_BUFFER: u16 = 1 << 1;
pub const CAP_VARIABLE_GEOMETRY: u16 = 1 << 2;
pub const REQUIRED_CAPS: u16 = CAP_RGB565 | CAP_DOUBLE_BUFFER | CAP_VARIABLE_GEOMETRY;
pub const CAPS_WORD_COUNT: usize = 5;
pub const STATUS_WORD_COUNT: usize = 11;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatchCapabilities {
    pub protocol_version: u16,
    pub flags: u16,
    pub max_width: u16,
    pub max_height: u16,
    pub max_stride_bytes: u16,
}

impl LatchCapabilities {
    pub fn production_ready(self) -> bool {
        self.protocol_version == PROTOCOL_VERSION
            && self.flags & REQUIRED_CAPS == REQUIRED_CAPS
            && self.max_width >= 1366
            && self.max_height >= 768
            && self.max_stride_bytes >= 2736
    }
}

pub fn decode_capabilities(words: &[u16]) -> Result<LatchCapabilities, String> {
    if words.len() != CAPS_WORD_COUNT {
        return Err(format!(
            "latch capabilities need {CAPS_WORD_COUNT} words, got {}",
            words.len()
        ));
    }
    Ok(LatchCapabilities {
        protocol_version: words[0],
        flags: words[1],
        max_width: words[2],
        max_height: words[3],
        max_stride_bytes: words[4],
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatchStatus {
    pub active_seq: u16,
    pub pending_seq: u16,
    pub flags: u16,
    pub flip_count: u16,
    pub post_count: u16,
    pub drop_count: u16,
    pub base: u32,
    pub width: u16,
    pub height: u16,
    pub stride: u16,
}

pub fn decode_status(words: &[u16]) -> Result<LatchStatus, String> {
    if words.len() != STATUS_WORD_COUNT {
        return Err(format!(
            "latch status needs {STATUS_WORD_COUNT} words, got {}",
            words.len()
        ));
    }
    Ok(LatchStatus {
        active_seq: words[0],
        pending_seq: words[1],
        flags: words[2] & 0x7,
        flip_count: words[3],
        post_count: words[4],
        drop_count: words[5],
        base: u32::from(words[6]) | (u32::from(words[7]) << 16),
        width: words[8] & 0x0fff,
        height: words[9] & 0x0fff,
        stride: words[10] & 0x3fff,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_status_vector_decodes_wire_order() {
        let status = decode_status(&[1, 2, 7, 3, 4, 5, 0x9000, 0x227e, 960, 540, 1920])
            .expect("golden status");
        assert_eq!(status.base, 0x227e_9000);
        assert_eq!(
            (status.width, status.height, status.stride),
            (960, 540, 1920)
        );
    }

    #[test]
    fn status_masks_reserved_bits_and_rejects_wrong_word_counts() {
        let status = decode_status(&[1, 2, u16::MAX, 3, 4, 5, 0, 0, u16::MAX, u16::MAX, u16::MAX])
            .expect("masked status");

        assert_eq!(status.flags, 0x7);
        assert_eq!(status.width, 0x0fff);
        assert_eq!(status.height, 0x0fff);
        assert_eq!(status.stride, 0x3fff);
        assert!(decode_status(&[]).unwrap_err().contains("got 0"));
        assert!(
            decode_status(&[0; STATUS_WORD_COUNT + 1])
                .unwrap_err()
                .contains("got 12")
        );
    }

    #[test]
    fn production_capabilities_require_protocol_v2_and_qualified_maximum() {
        let caps = decode_capabilities(&[2, REQUIRED_CAPS, 1366, 768, 2736]).unwrap();
        assert!(caps.production_ready());
        assert!(
            !decode_capabilities(&[1, REQUIRED_CAPS, 1366, 768, 2736])
                .unwrap()
                .production_ready()
        );
        assert!(
            !decode_capabilities(&[2, REQUIRED_CAPS, 1280, 720, 2560])
                .unwrap()
                .production_ready()
        );
        assert!(decode_capabilities(&[]).is_err());
    }
}
