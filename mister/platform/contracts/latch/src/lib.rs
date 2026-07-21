// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

pub const SET_FBUF_LATCH: u16 = 0x57;
pub const GET_FBUF_LATCH: u16 = 0x58;
pub const GET_FBUF_LATCH_CAPS: u16 = 0x59;
pub const LATCH_MAGIC: u16 = 0x4d47;
pub const STATUS_MAGIC: u16 = 0x4d48;
pub const CAPS_MAGIC: u16 = 0x4d49;
pub const PROTOCOL_VERSION: u16 = 3;
pub const LEGACY_PROTOCOL_VERSION: u16 = 2;
pub const CAP_RGB565: u16 = 1 << 0;
pub const CAP_DOUBLE_BUFFER: u16 = 1 << 1;
pub const CAP_VARIABLE_GEOMETRY: u16 = 1 << 2;
pub const REQUIRED_CAPS: u16 = CAP_RGB565 | CAP_DOUBLE_BUFFER | CAP_VARIABLE_GEOMETRY;
pub const ROUTE_HDMI: u16 = 0;
pub const ROUTE_CRT_240P60: u16 = 1;
pub const ROUTES_HDMI_AND_CRT_240P60: u16 = (1 << ROUTE_HDMI) | (1 << ROUTE_CRT_240P60);
pub const TIMING_TABLE_VERSION: u16 = 1;
pub const LEGACY_CAPS_WORD_COUNT: usize = 5;
pub const CAPS_WORD_COUNT: usize = 7;
pub const LEGACY_STATUS_WORD_COUNT: usize = 11;
pub const STATUS_WORD_COUNT: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatchCapabilities {
    pub protocol_version: u16,
    pub flags: u16,
    pub max_width: u16,
    pub max_height: u16,
    pub max_stride_bytes: u16,
    pub supported_routes: u16,
    pub timing_table_version: u16,
}

impl LatchCapabilities {
    pub fn production_ready(self) -> bool {
        matches!(
            self.protocol_version,
            LEGACY_PROTOCOL_VERSION | PROTOCOL_VERSION
        ) && self.flags & REQUIRED_CAPS == REQUIRED_CAPS
            && self.max_width >= 1366
            && self.max_height >= 768
            && self.max_stride_bytes >= 2736
    }

    pub fn crt_240p60_ready(self) -> bool {
        self.protocol_version == PROTOCOL_VERSION
            && self.production_ready()
            && self.supported_routes & (1 << ROUTE_CRT_240P60) != 0
            && self.timing_table_version == TIMING_TABLE_VERSION
    }
}

pub fn decode_capabilities(words: &[u16]) -> Result<LatchCapabilities, String> {
    if !matches!(words.len(), LEGACY_CAPS_WORD_COUNT | CAPS_WORD_COUNT) {
        return Err(format!(
            "latch capabilities need {LEGACY_CAPS_WORD_COUNT} or {CAPS_WORD_COUNT} words, got {}",
            words.len()
        ));
    }
    Ok(LatchCapabilities {
        protocol_version: words[0],
        flags: words[1],
        max_width: words[2],
        max_height: words[3],
        max_stride_bytes: words[4],
        supported_routes: words.get(5).copied().unwrap_or(1 << ROUTE_HDMI),
        timing_table_version: words.get(6).copied().unwrap_or(0),
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
    pub requested_route: u16,
    pub active_route: u16,
    pub reader_flags: u16,
    pub underrun_count: u16,
    pub timeout_count: u16,
}

pub fn decode_status(words: &[u16]) -> Result<LatchStatus, String> {
    if !matches!(words.len(), LEGACY_STATUS_WORD_COUNT | STATUS_WORD_COUNT) {
        return Err(format!(
            "latch status needs {LEGACY_STATUS_WORD_COUNT} or {STATUS_WORD_COUNT} words, got {}",
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
        requested_route: words.get(11).copied().unwrap_or(ROUTE_HDMI),
        active_route: words.get(12).copied().unwrap_or(ROUTE_HDMI),
        reader_flags: words.get(13).copied().unwrap_or(0) & 0x000f,
        underrun_count: words.get(14).copied().unwrap_or(0),
        timeout_count: words.get(15).copied().unwrap_or(0),
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
        assert_eq!(status.active_route, ROUTE_HDMI);

        let status = decode_status(&[
            1, 2, 7, 3, 4, 5, 0x9000, 0x227e, 640, 240, 1280, 1, 1, 0xf, 8, 9,
        ])
        .expect("v3 status");
        assert_eq!((status.requested_route, status.active_route), (1, 1));
        assert_eq!(
            (
                status.reader_flags,
                status.underrun_count,
                status.timeout_count
            ),
            (0xf, 8, 9)
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
        assert!(decode_status(&[0; STATUS_WORD_COUNT - 1])
            .unwrap_err()
            .contains("got 15"));
    }

    #[test]
    fn production_capabilities_accept_v2_and_require_v3_routes_for_crt() {
        let caps = decode_capabilities(&[2, REQUIRED_CAPS, 1366, 768, 2736]).unwrap();
        assert!(caps.production_ready());
        assert!(!caps.crt_240p60_ready());
        let caps = decode_capabilities(&[
            3,
            REQUIRED_CAPS,
            1366,
            768,
            2736,
            ROUTES_HDMI_AND_CRT_240P60,
            TIMING_TABLE_VERSION,
        ])
        .unwrap();
        assert!(caps.production_ready());
        assert!(caps.crt_240p60_ready());
        assert!(!decode_capabilities(&[1, REQUIRED_CAPS, 1366, 768, 2736])
            .unwrap()
            .production_ready());
        assert!(!decode_capabilities(&[2, REQUIRED_CAPS, 1280, 720, 2560])
            .unwrap()
            .production_ready());
        assert!(decode_capabilities(&[]).is_err());
    }
}
