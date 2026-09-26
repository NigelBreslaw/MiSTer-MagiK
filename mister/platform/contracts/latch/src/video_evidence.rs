//! Read-only video evidence schemas, independent of latch protocol version 5.
use crate::crc16_update_word;

pub const SELECT_FIRST: u16 = 0x68;
pub const SELECT_LIVE: u16 = 0x69;
pub const READ_SNAPSHOT: u16 = 0x6a;
pub const SCHEMA: u16 = 24;
pub const WORDS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub schema: u16,
    pub record_valid: bool,
    pub first_selected: bool,
    pub ledger_valid: bool,
    pub cause: u8,
    pub physical_depth: u8,
    pub physical_phase: u8,
    pub production_depth: Option<u8>,
    pub production_phase: Option<u8>,
    pub output_state: Option<u16>,
    pub flags: u16,
}

pub fn crc(command: u16, schema: u16, words: &[u16]) -> u16 {
    [command, schema, words.len() as u16]
        .into_iter()
        .chain(words.iter().copied())
        .fold(0xffff, crc16_update_word)
}

pub fn decode(words: &[u16]) -> Result<Evidence, &'static str> {
    let schema = *words.first().ok_or("empty evidence")?;
    let (command, length) = match schema {
        23 => (SELECT_FIRST, 4),
        SCHEMA => (READ_SNAPSHOT, WORDS),
        _ => return Err("unsupported evidence schema"),
    };
    if words.len() != length {
        return Err("wrong evidence length");
    }
    if crc(command, schema, &words[..length - 1]) != words[length - 1] {
        return Err("evidence CRC mismatch");
    }
    let flags = words[1];
    if schema == 23 {
        // Only observer-fault terminal records have this frozen phase/depth layout.
        if flags & 8 == 0 {
            return Err("legacy non-observer terminal layout unsupported");
        }
        if flags & 0xff00 != 0 || words[2] & 0xf000 != 0 {
            return Err("legacy reserved bits set");
        }
        return Ok(Evidence {
            schema,
            record_valid: flags & 1 != 0,
            first_selected: true,
            ledger_valid: false,
            cause: ((words[2] & 7) + 6) as u8,
            physical_depth: ((words[2] >> 10) & 3) as u8,
            physical_phase: ((words[2] >> 3) & 127) as u8,
            production_depth: None,
            production_phase: None,
            output_state: None,
            flags,
        });
    }
    let cause = ((flags >> 8) & 15) as u8;
    if cause > 9 {
        return Err("reserved evidence cause");
    }
    let physical_depth = ((words[2] >> 7) & 3) as u8;
    let physical_phase = (words[2] & 127) as u8;
    if flags & 0x1000 != 0 && physical_depth == 0 && physical_phase != 0 {
        return Err("impossible valid ledger phase");
    }
    if flags & 0x4000 == 0 && words[3] != 0 {
        return Err("invalid output payload is nonzero");
    }
    Ok(Evidence {
        schema,
        record_valid: flags & 0x8000 != 0,
        first_selected: flags & 0x2000 != 0,
        ledger_valid: flags & 0x1000 != 0,
        cause,
        physical_depth,
        physical_phase,
        production_depth: Some((flags & 3) as u8),
        production_phase: Some((words[2] >> 9) as u8),
        output_state: (flags & 0x4000 != 0).then_some(words[3]),
        flags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incident_schema23_is_observer_invalid_with_frozen_phase_45() {
        let evidence = decode(&[23, 75, 2409, 15306]).unwrap();
        assert_eq!(
            (
                evidence.cause,
                evidence.physical_depth,
                evidence.physical_phase
            ),
            (7, 2, 45)
        );
        assert!(!evidence.ledger_valid);
        assert_eq!(evidence.production_phase, None);
    }
    #[test]
    fn independently_captured_schema24_and_stopped_clock_records() {
        let e = decode(&[24, 0xf502, 0x5b2d, 0x531a, 0x41de]).unwrap();
        assert_eq!((e.cause, e.physical_depth, e.physical_phase), (5, 2, 45));
        assert_eq!(e.production_phase, Some(45));
        assert!(e.first_selected && e.ledger_valid);
        assert_eq!(
            decode(&[24, 0xb502, 0x5b2d, 0, 0xc359])
                .unwrap()
                .output_state,
            None
        );
    }
    #[test]
    fn corrupted_unknown_and_partial_records_fail_closed() {
        assert!(decode(&[24, 0xf502, 0x5b2d, 0x531a, 0x41df]).is_err());
        assert!(decode(&[25, 0, 0, 0, 0]).is_err());
        assert!(decode(&[24, 0, 0, 0]).is_err());
    }
}
