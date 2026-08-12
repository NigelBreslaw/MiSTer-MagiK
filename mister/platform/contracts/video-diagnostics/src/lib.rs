// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

mod generated;
mod generated_hdmi_evidence;

pub use generated::*;
pub use generated_hdmi_evidence::*;

const CRC_POLYNOMIAL: u16 = 0x1021;
const CRC_INITIAL: u16 = 0xffff;
const LEGAL_TRIGGERS: &[u16] = &[
    VIDEO_DIAGNOSTICS_TRIGGER_NONE,
    VIDEO_DIAGNOSTICS_TRIGGER_LEGACY_OWNED,
    VIDEO_DIAGNOSTICS_TRIGGER_ROUTE_DIVERGENCE,
    VIDEO_DIAGNOSTICS_TRIGGER_OWNED_OSD_WRITE,
    VIDEO_DIAGNOSTICS_TRIGGER_CONTROL_OR_CLOCK,
    VIDEO_DIAGNOSTICS_TRIGGER_AVALON_ADDRESS,
    VIDEO_DIAGNOSTICS_TRIGGER_AVALON_BURST,
    VIDEO_DIAGNOSTICS_TRIGGER_AVALON_RETURN,
    VIDEO_DIAGNOSTICS_TRIGGER_AVALON_TIMEOUT,
    VIDEO_DIAGNOSTICS_TRIGGER_AVALON_NO_READS,
    VIDEO_DIAGNOSTICS_TRIGGER_FINAL_BLACK,
    VIDEO_DIAGNOSTICS_TRIGGER_FINAL_WHITE,
    VIDEO_DIAGNOSTICS_TRIGGER_FINAL_TIMING,
];

pub fn control_trigger_has_valid_provenance(trigger: u16) -> bool {
    LEGAL_TRIGGERS.contains(&trigger)
}

pub fn avalon_trigger_has_valid_provenance(trigger: u16) -> bool {
    trigger == VIDEO_DIAGNOSTICS_TRIGGER_NONE
        || matches!(
            trigger,
            VIDEO_DIAGNOSTICS_TRIGGER_AVALON_ADDRESS
                | VIDEO_DIAGNOSTICS_TRIGGER_AVALON_BURST
                | VIDEO_DIAGNOSTICS_TRIGGER_AVALON_RETURN
                | VIDEO_DIAGNOSTICS_TRIGGER_AVALON_TIMEOUT
                | VIDEO_DIAGNOSTICS_TRIGGER_AVALON_NO_READS
        )
}

pub fn output_trigger_has_valid_provenance(trigger: u16) -> bool {
    trigger == VIDEO_DIAGNOSTICS_TRIGGER_NONE
        || matches!(
            trigger,
            VIDEO_DIAGNOSTICS_TRIGGER_FINAL_BLACK
                | VIDEO_DIAGNOSTICS_TRIGGER_FINAL_WHITE
                | VIDEO_DIAGNOSTICS_TRIGGER_FINAL_TIMING
        )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoDiagnosticsState {
    Idle,
    Armed,
    Frozen,
    Partial,
}

impl TryFrom<u16> for VideoDiagnosticsState {
    type Error = String;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value & 0x0003 {
            VIDEO_DIAGNOSTICS_STATE_IDLE => Ok(Self::Idle),
            VIDEO_DIAGNOSTICS_STATE_ARMED => Ok(Self::Armed),
            VIDEO_DIAGNOSTICS_STATE_FROZEN => Ok(Self::Frozen),
            VIDEO_DIAGNOSTICS_STATE_PARTIAL => Ok(Self::Partial),
            _ => Err(format!("invalid video diagnostics state {value}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoDiagnosticsSnapshot<const N: usize> {
    pub state: VideoDiagnosticsState,
    pub trigger: u16,
    pub generation: u16,
    pub words: [u16; N],
}

pub type VideoDiagnosticsControlSnapshot =
    VideoDiagnosticsSnapshot<VIDEO_DIAGNOSTICS_CONTROL_WORDS>;
pub type VideoDiagnosticsAvalonSnapshot = VideoDiagnosticsSnapshot<VIDEO_DIAGNOSTICS_AVALON_WORDS>;
pub type VideoDiagnosticsOutputSnapshot = VideoDiagnosticsSnapshot<VIDEO_DIAGNOSTICS_OUTPUT_WORDS>;

const fn crc16_update_byte(mut crc: u16, byte: u8) -> u16 {
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

const fn crc16_update_word(crc: u16, word: u16) -> u16 {
    let crc = crc16_update_byte(crc, (word >> 8) as u8);
    crc16_update_byte(crc, word as u8)
}

pub fn message_crc(command: u16, payload: &[u16]) -> u16 {
    let mut crc = CRC_INITIAL;
    for word in [command, VIDEO_DIAGNOSTICS_SCHEMA, payload.len() as u16] {
        crc = crc16_update_word(crc, word);
    }
    for word in payload {
        crc = crc16_update_word(crc, *word);
    }
    crc
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HdmiEvidence {
    pub words: [u16; HDMI_EVIDENCE_WORDS],
}

impl HdmiEvidence {
    pub fn flags(&self) -> u16 {
        self.words[HDMI_EVIDENCE_FLAGS_WORD]
    }
}

fn message_crc_with_schema(command: u16, schema: u16, payload: &[u16]) -> u16 {
    let mut crc = CRC_INITIAL;
    for word in [command, schema, payload.len() as u16] {
        crc = crc16_update_word(crc, word);
    }
    for word in payload {
        crc = crc16_update_word(crc, *word);
    }
    crc
}

pub fn decode_hdmi_evidence(words: &[u16]) -> Result<HdmiEvidence, String> {
    if words.len() != HDMI_EVIDENCE_WORDS {
        return Err(format!(
            "HDMI evidence command 0x{GET_HDMI_EVIDENCE:02x} needs {HDMI_EVIDENCE_WORDS} words, got {}",
            words.len()
        ));
    }
    if words[HDMI_EVIDENCE_SCHEMA_WORD] != HDMI_EVIDENCE_SCHEMA {
        return Err(format!(
            "unsupported HDMI evidence schema {}",
            words[HDMI_EVIDENCE_SCHEMA_WORD]
        ));
    }
    let expected = words[HDMI_EVIDENCE_CRC_WORD];
    let actual = message_crc_with_schema(
        GET_HDMI_EVIDENCE,
        HDMI_EVIDENCE_SCHEMA,
        &words[..HDMI_EVIDENCE_CRC_WORD],
    );
    if expected != actual {
        return Err(format!(
            "HDMI evidence CRC mismatch expected=0x{expected:04x} actual=0x{actual:04x}"
        ));
    }
    let flags = words[HDMI_EVIDENCE_FLAGS_WORD];
    if flags & !HDMI_EVIDENCE_FLAGS_MASK != 0 {
        return Err(format!(
            "HDMI evidence flags contain reserved bits: 0x{flags:04x}"
        ));
    }
    let lock_loss_count = words[HDMI_EVIDENCE_LOCK_LOSS_COUNT_WORD];
    if flags & HDMI_EVIDENCE_FLAG_LOCK_CURRENT != 0
        && flags & HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH == 0
    {
        return Err(
            "HDMI lock evidence reports current lock before observing lock high".to_string(),
        );
    }
    if flags & HDMI_EVIDENCE_FLAG_LOCK_ARMED != 0 && flags & HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH == 0
    {
        return Err("HDMI lock evidence armed without first observing lock high".to_string());
    }
    if flags & HDMI_EVIDENCE_FLAG_LOCK_EVER_LOST != 0 && flags & HDMI_EVIDENCE_FLAG_LOCK_ARMED == 0
    {
        return Err("HDMI lock evidence reports loss before arming".to_string());
    }
    if (lock_loss_count != 0) != (flags & HDMI_EVIDENCE_FLAG_LOCK_EVER_LOST != 0) {
        return Err("HDMI lock evidence loss count disagrees with sticky loss state".to_string());
    }
    if flags & HDMI_EVIDENCE_FLAG_LOCK_LOSS_COUNT_OVERFLOW != 0 && lock_loss_count != u16::MAX {
        return Err("HDMI lock evidence overflow requires a saturated count".to_string());
    }
    let mut owned = [0; HDMI_EVIDENCE_WORDS];
    owned.copy_from_slice(words);
    Ok(HdmiEvidence { words: owned })
}

fn decode<const N: usize>(
    command: u16,
    generation_index: usize,
    legal_triggers: &[u16],
    words: &[u16],
) -> Result<VideoDiagnosticsSnapshot<N>, String> {
    if words.len() != N {
        return Err(format!(
            "video diagnostics command 0x{command:02x} needs {N} words, got {}",
            words.len()
        ));
    }
    if words[0] != VIDEO_DIAGNOSTICS_SCHEMA {
        return Err(format!("unsupported video diagnostics schema {}", words[0]));
    }
    if words[1] & !VIDEO_DIAGNOSTICS_STATE_FLAGS_MASK != 0 {
        return Err(format!(
            "video diagnostics state flags contain reserved bits: 0x{:04x}",
            words[1]
        ));
    }
    if !legal_triggers.contains(&words[2]) {
        return Err(format!(
            "video diagnostics command 0x{command:02x} has illegal trigger {}",
            words[2]
        ));
    }
    let expected = words[N - 1];
    let actual = message_crc(command, &words[..N - 1]);
    if expected != actual {
        return Err(format!(
            "video diagnostics CRC mismatch expected=0x{expected:04x} actual=0x{actual:04x}"
        ));
    }
    let mut owned = [0; N];
    owned.copy_from_slice(words);
    Ok(VideoDiagnosticsSnapshot {
        state: VideoDiagnosticsState::try_from(words[1])?,
        trigger: words[2],
        generation: words[generation_index],
        words: owned,
    })
}

pub fn decode_control(words: &[u16]) -> Result<VideoDiagnosticsControlSnapshot, String> {
    let snapshot = decode(GET_VIDEO_DIAGNOSTICS_CONTROL, 4, LEGAL_TRIGGERS, words)?;
    let route_control_flags = words[VIDEO_DIAGNOSTICS_CONTROL_ROUTE_CONTROL_FLAGS];
    let legacy_mask_disposition = words[VIDEO_DIAGNOSTICS_CONTROL_LEGACY_MASK_DISPOSITION];
    if words[VIDEO_DIAGNOSTICS_CONTROL_MISSING_DOMAINS] & !0x0006 != 0
        || route_control_flags & 0x00e0 != 0
        || words[VIDEO_DIAGNOSTICS_CONTROL_PRE_ROUTE_FLAGS] & !VIDEO_DIAGNOSTICS_ROUTE_FLAGS_MASK
            != 0
        || words[VIDEO_DIAGNOSTICS_CONTROL_POST_ROUTE_FLAGS] & !VIDEO_DIAGNOSTICS_ROUTE_FLAGS_MASK
            != 0
        || legacy_mask_disposition & 0x0c00 != 0
        || (legacy_mask_disposition >> 12) > VIDEO_DIAGNOSTICS_DISPOSITION_OVERLONG
    {
        return Err("control video diagnostics contains reserved bits".to_string());
    }
    Ok(snapshot)
}

pub fn decode_avalon(words: &[u16]) -> Result<VideoDiagnosticsAvalonSnapshot, String> {
    let snapshot = decode(GET_VIDEO_DIAGNOSTICS_AVALON, 3, LEGAL_TRIGGERS, words)?;
    if words[VIDEO_DIAGNOSTICS_AVALON_ROUTE_FLAGS] & !VIDEO_DIAGNOSTICS_ROUTE_FLAGS_MASK != 0
        || words[VIDEO_DIAGNOSTICS_AVALON_FAULT_FLAGS] & !VIDEO_DIAGNOSTICS_AVALON_FAULT_FLAGS_MASK
            != 0
    {
        return Err("Avalon video diagnostics contains reserved bits".to_string());
    }
    Ok(snapshot)
}

pub fn decode_output(words: &[u16]) -> Result<VideoDiagnosticsOutputSnapshot, String> {
    let snapshot = decode(GET_VIDEO_DIAGNOSTICS_OUTPUT, 3, LEGAL_TRIGGERS, words)?;
    let fault_summary = words[VIDEO_DIAGNOSTICS_OUTPUT_FAULT_SUMMARY];
    let fault_flags = fault_summary & 0x00ff;
    let geometry_faults = (fault_summary >> 8) & 0x0007;
    if words[VIDEO_DIAGNOSTICS_OUTPUT_SOURCE_FLAGS] & !VIDEO_DIAGNOSTICS_OUTPUT_SOURCE_FLAGS_MASK
        != 0
        || words[VIDEO_DIAGNOSTICS_OUTPUT_CONTROL_FLAGS]
            & !VIDEO_DIAGNOSTICS_OUTPUT_CONTROL_FLAGS_MASK
            != 0
        || fault_flags & !VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_MASK != 0
        || geometry_faults & !VIDEO_DIAGNOSTICS_GEOMETRY_FAULTS_MASK != 0
        || fault_summary & 0xf800 != 0
    {
        return Err("output video diagnostics contains reserved bits".to_string());
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_words<const N: usize>(command: u16) -> [u16; N] {
        let mut words = [0; N];
        words[0] = VIDEO_DIAGNOSTICS_SCHEMA;
        words[N - 1] = message_crc(command, &words[..N - 1]);
        words
    }

    #[test]
    fn fixed_layouts_use_independent_crc_goldens() {
        let control = zero_words::<VIDEO_DIAGNOSTICS_CONTROL_WORDS>(GET_VIDEO_DIAGNOSTICS_CONTROL);
        let avalon = zero_words::<VIDEO_DIAGNOSTICS_AVALON_WORDS>(GET_VIDEO_DIAGNOSTICS_AVALON);
        let output = zero_words::<VIDEO_DIAGNOSTICS_OUTPUT_WORDS>(GET_VIDEO_DIAGNOSTICS_OUTPUT);
        assert_eq!(
            control[VIDEO_DIAGNOSTICS_CONTROL_CRC],
            VIDEO_DIAGNOSTICS_CONTROL_ZERO_GOLDEN_CRC
        );
        assert_eq!(avalon[15], VIDEO_DIAGNOSTICS_AVALON_ZERO_GOLDEN_CRC);
        assert_eq!(output[15], VIDEO_DIAGNOSTICS_OUTPUT_ZERO_GOLDEN_CRC);
        assert_eq!(
            decode_control(&control).unwrap().state,
            VideoDiagnosticsState::Idle
        );
        assert_eq!(
            decode_avalon(&avalon).unwrap().state,
            VideoDiagnosticsState::Idle
        );
        assert_eq!(
            decode_output(&output).unwrap().state,
            VideoDiagnosticsState::Idle
        );
    }

    #[test]
    fn hdmi_evidence_zero_record_has_independent_golden() {
        let mut words = [0; HDMI_EVIDENCE_WORDS];
        words[HDMI_EVIDENCE_SCHEMA_WORD] = HDMI_EVIDENCE_SCHEMA;
        words[HDMI_EVIDENCE_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_EVIDENCE,
            HDMI_EVIDENCE_SCHEMA,
            &words[..HDMI_EVIDENCE_CRC_WORD],
        );
        assert_eq!(words[HDMI_EVIDENCE_CRC_WORD], HDMI_EVIDENCE_ZERO_GOLDEN_CRC);
        let decoded = decode_hdmi_evidence(&words).unwrap();
        assert_eq!(decoded.flags(), 0);
    }

    #[test]
    fn hdmi_evidence_rejects_reserved_flags_and_bad_crc() {
        let mut words = [0; HDMI_EVIDENCE_WORDS];
        words[HDMI_EVIDENCE_SCHEMA_WORD] = HDMI_EVIDENCE_SCHEMA;
        words[HDMI_EVIDENCE_FLAGS_WORD] = 1 << 15;
        words[HDMI_EVIDENCE_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_EVIDENCE,
            HDMI_EVIDENCE_SCHEMA,
            &words[..HDMI_EVIDENCE_CRC_WORD],
        );
        assert!(decode_hdmi_evidence(&words).is_err());
        words[HDMI_EVIDENCE_FLAGS_WORD] = 0;
        words[HDMI_EVIDENCE_CRC_WORD] ^= 1;
        assert!(decode_hdmi_evidence(&words).is_err());
    }

    #[test]
    fn hdmi_evidence_rejects_impossible_lock_state() {
        let mut words = [0; HDMI_EVIDENCE_WORDS];
        words[HDMI_EVIDENCE_SCHEMA_WORD] = HDMI_EVIDENCE_SCHEMA;
        words[HDMI_EVIDENCE_FLAGS_WORD] = HDMI_EVIDENCE_FLAG_LOCK_EVER_LOST;
        words[HDMI_EVIDENCE_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_EVIDENCE,
            HDMI_EVIDENCE_SCHEMA,
            &words[..HDMI_EVIDENCE_CRC_WORD],
        );
        assert!(decode_hdmi_evidence(&words).is_err());

        words[HDMI_EVIDENCE_FLAGS_WORD] = HDMI_EVIDENCE_FLAG_LOCK_CURRENT;
        words[HDMI_EVIDENCE_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_EVIDENCE,
            HDMI_EVIDENCE_SCHEMA,
            &words[..HDMI_EVIDENCE_CRC_WORD],
        );
        assert!(decode_hdmi_evidence(&words).is_err());

        words[HDMI_EVIDENCE_FLAGS_WORD] = HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH
            | HDMI_EVIDENCE_FLAG_LOCK_ARMED
            | HDMI_EVIDENCE_FLAG_LOCK_EVER_LOST
            | HDMI_EVIDENCE_FLAG_LOCK_LOSS_COUNT_OVERFLOW;
        words[HDMI_EVIDENCE_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_EVIDENCE,
            HDMI_EVIDENCE_SCHEMA,
            &words[..HDMI_EVIDENCE_CRC_WORD],
        );
        assert!(decode_hdmi_evidence(&words).is_err());

        words[HDMI_EVIDENCE_FLAGS_WORD] =
            HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH | HDMI_EVIDENCE_FLAG_LOCK_ARMED;
        words[HDMI_EVIDENCE_LOCK_LOSS_COUNT_WORD] = 1;
        words[HDMI_EVIDENCE_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_EVIDENCE,
            HDMI_EVIDENCE_SCHEMA,
            &words[..HDMI_EVIDENCE_CRC_WORD],
        );
        assert!(decode_hdmi_evidence(&words).is_err());

        words[HDMI_EVIDENCE_FLAGS_WORD] |= HDMI_EVIDENCE_FLAG_LOCK_EVER_LOST;
        words[HDMI_EVIDENCE_LOCK_LOSS_COUNT_WORD] = 0;
        words[HDMI_EVIDENCE_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_EVIDENCE,
            HDMI_EVIDENCE_SCHEMA,
            &words[..HDMI_EVIDENCE_CRC_WORD],
        );
        assert!(decode_hdmi_evidence(&words).is_err());
    }

    #[test]
    fn schema_crc_and_length_are_strict() {
        let mut words = zero_words::<VIDEO_DIAGNOSTICS_AVALON_WORDS>(GET_VIDEO_DIAGNOSTICS_AVALON);
        words[0] += 1;
        assert!(decode_avalon(&words).is_err());
        words[0] = VIDEO_DIAGNOSTICS_SCHEMA;
        words[5] ^= 1;
        assert!(decode_avalon(&words).is_err());
        assert!(decode_avalon(&words[..15]).is_err());

        let mut reserved =
            zero_words::<VIDEO_DIAGNOSTICS_OUTPUT_WORDS>(GET_VIDEO_DIAGNOSTICS_OUTPUT);
        reserved[1] = 0x8000;
        reserved[15] = message_crc(GET_VIDEO_DIAGNOSTICS_OUTPUT, &reserved[..15]);
        assert!(decode_output(&reserved).is_err());
        reserved[1] = 0;
        reserved[2] = VIDEO_DIAGNOSTICS_TRIGGER_AVALON_TIMEOUT;
        reserved[15] = message_crc(GET_VIDEO_DIAGNOSTICS_OUTPUT, &reserved[..15]);
        assert!(decode_output(&reserved).is_ok());
        assert!(!output_trigger_has_valid_provenance(
            decode_output(&reserved).unwrap().trigger
        ));

        reserved[2] = 0xffff;
        reserved[15] = message_crc(GET_VIDEO_DIAGNOSTICS_OUTPUT, &reserved[..15]);
        assert!(decode_output(&reserved).is_err());

        reserved[2] = VIDEO_DIAGNOSTICS_TRIGGER_NONE;
        reserved[VIDEO_DIAGNOSTICS_OUTPUT_FAULT_SUMMARY] = 0x8000;
        reserved[VIDEO_DIAGNOSTICS_OUTPUT_CRC] = message_crc(
            GET_VIDEO_DIAGNOSTICS_OUTPUT,
            &reserved[..VIDEO_DIAGNOSTICS_OUTPUT_CRC],
        );
        assert!(decode_output(&reserved).is_err());
    }

    #[test]
    fn compact_control_fields_reject_reserved_bits() {
        let mut words =
            zero_words::<VIDEO_DIAGNOSTICS_CONTROL_WORDS>(GET_VIDEO_DIAGNOSTICS_CONTROL);
        words[VIDEO_DIAGNOSTICS_CONTROL_ROUTE_CONTROL_FLAGS] = 0x0020;
        words[VIDEO_DIAGNOSTICS_CONTROL_CRC] = message_crc(
            GET_VIDEO_DIAGNOSTICS_CONTROL,
            &words[..VIDEO_DIAGNOSTICS_CONTROL_CRC],
        );
        assert!(decode_control(&words).is_err());

        words[VIDEO_DIAGNOSTICS_CONTROL_ROUTE_CONTROL_FLAGS] = 0;
        words[VIDEO_DIAGNOSTICS_CONTROL_LEGACY_MASK_DISPOSITION] = 0x6000;
        words[VIDEO_DIAGNOSTICS_CONTROL_CRC] = message_crc(
            GET_VIDEO_DIAGNOSTICS_CONTROL,
            &words[..VIDEO_DIAGNOSTICS_CONTROL_CRC],
        );
        assert!(decode_control(&words).is_err());
    }

    #[test]
    fn trigger_provenance_is_domain_specific() {
        assert!(control_trigger_has_valid_provenance(
            VIDEO_DIAGNOSTICS_TRIGGER_FINAL_BLACK
        ));
        assert!(avalon_trigger_has_valid_provenance(
            VIDEO_DIAGNOSTICS_TRIGGER_NONE
        ));
        assert!(avalon_trigger_has_valid_provenance(
            VIDEO_DIAGNOSTICS_TRIGGER_AVALON_TIMEOUT
        ));
        assert!(!avalon_trigger_has_valid_provenance(
            VIDEO_DIAGNOSTICS_TRIGGER_FINAL_BLACK
        ));
        assert!(output_trigger_has_valid_provenance(
            VIDEO_DIAGNOSTICS_TRIGGER_NONE
        ));
        assert!(output_trigger_has_valid_provenance(
            VIDEO_DIAGNOSTICS_TRIGGER_FINAL_TIMING
        ));
        assert!(!output_trigger_has_valid_provenance(
            VIDEO_DIAGNOSTICS_TRIGGER_AVALON_TIMEOUT
        ));
    }
}
