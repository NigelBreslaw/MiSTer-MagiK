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

#[cfg(test)]
mod desktop_fixture {
    use super::*;
    #[test]
    fn accepts_shared_desktop_request() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/desktop-wire.json")).unwrap();
        let bytes = fixture["bytes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_u64().unwrap() as u8)
            .collect::<Vec<_>>();
        let (header, body) = read_frame(&mut bytes.as_slice()).unwrap();
        assert_eq!(header.op, "status");
        assert_eq!(header.id, "fixture");
        assert!(body.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rejects_oversized_declarations_before_reading_or_allocating_the_payload() {
        for (header, body, expected) in [
            (MAX_HEADER_BYTES as u32 + 1, 0, FrameError::HeaderTooLarge),
            (1, MAX_BODY_BYTES as u64 + 1, FrameError::BodyTooLarge),
            (1, u64::MAX, FrameError::BodyTooLarge),
        ] {
            let mut bytes = header.to_be_bytes().to_vec();
            bytes.extend_from_slice(&body.to_be_bytes());
            let mut reader = Cursor::new(bytes);
            assert_eq!(read_header(&mut reader), Err(expected));
            assert_eq!(reader.position(), 12);
        }
    }

    #[test]
    fn nonempty_frames_round_trip_and_truncation_is_rejected_at_each_boundary() {
        let envelope = Envelope {
            id: "request".into(),
            op: "artifact".into(),
            token: "fixture".into(),
            fields: serde_json::Map::from_iter([("extra".into(), serde_json::json!({"value":7}))]),
        };
        let body = b"\0binary\xffpayload";
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &envelope, body).unwrap();
        let mut stream = bytes.clone();
        stream.extend_from_slice(&bytes);
        let mut reader = Cursor::new(stream);
        for _ in 0..2 {
            assert_eq!(
                read_frame(&mut reader).unwrap(),
                (envelope.clone(), body.to_vec())
            );
        }
        assert_eq!(reader.position() as usize, bytes.len() * 2);
        for length in [0, 11, 12, bytes.len() - body.len() - 1, bytes.len() - 1] {
            assert!(matches!(
                read_frame(&mut &bytes[..length]),
                Err(FrameError::Io(_))
            ));
        }
    }

    #[test]
    fn malformed_or_incomplete_envelopes_are_rejected() {
        for json in [b"{broken}".as_slice(), b"{}", b"[]"] {
            let mut bytes = (json.len() as u32).to_be_bytes().to_vec();
            bytes.extend_from_slice(&0_u64.to_be_bytes());
            bytes.extend_from_slice(json);
            assert!(matches!(
                read_header(&mut bytes.as_slice()),
                Err(FrameError::Json(_))
            ));
        }
    }
}
