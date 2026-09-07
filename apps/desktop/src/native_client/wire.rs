//! The native 12-byte length prefix; no legacy line protocol.
use super::AgentError;
use serde_json::{Value, json};
use std::io::{Read, Write};
pub const MAX_HEADER: usize = 64 * 1024;
pub const MAX_BODY: usize = 64 * 1024 * 1024;
pub fn write(
    writer: &mut impl Write,
    id: &str,
    op: &str,
    token: &str,
    args: Value,
) -> Result<(), AgentError> {
    let mut header = json!({"id":id,"op":op,"token":token});
    let fields = args
        .as_object()
        .ok_or_else(|| AgentError::Protocol("request fields must be an object".into()))?;
    for (key, value) in fields {
        if matches!(key.as_str(), "id" | "op" | "token") {
            return Err(AgentError::Protocol("reserved request field".into()));
        }
        header[key] = value.clone();
    }
    let bytes = serde_json::to_vec(&header).map_err(|e| AgentError::Protocol(e.to_string()))?;
    if bytes.len() > MAX_HEADER {
        return Err(AgentError::Protocol("header too large".into()));
    }
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&0u64.to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}
pub fn read(
    reader: &mut impl Read,
    id: &str,
    expected: &str,
) -> Result<(Value, Vec<u8>), AgentError> {
    let mut lengths = [0; 12];
    reader.read_exact(&mut lengths)?;
    let header_len = u32::from_be_bytes(lengths[..4].try_into().unwrap()) as usize;
    let body_len = u64::from_be_bytes(lengths[4..].try_into().unwrap());
    if header_len > MAX_HEADER || body_len > MAX_BODY as u64 {
        return Err(AgentError::Protocol(
            "native response exceeds size limit".into(),
        ));
    }
    let mut header = vec![0; header_len];
    reader.read_exact(&mut header)?;
    let value: Value =
        serde_json::from_slice(&header).map_err(|e| AgentError::Protocol(e.to_string()))?;
    if value["id"] != id {
        return Err(AgentError::Protocol("response ID mismatch".into()));
    }
    if value["op"] == "error" {
        if value["code"] == "authentication-failed" {
            return Err(AgentError::Unauthorized);
        }
        return Err(AgentError::Command(format!(
            "{}: {}",
            value["code"].as_str().unwrap_or("error"),
            value["detail"].as_str().unwrap_or("request failed")
        )));
    }
    if value["op"] != expected {
        return Err(AgentError::Protocol(format!(
            "unexpected reply to {expected}"
        )));
    }
    let mut body = vec![0; body_len as usize];
    reader.read_exact(&mut body)?;
    Ok((value, body))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frame(header: Value, body: &[u8]) -> Vec<u8> {
        let bytes = serde_json::to_vec(&header).unwrap();
        let mut out = Vec::new();
        out.extend((bytes.len() as u32).to_be_bytes());
        out.extend((body.len() as u64).to_be_bytes());
        out.extend(bytes);
        out.extend(body);
        out
    }
    #[test]
    fn accepts_binary_and_classifies_failed_mismatched_truncated_responses() {
        let good = frame(json!({"id":"1","op":"image"}), b"pixels");
        assert_eq!(
            read(&mut good.as_slice(), "1", "image").unwrap().1,
            b"pixels"
        );
        assert!(read(&mut good.as_slice(), "2", "image").is_err());
        assert!(read(&mut good.as_slice(), "1", "other").is_err());
        assert!(read(&mut &good[..good.len() - 1], "1", "image").is_err());
        let auth = frame(
            json!({"id":"1","op":"error","code":"authentication-failed"}),
            b"",
        );
        assert!(matches!(
            read(&mut auth.as_slice(), "1", "image"),
            Err(AgentError::Unauthorized)
        ));
        let mut oversized = vec![0; 12];
        oversized[..4].copy_from_slice(&((MAX_HEADER + 1) as u32).to_be_bytes());
        assert!(matches!(
            read(&mut oversized.as_slice(), "1", "image"),
            Err(AgentError::Protocol(_))
        ));
    }
    #[test]
    fn request_codec_matches_native_fixture() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../magik/agent/tests/fixtures/desktop-wire.json"
        ))
        .unwrap();
        let bytes = fixture["bytes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_u64().unwrap() as u8)
            .collect::<Vec<_>>();
        let mut written = Vec::new();
        write(&mut written, "fixture", "status", "fixture", json!({})).unwrap();
        // JSON object field ordering is not part of the wire contract.
        assert_eq!(&written[..12], &bytes[..12]);
        assert_eq!(
            serde_json::from_slice::<Value>(&written[12..]).unwrap(),
            serde_json::from_slice::<Value>(&bytes[12..]).unwrap()
        );
    }
}
