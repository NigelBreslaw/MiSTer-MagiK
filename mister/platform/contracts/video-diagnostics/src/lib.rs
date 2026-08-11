mod generated;

pub use generated::*;

const CRC_POLYNOMIAL: u16 = 0x1021;
const CRC_INITIAL: u16 = 0xffff;

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
    let snapshot = decode(
        GET_VIDEO_DIAGNOSTICS_CONTROL,
        4,
        &[
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
        ],
        words,
    )?;
    if words[VIDEO_DIAGNOSTICS_CONTROL_MISSING_DOMAINS] & !0x0006 != 0
        || words[VIDEO_DIAGNOSTICS_CONTROL_ROUTE_FLAGS] & !VIDEO_DIAGNOSTICS_ROUTE_FLAGS_MASK != 0
        || words[VIDEO_DIAGNOSTICS_CONTROL_PRE_ROUTE_FLAGS] & !VIDEO_DIAGNOSTICS_ROUTE_FLAGS_MASK
            != 0
        || words[VIDEO_DIAGNOSTICS_CONTROL_POST_ROUTE_FLAGS] & !VIDEO_DIAGNOSTICS_ROUTE_FLAGS_MASK
            != 0
        || words[VIDEO_DIAGNOSTICS_CONTROL_CONTROL_FAULT_FLAGS]
            & !VIDEO_DIAGNOSTICS_CONTROL_FAULT_FLAGS_MASK
            != 0
        || words[VIDEO_DIAGNOSTICS_CONTROL_LEGACY_DISPOSITION]
            > VIDEO_DIAGNOSTICS_DISPOSITION_OVERLONG
    {
        return Err("control video diagnostics contains reserved bits".to_string());
    }
    Ok(snapshot)
}

pub fn decode_avalon(words: &[u16]) -> Result<VideoDiagnosticsAvalonSnapshot, String> {
    let snapshot = decode(
        GET_VIDEO_DIAGNOSTICS_AVALON,
        3,
        &[
            VIDEO_DIAGNOSTICS_TRIGGER_NONE,
            VIDEO_DIAGNOSTICS_TRIGGER_AVALON_ADDRESS,
            VIDEO_DIAGNOSTICS_TRIGGER_AVALON_BURST,
            VIDEO_DIAGNOSTICS_TRIGGER_AVALON_RETURN,
            VIDEO_DIAGNOSTICS_TRIGGER_AVALON_TIMEOUT,
            VIDEO_DIAGNOSTICS_TRIGGER_AVALON_NO_READS,
        ],
        words,
    )?;
    if words[VIDEO_DIAGNOSTICS_AVALON_ROUTE_FLAGS] & !VIDEO_DIAGNOSTICS_ROUTE_FLAGS_MASK != 0
        || words[VIDEO_DIAGNOSTICS_AVALON_FAULT_FLAGS] & !VIDEO_DIAGNOSTICS_AVALON_FAULT_FLAGS_MASK
            != 0
        || words[VIDEO_DIAGNOSTICS_AVALON_RESERVED] != 0
    {
        return Err("Avalon video diagnostics contains reserved bits".to_string());
    }
    Ok(snapshot)
}

pub fn decode_output(words: &[u16]) -> Result<VideoDiagnosticsOutputSnapshot, String> {
    let snapshot = decode(
        GET_VIDEO_DIAGNOSTICS_OUTPUT,
        3,
        &[
            VIDEO_DIAGNOSTICS_TRIGGER_NONE,
            VIDEO_DIAGNOSTICS_TRIGGER_FINAL_BLACK,
            VIDEO_DIAGNOSTICS_TRIGGER_FINAL_WHITE,
            VIDEO_DIAGNOSTICS_TRIGGER_FINAL_TIMING,
        ],
        words,
    )?;
    if words[VIDEO_DIAGNOSTICS_OUTPUT_SOURCE_FLAGS] & !VIDEO_DIAGNOSTICS_OUTPUT_SOURCE_FLAGS_MASK
        != 0
        || words[VIDEO_DIAGNOSTICS_OUTPUT_CONTROL_FLAGS]
            & !VIDEO_DIAGNOSTICS_OUTPUT_CONTROL_FLAGS_MASK
            != 0
        || words[VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS] & !VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_MASK
            != 0
        || words[VIDEO_DIAGNOSTICS_OUTPUT_GEOMETRY_FAULTS] & !VIDEO_DIAGNOSTICS_GEOMETRY_FAULTS_MASK
            != 0
        || words[VIDEO_DIAGNOSTICS_OUTPUT_RESERVED_0] != 0
        || words[VIDEO_DIAGNOSTICS_OUTPUT_RESERVED_1] != 0
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
        assert_eq!(control[47], VIDEO_DIAGNOSTICS_CONTROL_ZERO_GOLDEN_CRC);
        assert_eq!(avalon[31], VIDEO_DIAGNOSTICS_AVALON_ZERO_GOLDEN_CRC);
        assert_eq!(output[31], VIDEO_DIAGNOSTICS_OUTPUT_ZERO_GOLDEN_CRC);
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
    fn schema_crc_and_length_are_strict() {
        let mut words = zero_words::<VIDEO_DIAGNOSTICS_AVALON_WORDS>(GET_VIDEO_DIAGNOSTICS_AVALON);
        words[0] += 1;
        assert!(decode_avalon(&words).is_err());
        words[0] = VIDEO_DIAGNOSTICS_SCHEMA;
        words[5] ^= 1;
        assert!(decode_avalon(&words).is_err());
        assert!(decode_avalon(&words[..31]).is_err());

        let mut reserved =
            zero_words::<VIDEO_DIAGNOSTICS_OUTPUT_WORDS>(GET_VIDEO_DIAGNOSTICS_OUTPUT);
        reserved[1] = 0x8000;
        reserved[31] = message_crc(GET_VIDEO_DIAGNOSTICS_OUTPUT, &reserved[..31]);
        assert!(decode_output(&reserved).is_err());
        reserved[1] = 0;
        reserved[2] = VIDEO_DIAGNOSTICS_TRIGGER_AVALON_BURST;
        reserved[31] = message_crc(GET_VIDEO_DIAGNOSTICS_OUTPUT, &reserved[..31]);
        assert!(decode_output(&reserved).is_err());

        let mut reserved_avalon =
            zero_words::<VIDEO_DIAGNOSTICS_AVALON_WORDS>(GET_VIDEO_DIAGNOSTICS_AVALON);
        reserved_avalon[VIDEO_DIAGNOSTICS_AVALON_RESERVED] = 1;
        reserved_avalon[VIDEO_DIAGNOSTICS_AVALON_CRC] = message_crc(
            GET_VIDEO_DIAGNOSTICS_AVALON,
            &reserved_avalon[..VIDEO_DIAGNOSTICS_AVALON_CRC],
        );
        assert!(decode_avalon(&reserved_avalon).is_err());
    }
}
