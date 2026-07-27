// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

mod generated;

pub use generated::*;

// Historical aliases retained for v2 manifests and consumers. New code must
// negotiate a LatchProtocol and use its profile-specific counts.
pub const PROTOCOL_VERSION: u16 = ACTIVE_PROTOCOL_VERSION;
pub const REQUIRED_CAPS: u16 = V2_CAPS_FLAGS;
pub const CAPS_WORD_COUNT: usize = V2_CAPS_WORDS;
pub const STATUS_WORD_COUNT: usize = V2_STATUS_WORDS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatchProtocol {
    V2,
    V3,
}

impl LatchProtocol {
    pub const fn version(self) -> u16 {
        match self {
            Self::V2 => PROTOCOL_V2,
            Self::V3 => PROTOCOL_V3,
        }
    }

    pub const fn capability_flags(self) -> u16 {
        match self {
            Self::V2 => V2_CAPS_FLAGS,
            Self::V3 => V3_CAPS_FLAGS,
        }
    }

    pub const fn caps_word_count(self) -> usize {
        match self {
            Self::V2 => V2_CAPS_WORDS,
            Self::V3 => V3_CAPS_WORDS,
        }
    }

    pub const fn set_word_count(self) -> usize {
        match self {
            Self::V2 => V2_SET_WORDS,
            Self::V3 => V3_SET_WORDS,
        }
    }

    pub const fn status_word_count(self) -> usize {
        match self {
            Self::V2 => V2_STATUS_WORDS,
            Self::V3 => V3_STATUS_WORDS,
        }
    }

    pub const fn status_has_crc(self) -> bool {
        matches!(self, Self::V3)
    }
}

impl TryFrom<u16> for LatchProtocol {
    type Error = String;

    fn try_from(version: u16) -> Result<Self, Self::Error> {
        match version {
            PROTOCOL_V2 => Ok(Self::V2),
            PROTOCOL_V3 => Ok(Self::V3),
            _ => Err(format!("unsupported latch protocol version {version}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatchCapabilities {
    pub protocol: LatchProtocol,
    pub protocol_version: u16,
    pub flags: u16,
    pub max_width: u16,
    pub max_height: u16,
    pub max_stride_bytes: u16,
    pub crc: Option<u16>,
}

impl LatchCapabilities {
    pub fn production_ready(self) -> bool {
        self.protocol_version == self.protocol.version()
            && self.flags == self.protocol.capability_flags()
            && self.max_width == MAX_WIDTH
            && self.max_height == MAX_HEIGHT
            && self.max_stride_bytes == MAX_STRIDE_BYTES
    }
}

pub fn decode_capabilities(words: &[u16]) -> Result<LatchCapabilities, String> {
    let version = words
        .first()
        .copied()
        .ok_or_else(|| "latch capabilities omitted protocol version".to_string())?;
    let protocol = LatchProtocol::try_from(version)?;
    if words.len() != protocol.caps_word_count() {
        return Err(format!(
            "latch protocol {} capabilities need {} words, got {}",
            protocol.version(),
            protocol.caps_word_count(),
            words.len()
        ));
    }
    if protocol == LatchProtocol::V3 {
        verify_crc(GET_FBUF_LATCH_CAPS, protocol, &words[..5], words[5])?;
    }
    Ok(LatchCapabilities {
        protocol,
        protocol_version: words[0],
        flags: words[1],
        max_width: words[2],
        max_height: words[3],
        max_stride_bytes: words[4],
        crc: if protocol == LatchProtocol::V3 {
            Some(words[5])
        } else {
            None
        },
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
    pub reject_count: u16,
    pub active_route_epoch: u16,
    pub crc: Option<u16>,
}

impl LatchStatus {
    pub const fn active_enabled(self) -> bool {
        self.flags & (1 << STATUS_ACTIVE_ENABLED) != 0
    }

    pub const fn pending_enabled(self) -> bool {
        self.flags & (1 << STATUS_PENDING_ENABLED) != 0
    }

    pub const fn pending(self) -> bool {
        self.flags & (1 << STATUS_PENDING) != 0
    }

    pub const fn magik_ownership(self) -> bool {
        self.flags & (1 << STATUS_MAGIK_OWNERSHIP) != 0
    }

    pub const fn rejection_reason(self) -> u8 {
        ((self.flags >> STATUS_REJECT_REASON_SHIFT) & ((1 << STATUS_REJECT_REASON_WIDTH) - 1)) as u8
    }
}

pub fn decode_status(protocol: LatchProtocol, words: &[u16]) -> Result<LatchStatus, String> {
    if words.len() != protocol.status_word_count() {
        return Err(format!(
            "latch protocol {} status needs {} words, got {}",
            protocol.version(),
            protocol.status_word_count(),
            words.len()
        ));
    }
    if protocol == LatchProtocol::V3 {
        verify_crc(GET_FBUF_LATCH, protocol, &words[..13], words[13])?;
    }
    let flag_mask = match protocol {
        LatchProtocol::V2 => 0x0007,
        LatchProtocol::V3 => 0x00ff,
    };
    Ok(LatchStatus {
        active_seq: words[0],
        pending_seq: words[1],
        flags: words[2] & flag_mask,
        flip_count: words[3],
        post_count: words[4],
        drop_count: words[5],
        base: u32::from(words[6]) | (u32::from(words[7]) << 16),
        width: words[8] & 0x0fff,
        height: words[9] & 0x0fff,
        stride: words[10] & 0x3fff,
        reject_count: if protocol == LatchProtocol::V3 {
            words[11]
        } else {
            0
        },
        active_route_epoch: if protocol == LatchProtocol::V3 {
            words[12]
        } else {
            0
        },
        crc: if protocol == LatchProtocol::V3 {
            Some(words[13])
        } else {
            None
        },
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatchSetPayload {
    pub mode: u16,
    pub base: u32,
    pub width: u16,
    pub height: u16,
    pub destination_left: u16,
    pub destination_right: u16,
    pub destination_top: u16,
    pub destination_bottom: u16,
    pub stride: u16,
    pub sequence: u16,
}

impl LatchSetPayload {
    pub const fn words(self) -> [u16; V2_SET_WORDS] {
        [
            self.mode,
            self.base as u16,
            (self.base >> 16) as u16,
            self.width,
            self.height,
            self.destination_left,
            self.destination_right,
            self.destination_top,
            self.destination_bottom,
            self.stride,
            self.sequence,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatchSetWords {
    pub words: [u16; V3_SET_WORDS],
    pub word_count: usize,
}

pub fn encode_set(protocol: LatchProtocol, payload: LatchSetPayload) -> LatchSetWords {
    let payload_words = payload.words();
    let mut words = [0; V3_SET_WORDS];
    words[..V2_SET_WORDS].copy_from_slice(&payload_words);
    if protocol == LatchProtocol::V3 {
        words[V2_SET_WORDS] = message_crc(SET_FBUF_LATCH, protocol, &payload_words);
    }
    LatchSetWords {
        words,
        word_count: protocol.set_word_count(),
    }
}

pub const fn crc16_update_byte(mut crc: u16, byte: u8) -> u16 {
    crc ^= (byte as u16) << 8;
    let mut bit = 0;
    while bit < 8 {
        crc = if crc & 0x8000 != 0 {
            (crc << 1) ^ CRC_POLYNOMIAL
        } else {
            crc << 1
        };
        bit += 1;
    }
    crc
}

pub const fn crc16_update_word(crc: u16, word: u16) -> u16 {
    let crc = crc16_update_byte(crc, (word >> 8) as u8);
    crc16_update_byte(crc, word as u8)
}

pub fn message_crc(command: u16, protocol: LatchProtocol, payload: &[u16]) -> u16 {
    let mut crc = CRC_INITIAL;
    for word in [command, protocol.version(), payload.len() as u16] {
        crc = crc16_update_word(crc, word);
    }
    for word in payload {
        crc = crc16_update_word(crc, *word);
    }
    crc ^ CRC_FINAL_XOR
}

pub fn verify_crc(
    command: u16,
    protocol: LatchProtocol,
    payload: &[u16],
    expected: u16,
) -> Result<(), String> {
    let actual = message_crc(command, protocol, payload);
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "latch protocol {} CRC mismatch expected=0x{expected:04x} actual=0x{actual:04x}",
            protocol.version()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_v2_profile_is_frozen() {
        let capabilities =
            decode_capabilities(&[2, V2_CAPS_FLAGS, MAX_WIDTH, MAX_HEIGHT, MAX_STRIDE_BYTES])
                .unwrap();
        assert_eq!(capabilities.protocol, LatchProtocol::V2);
        assert!(capabilities.production_ready());
        assert_eq!(LatchProtocol::V2.caps_word_count(), 5);
        assert_eq!(LatchProtocol::V2.set_word_count(), 11);
        assert_eq!(LatchProtocol::V2.status_word_count(), 11);
        assert!(
            !decode_capabilities(&[
                2,
                V2_CAPS_FLAGS | CAP_POST_CRC,
                MAX_WIDTH,
                MAX_HEIGHT,
                MAX_STRIDE_BYTES,
            ])
            .unwrap()
            .production_ready()
        );
    }

    #[test]
    fn protocol_v3_requires_the_exact_complete_profile_and_caps_crc() {
        let mut words = [0; V3_CAPS_WORDS];
        words[..5].copy_from_slice(&GOLDEN_CAPS_V3_PAYLOAD);
        words[5] = GOLDEN_CAPS_V3_CRC;
        let capabilities = decode_capabilities(&words).unwrap();
        assert_eq!(capabilities.protocol, LatchProtocol::V3);
        assert!(capabilities.production_ready());

        words[1] ^= CAP_POST_CRC;
        words[5] = message_crc(GET_FBUF_LATCH_CAPS, LatchProtocol::V3, &words[..5]);
        assert!(!decode_capabilities(&words).unwrap().production_ready());
        words[1] ^= CAP_POST_CRC;
        words[5] = message_crc(GET_FBUF_LATCH_CAPS, LatchProtocol::V3, &words[..5]) ^ 1;
        assert!(decode_capabilities(&words).is_err());
    }

    #[test]
    fn shared_crc_goldens_fix_header_and_high_low_byte_order() {
        assert_eq!(
            message_crc(
                GET_FBUF_LATCH_CAPS,
                LatchProtocol::V3,
                &GOLDEN_CAPS_V3_PAYLOAD
            ),
            GOLDEN_CAPS_V3_CRC
        );
        assert_eq!(
            message_crc(SET_FBUF_LATCH, LatchProtocol::V3, &GOLDEN_SET_V3_PAYLOAD),
            GOLDEN_SET_V3_CRC
        );
        assert_eq!(
            message_crc(GET_FBUF_LATCH, LatchProtocol::V3, &GOLDEN_STATUS_V3_PAYLOAD),
            GOLDEN_STATUS_V3_CRC
        );
    }

    #[test]
    fn set_encoder_appends_crc_only_for_v3() {
        let payload = LatchSetPayload {
            mode: GOLDEN_SET_V3_PAYLOAD[0],
            base: u32::from(GOLDEN_SET_V3_PAYLOAD[1]) | (u32::from(GOLDEN_SET_V3_PAYLOAD[2]) << 16),
            width: GOLDEN_SET_V3_PAYLOAD[3],
            height: GOLDEN_SET_V3_PAYLOAD[4],
            destination_left: GOLDEN_SET_V3_PAYLOAD[5],
            destination_right: GOLDEN_SET_V3_PAYLOAD[6],
            destination_top: GOLDEN_SET_V3_PAYLOAD[7],
            destination_bottom: GOLDEN_SET_V3_PAYLOAD[8],
            stride: GOLDEN_SET_V3_PAYLOAD[9],
            sequence: GOLDEN_SET_V3_PAYLOAD[10],
        };
        let v2 = encode_set(LatchProtocol::V2, payload);
        assert_eq!(v2.word_count, V2_SET_WORDS);
        assert_eq!(&v2.words[..V2_SET_WORDS], &GOLDEN_SET_V3_PAYLOAD);
        let v3 = encode_set(LatchProtocol::V3, payload);
        assert_eq!(v3.word_count, V3_SET_WORDS);
        assert_eq!(v3.words[V2_SET_WORDS], GOLDEN_SET_V3_CRC);
    }

    #[test]
    fn status_decoders_preserve_v2_and_verify_v3() {
        let v2 = decode_status(
            LatchProtocol::V2,
            &[1, 2, 7, 3, 4, 5, 0x9000, 0x227e, 960, 540, 1920],
        )
        .unwrap();
        assert_eq!(v2.base, 0x227e_9000);
        assert_eq!((v2.width, v2.height, v2.stride), (960, 540, 1920));
        assert_eq!(v2.reject_count, 0);

        let mut words = [0; V3_STATUS_WORDS];
        words[..13].copy_from_slice(&GOLDEN_STATUS_V3_PAYLOAD);
        words[13] = GOLDEN_STATUS_V3_CRC;
        let v3 = decode_status(LatchProtocol::V3, &words).unwrap();
        assert!(v3.magik_ownership());
        assert_eq!(v3.rejection_reason(), 0);
        assert_eq!(v3.reject_count, 7);
        assert_eq!(v3.active_route_epoch, 9);
        words[13] ^= 1;
        assert!(decode_status(LatchProtocol::V3, &words).is_err());
    }
}
