//! Small bounded control envelopes; uploads use the streaming path.
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
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
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

pub fn write_frame(
    writer: &mut impl Write,
    header: &Envelope,
    body: &[u8],
) -> Result<(), FrameError> {
    let encoded =
        serde_json::to_vec(header).map_err(|error| FrameError::Json(error.to_string()))?;
    if encoded.len() > MAX_HEADER_BYTES {
        return Err(FrameError::HeaderTooLarge);
    }
    if body.len() > MAX_BODY_BYTES {
        return Err(FrameError::BodyTooLarge);
    }
    writer.write_all(&(encoded.len() as u32).to_be_bytes())?;
    writer.write_all(&(body.len() as u64).to_be_bytes())?;
    writer.write_all(&encoded)?;
    writer.write_all(body)?;
    Ok(())
}

pub fn read_header(reader: &mut impl Read) -> Result<(Envelope, usize), FrameError> {
    let mut lengths = [0; 12];
    reader.read_exact(&mut lengths)?;
    let header_length = u32::from_be_bytes(lengths[..4].try_into().expect("four bytes")) as usize;
    let body_length = usize::try_from(u64::from_be_bytes(
        lengths[4..].try_into().expect("eight bytes"),
    ))
    .map_err(|_| FrameError::BodyTooLarge)?;
    if header_length > MAX_HEADER_BYTES {
        return Err(FrameError::HeaderTooLarge);
    }
    if body_length > MAX_BODY_BYTES {
        return Err(FrameError::BodyTooLarge);
    }
    let mut header = vec![0; header_length];
    reader.read_exact(&mut header)?;
    let envelope = serde_json::from_slice(&header).map_err(|e| FrameError::Json(e.to_string()))?;
    Ok((envelope, body_length))
}

pub fn read_frame(reader: &mut impl Read) -> Result<(Envelope, Vec<u8>), FrameError> {
    let (header, length) = read_header(reader)?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok((header, body))
}

pub struct DeadlineReader<'a> {
    pub stream: &'a mut std::net::TcpStream,
    pub deadline: std::time::Instant,
}
impl Read for DeadlineReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let remaining = self
            .deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "request deadline elapsed"))?;
        self.stream
            .set_read_timeout(Some(remaining.min(std::time::Duration::from_secs(5))))?;
        self.stream.read(bytes)
    }
}
