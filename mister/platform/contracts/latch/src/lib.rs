// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

mod generated;

pub use generated::*;

pub const PROTOCOL_VERSION: u16 = ACTIVE_PROTOCOL_VERSION;
pub const REQUIRED_CAPS: u16 = V5_CAPS_FLAGS;
pub const CAPS_WORD_COUNT: usize = V5_CAPS_WORDS;
pub const STATUS_WORD_COUNT: usize = V5_STATUS_WORDS;
pub const FPGA_UIO_LOCK_PATH: &str = "/tmp/mister-magik/fpga-uio.lock";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatchProtocol {
    V5,
}

impl LatchProtocol {
    pub const fn version(self) -> u16 {
        PROTOCOL_V5
    }

    pub const fn capability_flags(self) -> u16 {
        V5_CAPS_FLAGS
    }

    pub const fn caps_word_count(self) -> usize {
        V5_CAPS_WORDS
    }

    pub const fn set_word_count(self) -> usize {
        V5_SET_WORDS
    }

    pub const fn status_word_count(self) -> usize {
        V5_STATUS_WORDS
    }

    pub const fn status_has_crc(self) -> bool {
        true
    }

    pub const fn diagnostics_word_count(self) -> Option<usize> {
        Some(V5_DIAGNOSTICS_WORDS)
    }
}

impl TryFrom<u16> for LatchProtocol {
    type Error = String;

    fn try_from(version: u16) -> Result<Self, Self::Error> {
        (version == PROTOCOL_V5)
            .then_some(Self::V5)
            .ok_or_else(|| format!("unsupported latch protocol version {version}; v5 required"))
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
            && self.flags == V5_CAPS_FLAGS
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
    verify_crc(GET_FBUF_LATCH_CAPS, protocol, &words[..5], words[5])?;
    Ok(LatchCapabilities {
        protocol,
        protocol_version: words[0],
        flags: words[1],
        max_width: words[2],
        max_height: words[3],
        max_stride_bytes: words[4],
        crc: Some(words[5]),
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
    pub accepted_seq: u16,
    pub active_transaction: u16,
    pub pending_transaction: u16,
    pub accepted_transaction: u16,
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
    verify_crc(GET_FBUF_LATCH, protocol, &words[..15], words[15])?;
    Ok(LatchStatus {
        active_seq: words[0],
        pending_seq: words[1],
        flags: words[2] & 0x00ff,
        flip_count: words[3],
        post_count: words[4],
        drop_count: 0,
        base: u32::from(words[5]) | (u32::from(words[6]) << 16),
        width: words[7] & 0x0fff,
        height: words[8] & 0x0fff,
        stride: words[9] & 0x3fff,
        reject_count: words[10],
        active_route_epoch: words[11],
        accepted_seq: words[1],
        active_transaction: words[12],
        pending_transaction: words[13],
        accepted_transaction: words[14],
        crc: Some(words[15]),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatchRejectionDiagnostics {
    pub reject_count: u16,
    pub reason: u8,
    pub expected_index: u16,
    pub observed_index: u16,
    pub observed_command: u16,
    pub receiver_open: bool,
    pub receiver_faulted: bool,
    pub crc: u16,
}

pub fn decode_rejection_diagnostics(
    protocol: LatchProtocol,
    words: &[u16],
) -> Result<LatchRejectionDiagnostics, String> {
    let Some(word_count) = protocol.diagnostics_word_count() else {
        return Err("latch rejection diagnostics require protocol v5".to_string());
    };
    if words.len() != word_count {
        return Err(format!(
            "latch protocol {} rejection diagnostics need {word_count} words, got {}",
            protocol.version(),
            words.len()
        ));
    }
    verify_crc(
        GET_FBUF_LATCH_DIAGNOSTICS,
        protocol,
        &words[..word_count - 1],
        words[word_count - 1],
    )?;
    Ok(LatchRejectionDiagnostics {
        reject_count: words[0],
        reason: (words[1] & 0x000f) as u8,
        expected_index: words[2],
        observed_index: words[3],
        observed_command: words[4],
        receiver_open: words[5] & 0x0001 != 0,
        receiver_faulted: words[5] & 0x0002 != 0,
        crc: words[6],
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatchReceipt {
    pub attempted_transaction: u16,
    pub attempted_sequence: u16,
    pub disposition: u16,
    pub accepted_transaction: u16,
    pub accepted_sequence: u16,
    pub pending_transaction: u16,
    pub pending_sequence: u16,
    pub active_transaction: u16,
    pub active_sequence: u16,
    pub reject_reason: u8,
    pub crc: u16,
}

impl LatchReceipt {
    pub const fn accepted(self) -> bool {
        self.disposition == RECEIPT_ACCEPTED
    }

    pub const fn rejected(self) -> bool {
        self.disposition == RECEIPT_REJECTED
    }
}

pub fn decode_receipt(words: &[u16]) -> Result<LatchReceipt, String> {
    if words.len() != V5_RECEIPT_WORDS {
        return Err(format!(
            "latch protocol v5 receipt needs {V5_RECEIPT_WORDS} words, got {}",
            words.len()
        ));
    }
    verify_crc(
        GET_FBUF_LATCH_RECEIPT,
        LatchProtocol::V5,
        &words[..V5_RECEIPT_WORDS - 1],
        words[V5_RECEIPT_WORDS - 1],
    )?;
    if !matches!(words[2], RECEIPT_ACCEPTED | RECEIPT_REJECTED) {
        return Err(format!(
            "latch receipt is not terminal: disposition={}",
            words[2]
        ));
    }
    Ok(LatchReceipt {
        attempted_transaction: words[0],
        attempted_sequence: words[1],
        disposition: words[2],
        accepted_transaction: words[3],
        accepted_sequence: words[4],
        pending_transaction: words[5],
        pending_sequence: words[6],
        active_transaction: words[7],
        active_sequence: words[8],
        reject_reason: (words[9] & 0x000f) as u8,
        crc: words[10],
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationTelemetry {
    pub owned_vblank_count: u32,
    pub presented_vblank_count: u32,
    pub repeated_vblank_count: u32,
    pub ownership_loss_count: u32,
    pub active_sequence: u16,
    pub flags: u16,
    pub crc: u16,
}

impl PresentationTelemetry {
    pub const fn magik_ownership(self) -> bool {
        self.flags & (1 << STATUS_MAGIK_OWNERSHIP) != 0
    }

    pub const fn pending(self) -> bool {
        self.flags & (1 << STATUS_PENDING) != 0
    }

    pub const fn lifetime_invariant_valid(self) -> bool {
        self.owned_vblank_count
            == self
                .presented_vblank_count
                .wrapping_add(self.repeated_vblank_count)
    }
}

pub trait PresentationTelemetryCounters: Copy {
    fn owned_vblank_count(self) -> u32;
    fn presented_vblank_count(self) -> u32;
    fn repeated_vblank_count(self) -> u32;
    fn ownership_loss_count(self) -> u32;
    fn magik_ownership(self) -> bool;
    fn pending(self) -> bool;

    fn lifetime_invariant_valid(self) -> bool {
        self.owned_vblank_count()
            == self
                .presented_vblank_count()
                .wrapping_add(self.repeated_vblank_count())
    }
}

impl PresentationTelemetryCounters for PresentationTelemetry {
    fn owned_vblank_count(self) -> u32 {
        self.owned_vblank_count
    }

    fn presented_vblank_count(self) -> u32 {
        self.presented_vblank_count
    }

    fn repeated_vblank_count(self) -> u32 {
        self.repeated_vblank_count
    }

    fn ownership_loss_count(self) -> u32 {
        self.ownership_loss_count
    }

    fn magik_ownership(self) -> bool {
        self.magik_ownership()
    }

    fn pending(self) -> bool {
        self.pending()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationTelemetryDelta {
    pub elapsed_us: u64,
    pub owned_vblank_delta: u32,
    pub presented_vblank_delta: u32,
    pub repeated_vblank_delta: u32,
    pub ownership_loss_delta: u32,
    pub maximum_plausible_vblanks: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentationTelemetryValidationError {
    InvalidTiming,
    LifetimeInvariant,
    DeltaInvariant,
    Implausible {
        owned_vblank_delta: u32,
        maximum_plausible_vblanks: u64,
    },
    EndpointsNotOwnedAndSettled,
    OwnershipLoss {
        count: u32,
    },
}

impl std::fmt::Display for PresentationTelemetryValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTiming => formatter.write_str(
                "presentation telemetry requires non-zero elapsed time and refresh period",
            ),
            Self::LifetimeInvariant => {
                formatter.write_str("FPGA presentation telemetry lifetime invariant failed")
            }
            Self::DeltaInvariant => {
                formatter.write_str("FPGA presentation telemetry delta invariant failed")
            }
            Self::Implausible {
                owned_vblank_delta,
                maximum_plausible_vblanks,
            } => write!(
                formatter,
                "FPGA presentation telemetry delta is implausible: owned={owned_vblank_delta} maximum={maximum_plausible_vblanks}"
            ),
            Self::EndpointsNotOwnedAndSettled => formatter
                .write_str("FPGA presentation telemetry endpoints are not owned and settled"),
            Self::OwnershipLoss { count } => write!(
                formatter,
                "FPGA presentation ownership changed during measurement: losses={count}"
            ),
        }
    }
}

impl std::error::Error for PresentationTelemetryValidationError {}

pub fn validate_presentation_telemetry_window<T: PresentationTelemetryCounters>(
    start: T,
    end: T,
    elapsed_us: u64,
    refresh_period_us: u64,
) -> Result<PresentationTelemetryDelta, PresentationTelemetryValidationError> {
    if elapsed_us == 0 || refresh_period_us == 0 {
        return Err(PresentationTelemetryValidationError::InvalidTiming);
    }
    if !start.lifetime_invariant_valid() || !end.lifetime_invariant_valid() {
        return Err(PresentationTelemetryValidationError::LifetimeInvariant);
    }
    let owned_vblank_delta = end
        .owned_vblank_count()
        .wrapping_sub(start.owned_vblank_count());
    let presented_vblank_delta = end
        .presented_vblank_count()
        .wrapping_sub(start.presented_vblank_count());
    let repeated_vblank_delta = end
        .repeated_vblank_count()
        .wrapping_sub(start.repeated_vblank_count());
    let ownership_loss_delta = end
        .ownership_loss_count()
        .wrapping_sub(start.ownership_loss_count());
    if owned_vblank_delta != presented_vblank_delta.wrapping_add(repeated_vblank_delta) {
        return Err(PresentationTelemetryValidationError::DeltaInvariant);
    }
    let maximum_plausible_vblanks = elapsed_us.div_ceil(refresh_period_us).saturating_add(2);
    if [
        owned_vblank_delta,
        presented_vblank_delta,
        repeated_vblank_delta,
    ]
    .into_iter()
    .any(|count| u64::from(count) > maximum_plausible_vblanks)
    {
        return Err(PresentationTelemetryValidationError::Implausible {
            owned_vblank_delta,
            maximum_plausible_vblanks,
        });
    }
    if !start.magik_ownership() || !end.magik_ownership() || start.pending() || end.pending() {
        return Err(PresentationTelemetryValidationError::EndpointsNotOwnedAndSettled);
    }
    if ownership_loss_delta != 0 {
        return Err(PresentationTelemetryValidationError::OwnershipLoss {
            count: ownership_loss_delta,
        });
    }
    Ok(PresentationTelemetryDelta {
        elapsed_us,
        owned_vblank_delta,
        presented_vblank_delta,
        repeated_vblank_delta,
        ownership_loss_delta,
        maximum_plausible_vblanks,
    })
}

pub fn decode_presentation_telemetry(words: &[u16]) -> Result<PresentationTelemetry, String> {
    if words.len() != V5_PRESENTATION_TELEMETRY_WORDS {
        return Err(format!(
            "latch protocol v5 presentation telemetry needs {V5_PRESENTATION_TELEMETRY_WORDS} words, got {}",
            words.len()
        ));
    }
    verify_crc(
        GET_FBUF_PRESENTATION_TELEMETRY,
        LatchProtocol::V5,
        &words[..V5_PRESENTATION_TELEMETRY_WORDS - 1],
        words[V5_PRESENTATION_TELEMETRY_WORDS - 1],
    )?;
    let counter = |low: usize| u32::from(words[low]) | (u32::from(words[low + 1]) << 16);
    Ok(PresentationTelemetry {
        owned_vblank_count: counter(0),
        presented_vblank_count: counter(2),
        repeated_vblank_count: counter(4),
        ownership_loss_count: counter(6),
        active_sequence: words[8],
        flags: words[9] & 0x00ff,
        crc: words[10],
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
    pub const fn words(self) -> [u16; V5_SET_PAYLOAD_WORDS] {
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
    pub words: [u16; V5_SET_WORDS],
    pub word_count: usize,
}

pub fn encode_set(protocol: LatchProtocol, payload: LatchSetPayload) -> LatchSetWords {
    let payload_words = payload.words();
    let mut words = [0; V5_SET_WORDS];
    words[..V5_SET_PAYLOAD_WORDS].copy_from_slice(&payload_words);
    words[V5_SET_PAYLOAD_WORDS] = message_crc(SET_FBUF_LATCH, protocol, &payload_words);
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
    fn protocol_v5_requires_the_exact_complete_profile_and_caps_crc() {
        let mut words = [0; V5_CAPS_WORDS];
        words[..5].copy_from_slice(&GOLDEN_CAPS_V5_PAYLOAD);
        words[5] = GOLDEN_CAPS_V5_CRC;
        let capabilities = decode_capabilities(&words).unwrap();
        assert_eq!(capabilities.protocol, LatchProtocol::V5);
        assert!(capabilities.production_ready());

        words[1] ^= CAP_POST_CRC;
        words[5] = message_crc(GET_FBUF_LATCH_CAPS, LatchProtocol::V5, &words[..5]);
        assert!(!decode_capabilities(&words).unwrap().production_ready());
        words[1] ^= CAP_POST_CRC;
        words[5] = message_crc(GET_FBUF_LATCH_CAPS, LatchProtocol::V5, &words[..5]) ^ 1;
        assert!(decode_capabilities(&words).is_err());
    }

    #[test]
    fn shared_crc_goldens_fix_header_and_high_low_byte_order() {
        assert_eq!(
            message_crc(
                GET_FBUF_LATCH_CAPS,
                LatchProtocol::V5,
                &GOLDEN_CAPS_V5_PAYLOAD
            ),
            GOLDEN_CAPS_V5_CRC
        );
        assert_eq!(
            message_crc(SET_FBUF_LATCH, LatchProtocol::V5, &GOLDEN_SET_V5_PAYLOAD),
            GOLDEN_SET_V5_CRC
        );
        assert_eq!(
            message_crc(GET_FBUF_LATCH, LatchProtocol::V5, &GOLDEN_STATUS_V5_PAYLOAD),
            GOLDEN_STATUS_V5_CRC
        );
        assert_eq!(
            message_crc(
                GET_FBUF_LATCH_DIAGNOSTICS,
                LatchProtocol::V5,
                &GOLDEN_DIAGNOSTICS_V5_PAYLOAD
            ),
            GOLDEN_DIAGNOSTICS_V5_CRC
        );
        assert_eq!(
            message_crc(
                GET_FBUF_LATCH_RECEIPT,
                LatchProtocol::V5,
                &GOLDEN_RECEIPT_V5_PAYLOAD
            ),
            GOLDEN_RECEIPT_V5_CRC
        );
        assert_eq!(
            message_crc(
                GET_FBUF_PRESENTATION_TELEMETRY,
                LatchProtocol::V5,
                &GOLDEN_PRESENTATION_TELEMETRY_V5_PAYLOAD
            ),
            GOLDEN_PRESENTATION_TELEMETRY_V5_CRC
        );
    }

    #[test]
    fn set_encoder_always_appends_v5_crc() {
        let payload = LatchSetPayload {
            mode: GOLDEN_SET_V5_PAYLOAD[0],
            base: u32::from(GOLDEN_SET_V5_PAYLOAD[1]) | (u32::from(GOLDEN_SET_V5_PAYLOAD[2]) << 16),
            width: GOLDEN_SET_V5_PAYLOAD[3],
            height: GOLDEN_SET_V5_PAYLOAD[4],
            destination_left: GOLDEN_SET_V5_PAYLOAD[5],
            destination_right: GOLDEN_SET_V5_PAYLOAD[6],
            destination_top: GOLDEN_SET_V5_PAYLOAD[7],
            destination_bottom: GOLDEN_SET_V5_PAYLOAD[8],
            stride: GOLDEN_SET_V5_PAYLOAD[9],
            sequence: GOLDEN_SET_V5_PAYLOAD[10],
        };
        let v5 = encode_set(LatchProtocol::V5, payload);
        assert_eq!(v5.word_count, V5_SET_WORDS);
        assert_eq!(&v5.words[..V5_SET_PAYLOAD_WORDS], &GOLDEN_SET_V5_PAYLOAD);
        assert_eq!(v5.words[V5_SET_PAYLOAD_WORDS], GOLDEN_SET_V5_CRC);
    }

    #[test]
    fn status_decoder_verifies_v5_identity_and_crc() {
        let mut words = [0; V5_STATUS_WORDS];
        words[..15].copy_from_slice(&GOLDEN_STATUS_V5_PAYLOAD);
        words[15] = GOLDEN_STATUS_V5_CRC;
        let v5 = decode_status(LatchProtocol::V5, &words).unwrap();
        assert!(v5.magik_ownership());
        assert_eq!(v5.rejection_reason(), 0);
        assert_eq!(v5.reject_count, 7);
        assert_eq!(v5.active_route_epoch, 9);
        assert_eq!(v5.accepted_transaction, 101);
        words[15] ^= 1;
        assert!(decode_status(LatchProtocol::V5, &words).is_err());
    }

    #[test]
    fn rejection_diagnostics_are_v5_only_and_crc_protected() {
        let mut words = [0; V5_DIAGNOSTICS_WORDS];
        words[..6].copy_from_slice(&GOLDEN_DIAGNOSTICS_V5_PAYLOAD);
        words[6] = GOLDEN_DIAGNOSTICS_V5_CRC;

        let diagnostics = decode_rejection_diagnostics(LatchProtocol::V5, &words).unwrap();
        assert_eq!(diagnostics.reject_count, 7);
        assert_eq!(diagnostics.reason, REJECT_MISSING_WORD);
        assert_eq!(diagnostics.expected_index, 11);
        assert_eq!(diagnostics.observed_command, GET_FBUF_LATCH);
        assert!(!diagnostics.receiver_open);
        words[6] ^= 1;
        assert!(decode_rejection_diagnostics(LatchProtocol::V5, &words).is_err());
    }

    #[test]
    fn v5_requires_exact_capabilities() {
        let v5 = LatchCapabilities {
            protocol: LatchProtocol::V5,
            protocol_version: PROTOCOL_V5,
            flags: V5_CAPS_FLAGS,
            max_width: MAX_WIDTH,
            max_height: MAX_HEIGHT,
            max_stride_bytes: MAX_STRIDE_BYTES,
            crc: Some(0),
        };

        assert!(v5.production_ready());
        assert!(
            !LatchCapabilities {
                flags: v5.flags & !CAP_POST_CRC,
                ..v5
            }
            .production_ready()
        );
        assert!(
            !LatchCapabilities {
                flags: V5_CAPS_FLAGS | 0x8000,
                ..v5
            }
            .production_ready()
        );
    }

    #[test]
    fn receipt_is_terminal_and_crc_protected() {
        let mut words = [0; V5_RECEIPT_WORDS];
        words[..10].copy_from_slice(&GOLDEN_RECEIPT_V5_PAYLOAD);
        words[10] = GOLDEN_RECEIPT_V5_CRC;
        let receipt = decode_receipt(&words).unwrap();
        assert!(receipt.accepted());
        assert_eq!(receipt.attempted_transaction, 101);
        assert_eq!(receipt.active_transaction, 100);
        words[2] = RECEIPT_NONE;
        words[10] = message_crc(GET_FBUF_LATCH_RECEIPT, LatchProtocol::V5, &words[..10]);
        assert!(decode_receipt(&words).is_err());
    }

    #[test]
    fn presentation_telemetry_is_atomic_self_checking_and_crc_protected() {
        let mut words = [0; V5_PRESENTATION_TELEMETRY_WORDS];
        words[..10].copy_from_slice(&GOLDEN_PRESENTATION_TELEMETRY_V5_PAYLOAD);
        words[10] = GOLDEN_PRESENTATION_TELEMETRY_V5_CRC;
        let telemetry = decode_presentation_telemetry(&words).unwrap();
        assert_eq!(telemetry.owned_vblank_count, 0x1234_5678);
        assert_eq!(telemetry.presented_vblank_count, 0x1234_5670);
        assert_eq!(telemetry.repeated_vblank_count, 8);
        assert_eq!(telemetry.ownership_loss_count, 3);
        assert_eq!(telemetry.active_sequence, 42);
        assert!(telemetry.magik_ownership());
        assert!(!telemetry.pending());
        assert!(telemetry.lifetime_invariant_valid());

        words[10] ^= 1;
        assert!(decode_presentation_telemetry(&words).is_err());
        assert!(decode_presentation_telemetry(&words[..10]).is_err());
    }

    fn telemetry(
        owned_vblank_count: u32,
        presented_vblank_count: u32,
        repeated_vblank_count: u32,
        ownership_loss_count: u32,
    ) -> PresentationTelemetry {
        PresentationTelemetry {
            owned_vblank_count,
            presented_vblank_count,
            repeated_vblank_count,
            ownership_loss_count,
            active_sequence: 42,
            flags: (1 << STATUS_MAGIK_OWNERSHIP) as u16,
            crc: 0,
        }
    }

    #[test]
    fn presentation_telemetry_window_handles_wrap_and_repeats() {
        let delta = validate_presentation_telemetry_window(
            telemetry(u32::MAX - 1, u32::MAX - 1, 0, 7),
            telemetry(1, 0, 1, 7),
            50_001,
            16_667,
        )
        .unwrap();
        assert_eq!(delta.owned_vblank_delta, 3);
        assert_eq!(delta.presented_vblank_delta, 2);
        assert_eq!(delta.repeated_vblank_delta, 1);
        assert_eq!(delta.ownership_loss_delta, 0);
    }

    #[test]
    fn presentation_telemetry_window_rejects_invalid_authority() {
        let valid = telemetry(10, 9, 1, 0);
        let mut invalid = telemetry(11, 9, 2, 0);
        invalid.flags |= (1 << STATUS_PENDING) as u16;
        assert_eq!(
            validate_presentation_telemetry_window(valid, invalid, 16_667, 16_667),
            Err(PresentationTelemetryValidationError::EndpointsNotOwnedAndSettled)
        );

        let lost = telemetry(11, 10, 1, 1);
        assert_eq!(
            validate_presentation_telemetry_window(valid, lost, 16_667, 16_667),
            Err(PresentationTelemetryValidationError::OwnershipLoss { count: 1 })
        );

        let broken = telemetry(11, 9, 1, 0);
        assert_eq!(
            validate_presentation_telemetry_window(valid, broken, 16_667, 16_667),
            Err(PresentationTelemetryValidationError::LifetimeInvariant)
        );
    }

    #[test]
    fn protocol_v4_is_rejected_without_fallback() {
        assert!(LatchProtocol::try_from(4).is_err());
    }
}
