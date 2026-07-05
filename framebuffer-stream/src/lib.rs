use std::io::{Read, Write};

pub const SCHEMA: &str = "mister-magik-framebuffer-stream-v1";
pub const MAGIC: &[u8; 8] = b"MMFSv1\0\0";
pub const HEADER_LEN: usize = 72;
pub const FLAG_LZ4_SIZE_PREPENDED: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FrameKind {
    Hello = 1,
    Keyframe = 2,
    RectDelta = 3,
    Heartbeat = 4,
    End = 5,
    Error = 6,
}

impl FrameKind {
    pub fn from_u8(value: u8) -> Result<Self, FrameStreamError> {
        match value {
            1 => Ok(Self::Hello),
            2 => Ok(Self::Keyframe),
            3 => Ok(Self::RectDelta),
            4 => Ok(Self::Heartbeat),
            5 => Ok(Self::End),
            6 => Ok(Self::Error),
            other => Err(FrameStreamError::UnknownKind(other)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameGeometry {
    pub width: u32,
    pub height: u32,
    pub stride_pixels: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl FrameRect {
    pub fn full(geometry: FrameGeometry) -> Self {
        Self {
            x: 0,
            y: 0,
            width: geometry.width,
            height: geometry.height,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameHeader {
    pub kind: FrameKind,
    pub flags: u16,
    pub sequence: u64,
    pub timestamp_us: u64,
    pub geometry: FrameGeometry,
    pub rect: FrameRect,
    pub raw_bytes: u32,
    pub payload_bytes: u32,
}

impl FrameHeader {
    pub fn encode(self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..8].copy_from_slice(MAGIC);
        out[8] = self.kind as u8;
        write_u16(&mut out[10..12], self.flags);
        write_u16(&mut out[12..14], HEADER_LEN as u16);
        write_u64(&mut out[16..24], self.sequence);
        write_u64(&mut out[24..32], self.timestamp_us);
        write_u32(&mut out[32..36], self.geometry.width);
        write_u32(&mut out[36..40], self.geometry.height);
        write_u32(&mut out[40..44], self.geometry.stride_pixels);
        write_u32(&mut out[44..48], self.rect.x);
        write_u32(&mut out[48..52], self.rect.y);
        write_u32(&mut out[52..56], self.rect.width);
        write_u32(&mut out[56..60], self.rect.height);
        write_u32(&mut out[60..64], self.raw_bytes);
        write_u32(&mut out[64..68], self.payload_bytes);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FrameStreamError> {
        if bytes.len() != HEADER_LEN {
            return Err(FrameStreamError::BadHeaderLen(bytes.len()));
        }
        if &bytes[0..8] != MAGIC {
            return Err(FrameStreamError::BadMagic);
        }
        let header_len = read_u16(&bytes[12..14]) as usize;
        if header_len != HEADER_LEN {
            return Err(FrameStreamError::BadHeaderLen(header_len));
        }
        Ok(Self {
            kind: FrameKind::from_u8(bytes[8])?,
            flags: read_u16(&bytes[10..12]),
            sequence: read_u64(&bytes[16..24]),
            timestamp_us: read_u64(&bytes[24..32]),
            geometry: FrameGeometry {
                width: read_u32(&bytes[32..36]),
                height: read_u32(&bytes[36..40]),
                stride_pixels: read_u32(&bytes[40..44]),
            },
            rect: FrameRect {
                x: read_u32(&bytes[44..48]),
                y: read_u32(&bytes[48..52]),
                width: read_u32(&bytes[52..56]),
                height: read_u32(&bytes[56..60]),
            },
            raw_bytes: read_u32(&bytes[60..64]),
            payload_bytes: read_u32(&bytes[64..68]),
        })
    }

    pub fn validate_shape(self) -> Result<(), FrameStreamError> {
        if self.geometry.width == 0
            || self.geometry.height == 0
            || self.geometry.stride_pixels < self.geometry.width
        {
            return Err(FrameStreamError::BadGeometry);
        }
        if self.rect.width == 0 || self.rect.height == 0 {
            return Err(FrameStreamError::BadRect);
        }
        let rect_right = self
            .rect
            .x
            .checked_add(self.rect.width)
            .ok_or(FrameStreamError::BadRect)?;
        let rect_bottom = self
            .rect
            .y
            .checked_add(self.rect.height)
            .ok_or(FrameStreamError::BadRect)?;
        if rect_right > self.geometry.width || rect_bottom > self.geometry.height {
            return Err(FrameStreamError::BadRect);
        }
        let expected_raw = self
            .rect
            .width
            .checked_mul(self.rect.height)
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or(FrameStreamError::PayloadTooLarge)?;
        if self.raw_bytes != expected_raw {
            return Err(FrameStreamError::BadPayloadLen {
                expected: expected_raw,
                actual: self.raw_bytes,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum FrameStreamError {
    BadMagic,
    BadHeaderLen(usize),
    UnknownKind(u8),
    BadGeometry,
    BadRect,
    PayloadTooLarge,
    BadPayloadLen { expected: u32, actual: u32 },
}

impl std::fmt::Display for FrameStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "bad framebuffer stream magic"),
            Self::BadHeaderLen(len) => write!(f, "bad framebuffer stream header length: {len}"),
            Self::UnknownKind(kind) => write!(f, "unknown framebuffer stream kind: {kind}"),
            Self::BadGeometry => write!(f, "bad framebuffer stream geometry"),
            Self::BadRect => write!(f, "bad framebuffer stream rect"),
            Self::PayloadTooLarge => write!(f, "framebuffer stream payload too large"),
            Self::BadPayloadLen { expected, actual } => {
                write!(f, "bad framebuffer stream payload length expected={expected} actual={actual}")
            }
        }
    }
}

impl std::error::Error for FrameStreamError {}

pub fn write_frame<W: Write>(
    writer: &mut W,
    header: FrameHeader,
    payload: &[u8],
) -> std::io::Result<()> {
    writer.write_all(&header.encode())?;
    writer.write_all(payload)
}

pub fn read_frame<R: Read>(reader: &mut R) -> std::io::Result<(FrameHeader, Vec<u8>)> {
    let mut header_bytes = [0u8; HEADER_LEN];
    reader.read_exact(&mut header_bytes)?;
    let header = FrameHeader::decode(&header_bytes)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let mut payload = vec![0u8; header.payload_bytes as usize];
    reader.read_exact(&mut payload)?;
    Ok((header, payload))
}

fn write_u16(dst: &mut [u8], value: u16) {
    dst.copy_from_slice(&value.to_le_bytes());
}

fn write_u32(dst: &mut [u8], value: u32) {
    dst.copy_from_slice(&value.to_le_bytes());
}

fn write_u64(dst: &mut [u8], value: u64) {
    dst.copy_from_slice(&value.to_le_bytes());
}

fn read_u16(src: &[u8]) -> u16 {
    u16::from_le_bytes(src.try_into().expect("u16 slice"))
}

fn read_u32(src: &[u8]) -> u32 {
    u32::from_le_bytes(src.try_into().expect("u32 slice"))
}

fn read_u64(src: &[u8]) -> u64 {
    u64::from_le_bytes(src.try_into().expect("u64 slice"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyframe_header() -> FrameHeader {
        let geometry = FrameGeometry {
            width: 960,
            height: 540,
            stride_pixels: 960,
        };
        FrameHeader {
            kind: FrameKind::Keyframe,
            flags: FLAG_LZ4_SIZE_PREPENDED,
            sequence: 42,
            timestamp_us: 123_456,
            geometry,
            rect: FrameRect::full(geometry),
            raw_bytes: 960 * 540 * 2,
            payload_bytes: 100,
        }
    }

    #[test]
    fn frame_header_round_trips() {
        let header = keyframe_header();
        let decoded = FrameHeader::decode(&header.encode()).expect("header should decode");

        assert_eq!(decoded, header);
        decoded.validate_shape().expect("shape should validate");
    }

    #[test]
    fn frame_header_rejects_bad_magic() {
        let mut bytes = keyframe_header().encode();
        bytes[0] = b'?';

        assert_eq!(FrameHeader::decode(&bytes), Err(FrameStreamError::BadMagic));
    }

    #[test]
    fn frame_header_validates_rect_payload_size() {
        let mut header = keyframe_header();
        header.rect = FrameRect {
            x: 10,
            y: 20,
            width: 3,
            height: 4,
        };
        header.raw_bytes = 24;
        header.validate_shape().expect("rect payload should match");
        header.raw_bytes = 22;

        assert_eq!(
            header.validate_shape(),
            Err(FrameStreamError::BadPayloadLen {
                expected: 24,
                actual: 22
            })
        );
    }

    #[test]
    fn read_write_frame_round_trips_payload() {
        let header = keyframe_header();
        let payload = [1, 2, 3, 4];
        let mut wire = Vec::new();
        write_frame(
            &mut wire,
            FrameHeader {
                payload_bytes: payload.len() as u32,
                raw_bytes: 960 * 540 * 2,
                ..header
            },
            &payload,
        )
        .expect("write frame");

        let (decoded, decoded_payload) = read_frame(&mut wire.as_slice()).expect("read frame");

        assert_eq!(decoded.payload_bytes, payload.len() as u32);
        assert_eq!(decoded_payload, payload);
    }
}
