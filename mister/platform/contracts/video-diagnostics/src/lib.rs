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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HdmiOutputActivity {
    pub words: [u16; HDMI_OUTPUT_ACTIVITY_WORDS],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawScalerState {
    pub words: [u16; RAW_SCALER_STATE_WORDS],
}

impl RawScalerState {
    pub fn flags(&self) -> u16 {
        self.words[RAW_SCALER_STATE_FLAGS_WORD]
    }

    pub fn frame_valid(&self) -> bool {
        self.flags() & RAW_SCALER_STATE_FLAG_FRAME_VALID != 0
    }

    pub fn frame_sequence(&self) -> u16 {
        self.words[RAW_SCALER_STATE_FRAME_SEQUENCE_WORD]
    }

    pub fn active_pixels(&self) -> u32 {
        u32::from(self.words[RAW_SCALER_STATE_ACTIVE_PIXELS_LOW_WORD])
            | (u32::from(
                self.words[RAW_SCALER_STATE_ACTIVE_PIXELS_HIGH_WORD]
                    & RAW_SCALER_STATE_ACTIVE_PIXELS_UPPER_MASK,
            ) << 16)
    }

    pub fn active_lines(&self) -> u16 {
        (self.words[RAW_SCALER_STATE_LINES_VARIATION_WORD] >> RAW_SCALER_STATE_ACTIVE_LINES_BIT)
            & RAW_SCALER_STATE_ACTIVE_LINES_MASK
    }

    pub fn variation_count(&self) -> u8 {
        ((self.words[RAW_SCALER_STATE_LINES_VARIATION_WORD]
            >> RAW_SCALER_STATE_VARIATION_COUNT_BIT)
            & RAW_SCALER_STATE_VARIATION_COUNT_MASK) as u8
    }

    fn crc32(&self, low_word: usize, high_word: usize) -> u32 {
        u32::from(self.words[low_word]) | (u32::from(self.words[high_word]) << 16)
    }

    pub fn newest_crc32c(&self) -> u32 {
        self.crc32(
            RAW_SCALER_STATE_NEWEST_CRC_LOW_WORD,
            RAW_SCALER_STATE_NEWEST_CRC_HIGH_WORD,
        )
    }

    pub fn previous_crc32c(&self) -> u32 {
        self.crc32(
            RAW_SCALER_STATE_PREVIOUS_CRC_LOW_WORD,
            RAW_SCALER_STATE_PREVIOUS_CRC_HIGH_WORD,
        )
    }

    pub fn oldest_crc32c(&self) -> u32 {
        self.crc32(
            RAW_SCALER_STATE_OLDEST_CRC_LOW_WORD,
            RAW_SCALER_STATE_OLDEST_CRC_HIGH_WORD,
        )
    }
}

impl HdmiOutputActivity {
    pub fn flags(&self) -> u16 {
        self.words[HDMI_OUTPUT_ACTIVITY_FLAGS_WORD]
    }

    pub fn no_de_count(&self) -> u8 {
        self.words[HDMI_OUTPUT_ACTIVITY_NO_DE_COUNT_WORD] as u8
    }

    pub fn de_all_zero_count(&self) -> u8 {
        self.words[HDMI_OUTPUT_ACTIVITY_DE_ALL_ZERO_COUNT_WORD] as u8
    }

    pub fn de_has_nonzero_count(&self) -> u8 {
        self.words[HDMI_OUTPUT_ACTIVITY_DE_HAS_NONZERO_COUNT_WORD] as u8
    }
}

fn packed_counter(words: &[u16], first_word: usize, bit: usize) -> u8 {
    let packed = u32::from(words[first_word])
        | (words
            .get(first_word + 1)
            .copied()
            .map(u32::from)
            .unwrap_or(0)
            << 16);
    ((packed >> bit) & u32::from(HDMI_PATH_ACTIVITY_COUNTER_MASK)) as u8
}

macro_rules! path_activity_record {
    ($name:ident, $words:ident, $flags_word:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct $name {
            pub words: [u16; $words],
        }

        impl $name {
            pub fn flags(&self) -> u16 {
                self.words[$flags_word]
            }
        }
    };
}

path_activity_record!(
    HdmiFinalPathActivity,
    HDMI_FINAL_PATH_ACTIVITY_WORDS,
    HDMI_FINAL_PATH_ACTIVITY_FLAGS_WORD
);
path_activity_record!(
    HdmiScalerRawActivity,
    HDMI_SCALER_RAW_ACTIVITY_WORDS,
    HDMI_SCALER_RAW_ACTIVITY_FLAGS_WORD
);
path_activity_record!(
    HdmiPostOsdActivity,
    HDMI_POST_OSD_ACTIVITY_WORDS,
    HDMI_POST_OSD_ACTIVITY_FLAGS_WORD
);
path_activity_record!(
    HdmiAvalonLivenessActivity,
    HDMI_AVALON_LIVENESS_ACTIVITY_WORDS,
    HDMI_AVALON_LIVENESS_ACTIVITY_FLAGS_WORD
);
path_activity_record!(
    HdmiScalerFetchActivity,
    HDMI_SCALER_FETCH_ACTIVITY_WORDS,
    HDMI_SCALER_FETCH_ACTIVITY_FLAGS_WORD
);

fn packed_field(word: u16, bit: usize, mask: u16) -> u8 {
    ((word >> bit) & mask) as u8
}

impl HdmiScalerFetchActivity {
    pub fn batch_two_count(&self) -> u8 {
        packed_field(
            self.words[HDMI_SCALER_FETCH_ACTIVITY_BATCH_TWO_COUNT_WORD],
            HDMI_SCALER_FETCH_ACTIVITY_BATCH_TWO_COUNT_BIT,
            HDMI_SCALER_FETCH_ACTIVITY_BATCH_TWO_COUNT_MASK,
        )
    }

    pub fn starved_frame_count(&self) -> u8 {
        packed_field(
            self.words[HDMI_SCALER_FETCH_ACTIVITY_STARVED_FRAME_COUNT_WORD],
            HDMI_SCALER_FETCH_ACTIVITY_STARVED_FRAME_COUNT_BIT,
            HDMI_SCALER_FETCH_ACTIVITY_STARVED_FRAME_COUNT_MASK,
        )
    }
}

impl HdmiFinalPathActivity {
    pub fn black_direct_count(&self) -> u8 {
        packed_counter(
            &self.words,
            HDMI_FINAL_PATH_ACTIVITY_BLACK_COUNTS_WORD,
            HDMI_FINAL_PATH_ACTIVITY_BLACK_DIRECT_BIT,
        )
    }

    pub fn black_scaled_count(&self) -> u8 {
        packed_counter(
            &self.words,
            HDMI_FINAL_PATH_ACTIVITY_BLACK_COUNTS_WORD,
            HDMI_FINAL_PATH_ACTIVITY_BLACK_SCALED_BIT,
        )
    }

    pub fn black_mixed_count(&self) -> u8 {
        packed_counter(
            &self.words,
            HDMI_FINAL_PATH_ACTIVITY_BLACK_COUNTS_WORD,
            HDMI_FINAL_PATH_ACTIVITY_BLACK_MIXED_BIT,
        )
    }

    pub fn de_has_nonzero_count(&self) -> u8 {
        packed_counter(
            &self.words,
            HDMI_FINAL_PATH_ACTIVITY_BLACK_COUNTS_WORD,
            HDMI_FINAL_PATH_ACTIVITY_DE_HAS_NONZERO_BIT,
        )
    }

    pub fn no_de_count(&self) -> u8 {
        packed_counter(
            &self.words,
            HDMI_FINAL_PATH_ACTIVITY_BLACK_COUNTS_WORD,
            HDMI_FINAL_PATH_ACTIVITY_NO_DE_BIT,
        )
    }
}

macro_rules! frame_activity_accessors {
    ($name:ident, $counts_word:ident, $no_de:ident, $zero:ident, $nonzero:ident) => {
        impl $name {
            pub fn no_de_count(&self) -> u8 {
                packed_counter(&self.words, $counts_word, $no_de)
            }

            pub fn de_all_zero_count(&self) -> u8 {
                packed_counter(&self.words, $counts_word, $zero)
            }

            pub fn de_has_nonzero_count(&self) -> u8 {
                packed_counter(&self.words, $counts_word, $nonzero)
            }
        }
    };
}

frame_activity_accessors!(
    HdmiScalerRawActivity,
    HDMI_SCALER_RAW_ACTIVITY_COUNTS_WORD,
    HDMI_SCALER_RAW_ACTIVITY_NO_DE_BIT,
    HDMI_SCALER_RAW_ACTIVITY_DE_ALL_ZERO_BIT,
    HDMI_SCALER_RAW_ACTIVITY_DE_HAS_NONZERO_BIT
);
frame_activity_accessors!(
    HdmiPostOsdActivity,
    HDMI_POST_OSD_ACTIVITY_COUNTS_WORD,
    HDMI_POST_OSD_ACTIVITY_NO_DE_BIT,
    HDMI_POST_OSD_ACTIVITY_DE_ALL_ZERO_BIT,
    HDMI_POST_OSD_ACTIVITY_DE_HAS_NONZERO_BIT
);

impl HdmiAvalonLivenessActivity {
    pub fn bucket_count(&self) -> u8 {
        packed_counter(
            &self.words,
            HDMI_AVALON_LIVENESS_ACTIVITY_COUNTS_WORD,
            HDMI_AVALON_LIVENESS_ACTIVITY_BUCKET_BIT,
        )
    }

    pub fn request_count(&self) -> u8 {
        packed_counter(
            &self.words,
            HDMI_AVALON_LIVENESS_ACTIVITY_COUNTS_WORD,
            HDMI_AVALON_LIVENESS_ACTIVITY_REQUEST_BIT,
        )
    }

    pub fn accepted_count(&self) -> u8 {
        packed_counter(
            &self.words,
            HDMI_AVALON_LIVENESS_ACTIVITY_COUNTS_WORD,
            HDMI_AVALON_LIVENESS_ACTIVITY_ACCEPTED_BIT,
        )
    }

    pub fn returned_count(&self) -> u8 {
        packed_counter(
            &self.words,
            HDMI_AVALON_LIVENESS_ACTIVITY_COUNTS_WORD,
            HDMI_AVALON_LIVENESS_ACTIVITY_RETURNED_BIT,
        )
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

pub fn decode_hdmi_output_activity(words: &[u16]) -> Result<HdmiOutputActivity, String> {
    if words.len() != HDMI_OUTPUT_ACTIVITY_WORDS {
        return Err(format!(
            "HDMI output activity command 0x{GET_HDMI_OUTPUT_ACTIVITY:02x} needs \
             {HDMI_OUTPUT_ACTIVITY_WORDS} words, got {}",
            words.len()
        ));
    }
    if words[HDMI_OUTPUT_ACTIVITY_SCHEMA_WORD] != HDMI_OUTPUT_ACTIVITY_SCHEMA {
        return Err(format!(
            "unsupported HDMI output activity schema {}",
            words[HDMI_OUTPUT_ACTIVITY_SCHEMA_WORD]
        ));
    }
    let expected = words[HDMI_OUTPUT_ACTIVITY_CRC_WORD];
    let actual = message_crc_with_schema(
        GET_HDMI_OUTPUT_ACTIVITY,
        HDMI_OUTPUT_ACTIVITY_SCHEMA,
        &words[..HDMI_OUTPUT_ACTIVITY_CRC_WORD],
    );
    if expected != actual {
        return Err(format!(
            "HDMI output activity CRC mismatch expected=0x{expected:04x} actual=0x{actual:04x}"
        ));
    }
    let flags = words[HDMI_OUTPUT_ACTIVITY_FLAGS_WORD];
    if flags & !HDMI_OUTPUT_ACTIVITY_FLAGS_MASK != 0 {
        return Err(format!(
            "HDMI output activity flags contain reserved bits: 0x{flags:04x}"
        ));
    }
    let counts = [
        words[HDMI_OUTPUT_ACTIVITY_NO_DE_COUNT_WORD],
        words[HDMI_OUTPUT_ACTIVITY_DE_ALL_ZERO_COUNT_WORD],
        words[HDMI_OUTPUT_ACTIVITY_DE_HAS_NONZERO_COUNT_WORD],
    ];
    if counts
        .iter()
        .any(|count| *count > HDMI_OUTPUT_ACTIVITY_COUNTER_MASK)
    {
        return Err(format!(
            "HDMI output activity counter exceeds its {HDMI_OUTPUT_ACTIVITY_COUNTER_BITS}-bit contract"
        ));
    }
    if flags & HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID == 0 && counts.iter().any(|count| *count != 0)
    {
        return Err(
            "HDMI output activity has counters before its first completed frame".to_string(),
        );
    }
    let mut owned = [0; HDMI_OUTPUT_ACTIVITY_WORDS];
    owned.copy_from_slice(words);
    Ok(HdmiOutputActivity { words: owned })
}

fn decode_path_activity<const N: usize>(
    label: &str,
    command: u16,
    schema: u16,
    flags_contract: (usize, u16, u16),
    packed_contract: (&[usize], &[(usize, u16)]),
    words: &[u16],
) -> Result<[u16; N], String> {
    let (flags_word, flags_mask, valid_flag) = flags_contract;
    let (packed_words, packed_masks) = packed_contract;
    if words.len() != N {
        return Err(format!(
            "{label} command 0x{command:02x} needs {N} words, got {}",
            words.len()
        ));
    }
    if words[0] != schema {
        return Err(format!("unsupported {label} schema {}", words[0]));
    }
    let expected = words[N - 1];
    let actual = message_crc_with_schema(command, schema, &words[..N - 1]);
    if expected != actual {
        return Err(format!(
            "{label} CRC mismatch expected=0x{expected:04x} actual=0x{actual:04x}"
        ));
    }
    let flags = words[flags_word];
    if flags & !flags_mask != 0 {
        return Err(format!(
            "{label} flags contain reserved bits: 0x{flags:04x}"
        ));
    }
    if flags & valid_flag == 0 && packed_words.iter().any(|index| words[*index] != 0) {
        return Err(format!("{label} has counters before becoming valid"));
    }
    for (index, mask) in packed_masks {
        if words[*index] & !mask != 0 {
            return Err(format!(
                "{label} packed counters contain reserved bits: 0x{:04x}",
                words[*index]
            ));
        }
    }
    let mut owned = [0; N];
    owned.copy_from_slice(words);
    Ok(owned)
}

pub fn decode_hdmi_final_path_activity(words: &[u16]) -> Result<HdmiFinalPathActivity, String> {
    decode_path_activity(
        "HDMI final path activity",
        GET_HDMI_FINAL_PATH_ACTIVITY,
        HDMI_FINAL_PATH_ACTIVITY_SCHEMA,
        (
            HDMI_FINAL_PATH_ACTIVITY_FLAGS_WORD,
            HDMI_FINAL_PATH_ACTIVITY_FLAGS_MASK,
            HDMI_FINAL_PATH_ACTIVITY_FLAG_FRAME_VALID,
        ),
        (
            &[
                HDMI_FINAL_PATH_ACTIVITY_BLACK_COUNTS_WORD,
                HDMI_FINAL_PATH_ACTIVITY_ACTIVITY_COUNTS_WORD,
            ],
            &[
                (HDMI_FINAL_PATH_ACTIVITY_BLACK_COUNTS_WORD, 0xffff),
                (HDMI_FINAL_PATH_ACTIVITY_ACTIVITY_COUNTS_WORD, 0x000f),
            ],
        ),
        words,
    )
    .map(|words| HdmiFinalPathActivity { words })
}

pub fn decode_hdmi_scaler_raw_activity(words: &[u16]) -> Result<HdmiScalerRawActivity, String> {
    decode_path_activity(
        "HDMI raw scaler activity",
        GET_HDMI_SCALER_RAW_ACTIVITY,
        HDMI_SCALER_RAW_ACTIVITY_SCHEMA,
        (
            HDMI_SCALER_RAW_ACTIVITY_FLAGS_WORD,
            HDMI_SCALER_RAW_ACTIVITY_FLAGS_MASK,
            HDMI_SCALER_RAW_ACTIVITY_FLAG_FRAME_VALID,
        ),
        (
            &[HDMI_SCALER_RAW_ACTIVITY_COUNTS_WORD],
            &[(HDMI_SCALER_RAW_ACTIVITY_COUNTS_WORD, 0x0fff)],
        ),
        words,
    )
    .map(|words| HdmiScalerRawActivity { words })
}

pub fn decode_hdmi_post_osd_activity(words: &[u16]) -> Result<HdmiPostOsdActivity, String> {
    decode_path_activity(
        "HDMI post-OSD activity",
        GET_HDMI_POST_OSD_ACTIVITY,
        HDMI_POST_OSD_ACTIVITY_SCHEMA,
        (
            HDMI_POST_OSD_ACTIVITY_FLAGS_WORD,
            HDMI_POST_OSD_ACTIVITY_FLAGS_MASK,
            HDMI_POST_OSD_ACTIVITY_FLAG_FRAME_VALID,
        ),
        (
            &[HDMI_POST_OSD_ACTIVITY_COUNTS_WORD],
            &[(HDMI_POST_OSD_ACTIVITY_COUNTS_WORD, 0x0fff)],
        ),
        words,
    )
    .map(|words| HdmiPostOsdActivity { words })
}

pub fn decode_hdmi_avalon_liveness_activity(
    words: &[u16],
) -> Result<HdmiAvalonLivenessActivity, String> {
    decode_path_activity(
        "HDMI Avalon liveness activity",
        GET_HDMI_AVALON_LIVENESS_ACTIVITY,
        HDMI_AVALON_LIVENESS_ACTIVITY_SCHEMA,
        (
            HDMI_AVALON_LIVENESS_ACTIVITY_FLAGS_WORD,
            HDMI_AVALON_LIVENESS_ACTIVITY_FLAGS_MASK,
            HDMI_AVALON_LIVENESS_ACTIVITY_FLAG_BUCKET_VALID,
        ),
        (
            &[HDMI_AVALON_LIVENESS_ACTIVITY_COUNTS_WORD],
            &[(HDMI_AVALON_LIVENESS_ACTIVITY_COUNTS_WORD, 0xffff)],
        ),
        words,
    )
    .map(|words| HdmiAvalonLivenessActivity { words })
}

pub fn decode_hdmi_scaler_fetch_activity(words: &[u16]) -> Result<HdmiScalerFetchActivity, String> {
    decode_path_activity(
        "HDMI scaler fetch activity",
        GET_HDMI_SCALER_FETCH_ACTIVITY,
        HDMI_SCALER_FETCH_ACTIVITY_SCHEMA,
        (
            HDMI_SCALER_FETCH_ACTIVITY_FLAGS_WORD,
            HDMI_SCALER_FETCH_ACTIVITY_FLAGS_MASK,
            HDMI_SCALER_FETCH_ACTIVITY_FLAG_SNAPSHOT_VALID,
        ),
        (
            &[
                HDMI_SCALER_FETCH_ACTIVITY_RESERVED_STATE_WORD,
                HDMI_SCALER_FETCH_ACTIVITY_EVENTS_WORD,
            ],
            &[
                (
                    HDMI_SCALER_FETCH_ACTIVITY_RESERVED_STATE_WORD,
                    !HDMI_SCALER_FETCH_ACTIVITY_RESERVED_STATE_RESERVED_ZERO_MASK,
                ),
                (
                    HDMI_SCALER_FETCH_ACTIVITY_EVENTS_WORD,
                    !HDMI_SCALER_FETCH_ACTIVITY_EVENTS_RESERVED_ZERO_MASK,
                ),
            ],
        ),
        words,
    )
    .map(|words| HdmiScalerFetchActivity { words })
}

pub fn decode_raw_scaler_state(words: &[u16]) -> Result<RawScalerState, String> {
    if words.len() != RAW_SCALER_STATE_WORDS {
        return Err(format!(
            "raw scaler state command 0x{GET_RAW_SCALER_STATE:02x} needs \
             {RAW_SCALER_STATE_WORDS} words, got {}",
            words.len()
        ));
    }
    if words[RAW_SCALER_STATE_SCHEMA_WORD] != RAW_SCALER_STATE_SCHEMA {
        return Err(format!(
            "unsupported raw scaler state schema {}",
            words[RAW_SCALER_STATE_SCHEMA_WORD]
        ));
    }
    let expected = words[RAW_SCALER_STATE_CRC_WORD];
    let actual = message_crc_with_schema(
        GET_RAW_SCALER_STATE,
        RAW_SCALER_STATE_SCHEMA,
        &words[..RAW_SCALER_STATE_CRC_WORD],
    );
    if expected != actual {
        return Err(format!(
            "raw scaler state CRC mismatch expected=0x{expected:04x} actual=0x{actual:04x}"
        ));
    }
    let flags = words[RAW_SCALER_STATE_FLAGS_WORD];
    if flags & !RAW_SCALER_STATE_FLAGS_MASK != 0 {
        return Err(format!(
            "raw scaler ordered-frame flags contain reserved bits: 0x{flags:04x}"
        ));
    }
    if words[RAW_SCALER_STATE_ACTIVE_PIXELS_HIGH_WORD]
        & RAW_SCALER_STATE_ACTIVE_PIXELS_HIGH_RESERVED_ZERO_MASK
        != 0
    {
        return Err("raw scaler ordered-frame pixel count has reserved bits".to_string());
    }
    let mut owned = [0; RAW_SCALER_STATE_WORDS];
    owned.copy_from_slice(words);
    let decoded = RawScalerState { words: owned };
    if !decoded.frame_valid() {
        if flags != 0
            || words[1..RAW_SCALER_STATE_CRC_WORD]
                .iter()
                .any(|word| *word != 0)
        {
            return Err(
                "raw scaler ordered-frame payload exists before coherent evidence".to_string(),
            );
        }
        return Ok(decoded);
    }
    if flags & RAW_SCALER_STATE_FLAG_NONEMPTY == 0 {
        return Err("raw scaler ordered-frame valid evidence is not marked nonempty".to_string());
    }
    if decoded.active_pixels() == 0
        || decoded.active_lines() == 0
        || u32::from(decoded.active_lines()) > decoded.active_pixels()
    {
        return Err(format!(
            "raw scaler ordered-frame geometry is impossible: pixels={} lines={}",
            decoded.active_pixels(),
            decoded.active_lines()
        ));
    }
    if decoded.variation_count() > 8 {
        return Err(format!(
            "raw scaler ordered-frame variation count {} exceeds eight comparisons",
            decoded.variation_count()
        ));
    }
    let window_full = flags & RAW_SCALER_STATE_FLAG_VARIATION_WINDOW_FULL != 0;
    let saturated = flags & RAW_SCALER_STATE_FLAG_VARIATION_SATURATED != 0;
    if saturated != (window_full && decoded.variation_count() == 8) {
        return Err("raw scaler ordered-frame variation flags are incoherent".to_string());
    }
    Ok(decoded)
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

    fn zero_hdmi_words<const N: usize>(command: u16, schema: u16) -> [u16; N] {
        let mut words = [0; N];
        words[0] = schema;
        words[N - 1] = message_crc_with_schema(command, schema, &words[..N - 1]);
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
    fn raw_scaler_state_decodes_ordered_frame_state() {
        let mut words = zero_hdmi_words::<RAW_SCALER_STATE_WORDS>(
            GET_RAW_SCALER_STATE,
            RAW_SCALER_STATE_SCHEMA,
        );
        words[RAW_SCALER_STATE_FLAGS_WORD] = RAW_SCALER_STATE_FLAG_FRAME_VALID
            | RAW_SCALER_STATE_FLAG_NONEMPTY
            | RAW_SCALER_STATE_FLAG_VARIATION_WINDOW_FULL;
        words[RAW_SCALER_STATE_FRAME_SEQUENCE_WORD] = 0x1234;
        words[RAW_SCALER_STATE_ACTIVE_PIXELS_LOW_WORD] = 0xa000;
        words[RAW_SCALER_STATE_ACTIVE_PIXELS_HIGH_WORD] = 0x001f;
        words[RAW_SCALER_STATE_LINES_VARIATION_WORD] = 1080 | (3 << 12);
        words[RAW_SCALER_STATE_NEWEST_CRC_LOW_WORD] = 0x5678;
        words[RAW_SCALER_STATE_NEWEST_CRC_HIGH_WORD] = 0x1234;
        words[RAW_SCALER_STATE_PREVIOUS_CRC_LOW_WORD] = 0xdef0;
        words[RAW_SCALER_STATE_PREVIOUS_CRC_HIGH_WORD] = 0x9abc;
        words[RAW_SCALER_STATE_OLDEST_CRC_LOW_WORD] = 0x3210;
        words[RAW_SCALER_STATE_OLDEST_CRC_HIGH_WORD] = 0x7654;
        words[RAW_SCALER_STATE_CRC_WORD] = message_crc_with_schema(
            GET_RAW_SCALER_STATE,
            RAW_SCALER_STATE_SCHEMA,
            &words[..RAW_SCALER_STATE_CRC_WORD],
        );
        let decoded = decode_raw_scaler_state(&words).unwrap();
        assert!(decoded.frame_valid());
        assert_eq!(decoded.frame_sequence(), 0x1234);
        assert_eq!(decoded.active_pixels(), 2_072_576);
        assert_eq!(decoded.active_lines(), 1080);
        assert_eq!(decoded.variation_count(), 3);
        assert_eq!(decoded.newest_crc32c(), 0x1234_5678);
        assert_eq!(decoded.previous_crc32c(), 0x9abc_def0);
        assert_eq!(decoded.oldest_crc32c(), 0x7654_3210);
    }

    #[test]
    fn raw_scaler_state_rejects_crc_reserved_bits_and_incoherent_geometry() {
        let mut words = zero_hdmi_words::<RAW_SCALER_STATE_WORDS>(
            GET_RAW_SCALER_STATE,
            RAW_SCALER_STATE_SCHEMA,
        );
        words[RAW_SCALER_STATE_CRC_WORD] ^= 1;
        assert!(decode_raw_scaler_state(&words).is_err());
        words[RAW_SCALER_STATE_FLAGS_WORD] =
            RAW_SCALER_STATE_FLAG_FRAME_VALID | RAW_SCALER_STATE_FLAG_NONEMPTY;
        words[RAW_SCALER_STATE_ACTIVE_PIXELS_LOW_WORD] = 1;
        words[RAW_SCALER_STATE_ACTIVE_PIXELS_HIGH_WORD] = 0x0100;
        words[RAW_SCALER_STATE_LINES_VARIATION_WORD] = 1;
        words[RAW_SCALER_STATE_CRC_WORD] = message_crc_with_schema(
            GET_RAW_SCALER_STATE,
            RAW_SCALER_STATE_SCHEMA,
            &words[..RAW_SCALER_STATE_CRC_WORD],
        );
        assert!(decode_raw_scaler_state(&words).is_err());

        words[RAW_SCALER_STATE_ACTIVE_PIXELS_HIGH_WORD] = 0;
        words[RAW_SCALER_STATE_ACTIVE_PIXELS_LOW_WORD] = 1;
        words[RAW_SCALER_STATE_LINES_VARIATION_WORD] = 2;
        words[RAW_SCALER_STATE_CRC_WORD] = message_crc_with_schema(
            GET_RAW_SCALER_STATE,
            RAW_SCALER_STATE_SCHEMA,
            &words[..RAW_SCALER_STATE_CRC_WORD],
        );
        assert!(decode_raw_scaler_state(&words).is_err());
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
    fn hdmi_output_activity_decodes_strict_snapshot() {
        let mut words = [0; HDMI_OUTPUT_ACTIVITY_WORDS];
        words[HDMI_OUTPUT_ACTIVITY_SCHEMA_WORD] = HDMI_OUTPUT_ACTIVITY_SCHEMA;
        words[HDMI_OUTPUT_ACTIVITY_FLAGS_WORD] = HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID;
        words[HDMI_OUTPUT_ACTIVITY_NO_DE_COUNT_WORD] = 3;
        words[HDMI_OUTPUT_ACTIVITY_DE_ALL_ZERO_COUNT_WORD] = 5;
        words[HDMI_OUTPUT_ACTIVITY_DE_HAS_NONZERO_COUNT_WORD] = 7;
        words[HDMI_OUTPUT_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_OUTPUT_ACTIVITY,
            HDMI_OUTPUT_ACTIVITY_SCHEMA,
            &words[..HDMI_OUTPUT_ACTIVITY_CRC_WORD],
        );
        let decoded = decode_hdmi_output_activity(&words).unwrap();
        assert_eq!(decoded.flags(), HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID);
        assert_eq!(decoded.no_de_count(), 3);
        assert_eq!(decoded.de_all_zero_count(), 5);
        assert_eq!(decoded.de_has_nonzero_count(), 7);
    }

    #[test]
    fn hdmi_output_activity_zero_record_and_invariants_are_strict() {
        let mut words = [0; HDMI_OUTPUT_ACTIVITY_WORDS];
        words[HDMI_OUTPUT_ACTIVITY_SCHEMA_WORD] = HDMI_OUTPUT_ACTIVITY_SCHEMA;
        words[HDMI_OUTPUT_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_OUTPUT_ACTIVITY,
            HDMI_OUTPUT_ACTIVITY_SCHEMA,
            &words[..HDMI_OUTPUT_ACTIVITY_CRC_WORD],
        );
        assert_eq!(
            words[HDMI_OUTPUT_ACTIVITY_CRC_WORD],
            HDMI_OUTPUT_ACTIVITY_ZERO_GOLDEN_CRC
        );
        assert!(decode_hdmi_output_activity(&words).is_ok());

        words[HDMI_OUTPUT_ACTIVITY_FLAGS_WORD] = 1 << 15;
        words[HDMI_OUTPUT_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_OUTPUT_ACTIVITY,
            HDMI_OUTPUT_ACTIVITY_SCHEMA,
            &words[..HDMI_OUTPUT_ACTIVITY_CRC_WORD],
        );
        assert!(decode_hdmi_output_activity(&words).is_err());

        words[HDMI_OUTPUT_ACTIVITY_FLAGS_WORD] = 0;
        words[HDMI_OUTPUT_ACTIVITY_NO_DE_COUNT_WORD] = 1;
        words[HDMI_OUTPUT_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_OUTPUT_ACTIVITY,
            HDMI_OUTPUT_ACTIVITY_SCHEMA,
            &words[..HDMI_OUTPUT_ACTIVITY_CRC_WORD],
        );
        assert!(decode_hdmi_output_activity(&words).is_err());

        words[HDMI_OUTPUT_ACTIVITY_FLAGS_WORD] = HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID;
        words[HDMI_OUTPUT_ACTIVITY_NO_DE_COUNT_WORD] = HDMI_OUTPUT_ACTIVITY_COUNTER_MASK + 1;
        words[HDMI_OUTPUT_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_OUTPUT_ACTIVITY,
            HDMI_OUTPUT_ACTIVITY_SCHEMA,
            &words[..HDMI_OUTPUT_ACTIVITY_CRC_WORD],
        );
        assert!(decode_hdmi_output_activity(&words).is_err());
    }

    #[test]
    fn hdmi_path_activity_records_decode_packed_counters() {
        let mut final_path = zero_hdmi_words::<HDMI_FINAL_PATH_ACTIVITY_WORDS>(
            GET_HDMI_FINAL_PATH_ACTIVITY,
            HDMI_FINAL_PATH_ACTIVITY_SCHEMA,
        );
        assert_eq!(
            final_path[HDMI_FINAL_PATH_ACTIVITY_CRC_WORD],
            HDMI_FINAL_PATH_ACTIVITY_ZERO_GOLDEN_CRC
        );
        final_path[HDMI_FINAL_PATH_ACTIVITY_FLAGS_WORD] = HDMI_FINAL_PATH_ACTIVITY_FLAG_FRAME_VALID;
        final_path[HDMI_FINAL_PATH_ACTIVITY_BLACK_COUNTS_WORD] = 0x4321;
        final_path[HDMI_FINAL_PATH_ACTIVITY_ACTIVITY_COUNTS_WORD] = 0x0005;
        final_path[HDMI_FINAL_PATH_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_FINAL_PATH_ACTIVITY,
            HDMI_FINAL_PATH_ACTIVITY_SCHEMA,
            &final_path[..HDMI_FINAL_PATH_ACTIVITY_CRC_WORD],
        );
        let decoded = decode_hdmi_final_path_activity(&final_path).unwrap();
        assert_eq!(decoded.black_direct_count(), 1);
        assert_eq!(decoded.black_scaled_count(), 2);
        assert_eq!(decoded.black_mixed_count(), 3);
        assert_eq!(decoded.de_has_nonzero_count(), 4);
        assert_eq!(decoded.no_de_count(), 5);

        let mut scaler = zero_hdmi_words::<HDMI_SCALER_RAW_ACTIVITY_WORDS>(
            GET_HDMI_SCALER_RAW_ACTIVITY,
            HDMI_SCALER_RAW_ACTIVITY_SCHEMA,
        );
        assert_eq!(
            scaler[HDMI_SCALER_RAW_ACTIVITY_CRC_WORD],
            HDMI_SCALER_RAW_ACTIVITY_ZERO_GOLDEN_CRC
        );
        scaler[HDMI_SCALER_RAW_ACTIVITY_FLAGS_WORD] = HDMI_SCALER_RAW_ACTIVITY_FLAG_FRAME_VALID;
        scaler[HDMI_SCALER_RAW_ACTIVITY_COUNTS_WORD] = 0x0765;
        scaler[HDMI_SCALER_RAW_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_SCALER_RAW_ACTIVITY,
            HDMI_SCALER_RAW_ACTIVITY_SCHEMA,
            &scaler[..HDMI_SCALER_RAW_ACTIVITY_CRC_WORD],
        );
        let decoded = decode_hdmi_scaler_raw_activity(&scaler).unwrap();
        assert_eq!(decoded.no_de_count(), 5);
        assert_eq!(decoded.de_all_zero_count(), 6);
        assert_eq!(decoded.de_has_nonzero_count(), 7);

        let mut post = zero_hdmi_words::<HDMI_POST_OSD_ACTIVITY_WORDS>(
            GET_HDMI_POST_OSD_ACTIVITY,
            HDMI_POST_OSD_ACTIVITY_SCHEMA,
        );
        assert_eq!(
            post[HDMI_POST_OSD_ACTIVITY_CRC_WORD],
            HDMI_POST_OSD_ACTIVITY_ZERO_GOLDEN_CRC
        );
        post[HDMI_POST_OSD_ACTIVITY_FLAGS_WORD] = HDMI_POST_OSD_ACTIVITY_FLAG_FRAME_VALID;
        post[HDMI_POST_OSD_ACTIVITY_COUNTS_WORD] = 0x0321;
        post[HDMI_POST_OSD_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_POST_OSD_ACTIVITY,
            HDMI_POST_OSD_ACTIVITY_SCHEMA,
            &post[..HDMI_POST_OSD_ACTIVITY_CRC_WORD],
        );
        let decoded = decode_hdmi_post_osd_activity(&post).unwrap();
        assert_eq!(decoded.no_de_count(), 1);
        assert_eq!(decoded.de_all_zero_count(), 2);
        assert_eq!(decoded.de_has_nonzero_count(), 3);

        let mut avalon = zero_hdmi_words::<HDMI_AVALON_LIVENESS_ACTIVITY_WORDS>(
            GET_HDMI_AVALON_LIVENESS_ACTIVITY,
            HDMI_AVALON_LIVENESS_ACTIVITY_SCHEMA,
        );
        assert_eq!(
            avalon[HDMI_AVALON_LIVENESS_ACTIVITY_CRC_WORD],
            HDMI_AVALON_LIVENESS_ACTIVITY_ZERO_GOLDEN_CRC
        );
        avalon[HDMI_AVALON_LIVENESS_ACTIVITY_FLAGS_WORD] =
            HDMI_AVALON_LIVENESS_ACTIVITY_FLAG_BUCKET_VALID;
        avalon[HDMI_AVALON_LIVENESS_ACTIVITY_COUNTS_WORD] = 0xa987;
        avalon[HDMI_AVALON_LIVENESS_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_AVALON_LIVENESS_ACTIVITY,
            HDMI_AVALON_LIVENESS_ACTIVITY_SCHEMA,
            &avalon[..HDMI_AVALON_LIVENESS_ACTIVITY_CRC_WORD],
        );
        let decoded = decode_hdmi_avalon_liveness_activity(&avalon).unwrap();
        assert_eq!(decoded.bucket_count(), 10);
        assert_eq!(decoded.request_count(), 7);
        assert_eq!(decoded.accepted_count(), 8);
        assert_eq!(decoded.returned_count(), 9);

        let mut fetch = zero_hdmi_words::<HDMI_SCALER_FETCH_ACTIVITY_WORDS>(
            GET_HDMI_SCALER_FETCH_ACTIVITY,
            HDMI_SCALER_FETCH_ACTIVITY_SCHEMA,
        );
        assert_eq!(
            fetch[HDMI_SCALER_FETCH_ACTIVITY_CRC_WORD],
            HDMI_SCALER_FETCH_ACTIVITY_ZERO_GOLDEN_CRC
        );
        fetch[HDMI_SCALER_FETCH_ACTIVITY_RESERVED_STATE_WORD] = 0;
        fetch[HDMI_SCALER_FETCH_ACTIVITY_EVENTS_WORD] = 0x00f3;
        fetch[HDMI_SCALER_FETCH_ACTIVITY_FLAGS_WORD] =
            HDMI_SCALER_FETCH_ACTIVITY_FLAG_SNAPSHOT_VALID;
        fetch[HDMI_SCALER_FETCH_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_SCALER_FETCH_ACTIVITY,
            HDMI_SCALER_FETCH_ACTIVITY_SCHEMA,
            &fetch[..HDMI_SCALER_FETCH_ACTIVITY_CRC_WORD],
        );
        let decoded = decode_hdmi_scaler_fetch_activity(&fetch).unwrap();
        assert_eq!(decoded.batch_two_count(), 3);
        assert_eq!(decoded.starved_frame_count(), 15);
    }

    #[test]
    fn hdmi_path_activity_records_reject_invalid_state() {
        let mut final_path = zero_hdmi_words::<HDMI_FINAL_PATH_ACTIVITY_WORDS>(
            GET_HDMI_FINAL_PATH_ACTIVITY,
            HDMI_FINAL_PATH_ACTIVITY_SCHEMA,
        );
        final_path[HDMI_FINAL_PATH_ACTIVITY_BLACK_COUNTS_WORD] = 1;
        final_path[HDMI_FINAL_PATH_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_FINAL_PATH_ACTIVITY,
            HDMI_FINAL_PATH_ACTIVITY_SCHEMA,
            &final_path[..HDMI_FINAL_PATH_ACTIVITY_CRC_WORD],
        );
        assert!(decode_hdmi_final_path_activity(&final_path).is_err());

        final_path[HDMI_FINAL_PATH_ACTIVITY_FLAGS_WORD] = HDMI_FINAL_PATH_ACTIVITY_FLAG_FRAME_VALID;
        final_path[HDMI_FINAL_PATH_ACTIVITY_ACTIVITY_COUNTS_WORD] = 0x0010;
        final_path[HDMI_FINAL_PATH_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_FINAL_PATH_ACTIVITY,
            HDMI_FINAL_PATH_ACTIVITY_SCHEMA,
            &final_path[..HDMI_FINAL_PATH_ACTIVITY_CRC_WORD],
        );
        assert!(decode_hdmi_final_path_activity(&final_path).is_err());

        let mut scaler = zero_hdmi_words::<HDMI_SCALER_RAW_ACTIVITY_WORDS>(
            GET_HDMI_SCALER_RAW_ACTIVITY,
            HDMI_SCALER_RAW_ACTIVITY_SCHEMA,
        );
        scaler[HDMI_SCALER_RAW_ACTIVITY_FLAGS_WORD] = HDMI_SCALER_RAW_ACTIVITY_FLAG_FRAME_VALID;
        scaler[HDMI_SCALER_RAW_ACTIVITY_COUNTS_WORD] = 0x1000;
        scaler[HDMI_SCALER_RAW_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_SCALER_RAW_ACTIVITY,
            HDMI_SCALER_RAW_ACTIVITY_SCHEMA,
            &scaler[..HDMI_SCALER_RAW_ACTIVITY_CRC_WORD],
        );
        assert!(decode_hdmi_scaler_raw_activity(&scaler).is_err());

        final_path[HDMI_FINAL_PATH_ACTIVITY_FLAGS_WORD] = 0x8000;
        final_path[HDMI_FINAL_PATH_ACTIVITY_BLACK_COUNTS_WORD] = 0;
        final_path[HDMI_FINAL_PATH_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_FINAL_PATH_ACTIVITY,
            HDMI_FINAL_PATH_ACTIVITY_SCHEMA,
            &final_path[..HDMI_FINAL_PATH_ACTIVITY_CRC_WORD],
        );
        assert!(decode_hdmi_final_path_activity(&final_path).is_err());

        let mut avalon = zero_hdmi_words::<HDMI_AVALON_LIVENESS_ACTIVITY_WORDS>(
            GET_HDMI_AVALON_LIVENESS_ACTIVITY,
            HDMI_AVALON_LIVENESS_ACTIVITY_SCHEMA,
        );
        avalon[HDMI_AVALON_LIVENESS_ACTIVITY_COUNTS_WORD] = 1;
        avalon[HDMI_AVALON_LIVENESS_ACTIVITY_CRC_WORD] = message_crc_with_schema(
            GET_HDMI_AVALON_LIVENESS_ACTIVITY,
            HDMI_AVALON_LIVENESS_ACTIVITY_SCHEMA,
            &avalon[..HDMI_AVALON_LIVENESS_ACTIVITY_CRC_WORD],
        );
        assert!(decode_hdmi_avalon_liveness_activity(&avalon).is_err());
        avalon[HDMI_AVALON_LIVENESS_ACTIVITY_FLAGS_WORD] =
            HDMI_AVALON_LIVENESS_ACTIVITY_FLAG_BUCKET_VALID;
        avalon[HDMI_AVALON_LIVENESS_ACTIVITY_CRC_WORD] ^= 1;
        assert!(decode_hdmi_avalon_liveness_activity(&avalon).is_err());

        let mut fetch = zero_hdmi_words::<HDMI_SCALER_FETCH_ACTIVITY_WORDS>(
            GET_HDMI_SCALER_FETCH_ACTIVITY,
            HDMI_SCALER_FETCH_ACTIVITY_SCHEMA,
        );
        fetch[HDMI_SCALER_FETCH_ACTIVITY_FLAGS_WORD] =
            HDMI_SCALER_FETCH_ACTIVITY_FLAG_SNAPSHOT_VALID;
        for bit in 0..16 {
            fetch[HDMI_SCALER_FETCH_ACTIVITY_RESERVED_STATE_WORD] = 1 << bit;
            fetch[HDMI_SCALER_FETCH_ACTIVITY_EVENTS_WORD] = 0;
            fetch[HDMI_SCALER_FETCH_ACTIVITY_CRC_WORD] = message_crc_with_schema(
                GET_HDMI_SCALER_FETCH_ACTIVITY,
                HDMI_SCALER_FETCH_ACTIVITY_SCHEMA,
                &fetch[..HDMI_SCALER_FETCH_ACTIVITY_CRC_WORD],
            );
            assert!(decode_hdmi_scaler_fetch_activity(&fetch).is_err());
        }
        fetch[HDMI_SCALER_FETCH_ACTIVITY_RESERVED_STATE_WORD] = 0;
        for bit in [2, 3, 8, 9, 10, 11, 12, 13, 14, 15] {
            fetch[HDMI_SCALER_FETCH_ACTIVITY_EVENTS_WORD] = 1 << bit;
            fetch[HDMI_SCALER_FETCH_ACTIVITY_CRC_WORD] = message_crc_with_schema(
                GET_HDMI_SCALER_FETCH_ACTIVITY,
                HDMI_SCALER_FETCH_ACTIVITY_SCHEMA,
                &fetch[..HDMI_SCALER_FETCH_ACTIVITY_CRC_WORD],
            );
            assert!(decode_hdmi_scaler_fetch_activity(&fetch).is_err());
        }
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
