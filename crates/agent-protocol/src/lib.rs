// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::{Value, json};

pub const PORT: u16 = 7498;
// Version 15 decodes coherent cross-domain FPGA diagnostic freeze triggers.
pub const AGENT_VERSION: u64 = 15;
pub const PROTOCOL_VERSION: u64 = 2;
pub const FRAMEBUFFER_CAPTURE_CAPABILITY: &str = "framebuffer-capture-v2";
pub const DEVICE_TELEMETRY_CAPABILITY: &str = "device-telemetry-v2";
pub const LAUNCHER_AUTOMATION_CAPABILITY: &str = "launcher-automation-v1";
pub const ALPHA_CANDIDATE_INSTALL_CAPABILITY: &str = "alpha-candidate-install-v1";
pub const SCREENSAVER_FRAME_EVIDENCE_CAPABILITY: &str = "screensaver-frame-evidence-v6";
pub const MAX_BINARY_PAYLOAD_BYTES: u64 = 512 * 1024 * 1024;

pub fn request(token: &str, id: u64, command: &str, args: Value) -> Value {
    json!({ "token": token, "id": id, "cmd": command, "args": args })
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResponseEnvelope {
    Ok { full: Value, result: Value },
    Error(String),
}

pub fn parse_response_line(line: &str) -> Result<ResponseEnvelope, String> {
    if line.trim().is_empty() {
        return Err("empty response from agent".to_string());
    }
    let full: Value = serde_json::from_str(line.trim())
        .map_err(|error| format!("invalid JSON response: {error}"))?;
    if full.get("ok").and_then(Value::as_bool) == Some(true) {
        let result = full.get("result").cloned().unwrap_or(Value::Null);
        Ok(ResponseEnvelope::Ok { full, result })
    } else {
        Ok(ResponseEnvelope::Error(
            full.get("error")
                .and_then(Value::as_str)
                .unwrap_or("agent command failed")
                .to_string(),
        ))
    }
}

pub fn binary_payload_len(value: &Value) -> Result<usize, String> {
    let payload_bytes = value
        .pointer("/result/payload_bytes")
        .or_else(|| value.pointer("/result/raw_bytes"))
        .or_else(|| value.pointer("/payload_bytes"))
        .or_else(|| value.pointer("/raw_bytes"))
        .and_then(Value::as_u64)
        .ok_or_else(|| "binary response missing payload byte count".to_string())?;
    if payload_bytes > MAX_BINARY_PAYLOAD_BYTES {
        return Err(format!(
            "binary response payload too large: {payload_bytes} bytes"
        ));
    }
    usize::try_from(payload_bytes)
        .map_err(|_| format!("binary response payload size overflows usize: {payload_bytes}"))
}

pub fn decompress_size_prepended_exact(
    payload: &[u8],
    expected_raw: usize,
    max_raw: usize,
) -> Result<Vec<u8>, String> {
    if expected_raw > max_raw {
        return Err(format!(
            "decoded payload too large: {expected_raw} bytes (max {max_raw})"
        ));
    }
    let declared_raw = payload
        .get(..4)
        .and_then(|prefix| prefix.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| "decode lz4 payload: missing size prefix".to_string())?;
    let declared_raw = usize::try_from(declared_raw)
        .map_err(|_| format!("decoded payload size overflows usize: {declared_raw}"))?;
    if declared_raw > max_raw {
        return Err(format!(
            "decoded payload too large: {declared_raw} bytes (max {max_raw})"
        ));
    }
    if declared_raw != expected_raw {
        return Err(format!(
            "decoded payload size mismatch: expected {expected_raw}, got {declared_raw}"
        ));
    }
    let raw = lz4_flex::decompress_size_prepended(payload)
        .map_err(|error| format!("decode lz4 payload: {error}"))?;
    if raw.len() != expected_raw {
        return Err(format!(
            "decoded payload size mismatch: expected {expected_raw}, got {}",
            raw.len()
        ));
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_cover_success_error_and_malformed_lines() {
        assert!(matches!(
            parse_response_line(r#"{"ok":true,"result":{"x":1}}"#).unwrap(),
            ResponseEnvelope::Ok { .. }
        ));
        assert_eq!(
            parse_response_line(r#"{"ok":false,"error":"nope"}"#).unwrap(),
            ResponseEnvelope::Error("nope".to_string())
        );
        assert_eq!(
            parse_response_line(r#"{"ok":true}"#).unwrap(),
            ResponseEnvelope::Ok {
                full: json!({"ok": true}),
                result: Value::Null,
            }
        );
        assert_eq!(
            parse_response_line(r#"{"ok":false}"#).unwrap(),
            ResponseEnvelope::Error("agent command failed".to_string())
        );
        assert!(parse_response_line("").is_err());
        assert!(parse_response_line("  \n\t").is_err());
        assert!(parse_response_line("{").is_err());
    }

    #[test]
    fn binary_lengths_are_bounded_at_both_envelope_shapes() {
        assert_eq!(binary_payload_len(&json!({"payload_bytes": 4})).unwrap(), 4);
        assert_eq!(
            binary_payload_len(&json!({"result": {"raw_bytes": 5}})).unwrap(),
            5
        );
        assert!(
            binary_payload_len(&json!({"payload_bytes": MAX_BINARY_PAYLOAD_BYTES + 1})).is_err()
        );
        assert!(binary_payload_len(&json!({})).is_err());
        assert!(binary_payload_len(&json!({"payload_bytes": "4"})).is_err());
    }

    #[test]
    fn exact_lz4_decode_rejects_size_disagreement() {
        let encoded = lz4_flex::compress_prepend_size(b"hello");
        assert_eq!(
            decompress_size_prepended_exact(&encoded, 5, 10).unwrap(),
            b"hello"
        );
        assert!(decompress_size_prepended_exact(&encoded, 4, 10).is_err());
        assert!(decompress_size_prepended_exact(&encoded, 5, 4).is_err());
        assert!(decompress_size_prepended_exact(&[], 0, 10).is_err());
        assert!(decompress_size_prepended_exact(&[0, 0, 0, 0], 0, 10).is_err());
    }

    #[test]
    fn exact_lz4_decode_rejects_oversized_embedded_size_before_decompression() {
        let payload = (11_u32).to_le_bytes();
        let error = decompress_size_prepended_exact(&payload, 5, 10).unwrap_err();

        assert!(error.contains("too large"));
    }
}
