//! Bounded native control framing for the independently owned MagiK 2.0 agent.

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_BODY_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Envelope {
    pub id: String,
    pub op: String,
    pub token: String,
    #[serde(flatten)]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    Io(String),
    HeaderTooLarge,
    BodyTooLarge,
    Json(String),
}

impl From<io::Error> for FrameError {
    fn from(error: io::Error) -> Self { Self::Io(error.to_string()) }
}

pub fn write_frame(writer: &mut impl Write, header: &Envelope, body: &[u8]) -> Result<(), FrameError> {
    let encoded = serde_json::to_vec(header).map_err(|error| FrameError::Json(error.to_string()))?;
    if encoded.len() > MAX_HEADER_BYTES { return Err(FrameError::HeaderTooLarge); }
    if body.len() > MAX_BODY_BYTES { return Err(FrameError::BodyTooLarge); }
    writer.write_all(&(encoded.len() as u32).to_be_bytes())?;
    writer.write_all(&(body.len() as u64).to_be_bytes())?;
    writer.write_all(&encoded)?;
    writer.write_all(body)?;
    Ok(())
}

pub fn read_frame(reader: &mut impl Read) -> Result<(Envelope, Vec<u8>), FrameError> {
    let mut lengths = [0_u8; 12];
    reader.read_exact(&mut lengths)?;
    let header_length = u32::from_be_bytes(lengths[..4].try_into().expect("four bytes")) as usize;
    let body_length = u64::from_be_bytes(lengths[4..].try_into().expect("eight bytes")) as usize;
    if header_length > MAX_HEADER_BYTES { return Err(FrameError::HeaderTooLarge); }
    if body_length > MAX_BODY_BYTES { return Err(FrameError::BodyTooLarge); }
    let mut header = vec![0; header_length];
    let mut body = vec![0; body_length];
    reader.read_exact(&mut header)?;
    reader.read_exact(&mut body)?;
    let envelope = serde_json::from_slice(&header).map_err(|error| FrameError::Json(error.to_string()))?;
    Ok((envelope, body))
}

#[derive(Debug, Serialize)]
pub struct Status<'a> {
    pub identity: &'a str,
    pub capabilities: &'a [&'a str],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_binary_payload() {
        let envelope = Envelope { id: "one".into(), op: "upload".into(), token: "token".into(), fields: serde_json::Map::new() };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &envelope, &[0, 255]).expect("write frame");
        assert_eq!(read_frame(&mut bytes.as_slice()).expect("read frame"), (envelope, vec![0, 255]));
    }

    #[test]
    fn rejects_truncated_payload() {
        let error = read_frame(&mut &b"\0\0\0\x02\0\0\0\0\0\0\0\x04{}x"[..]).expect_err("truncated body");
        assert!(matches!(error, FrameError::Io(_)));
    }
}
