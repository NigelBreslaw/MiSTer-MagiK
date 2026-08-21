// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::{Value, json};

pub const PORT: u16 = 7498;
// Version 26 permits the closed launcher-automation bridge to hold one logical
// input for the fixed 40-second Arcade velocity-scroll benchmark.
pub const AGENT_VERSION: u64 = 26;
pub const PROTOCOL_VERSION: u64 = 2;
pub const FRAMEBUFFER_CAPTURE_CAPABILITY: &str = "framebuffer-capture-v2";
pub const DEVICE_TELEMETRY_CAPABILITY: &str = "device-telemetry-v2";
pub const LAUNCHER_AUTOMATION_CAPABILITY: &str = "launcher-automation-v1";
pub const LAUNCHER_AUTOMATION_MAX_HOLD_MS: u64 = 40_000;
pub const ALPHA_CANDIDATE_INSTALL_CAPABILITY: &str = "alpha-candidate-install-v1";
pub const SCREENSAVER_FRAME_EVIDENCE_CAPABILITY: &str = "screensaver-frame-evidence-v6";
pub const RUNTIME_UPLOAD_COMMAND: &str = "runtime_upload_v1";
pub const RUNTIME_UPLOAD_CAPABILITY: &str = "runtime-upload-v1";
pub const RUNTIME_UPLOAD_SCHEMA: &str = "mister-magik-runtime-upload-v1";
pub const MAX_RUNTIME_UPLOAD_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_BINARY_PAYLOAD_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeUploadSpec {
    pub payload_bytes: u64,
    pub sha256: String,
}

impl RuntimeUploadSpec {
    pub fn from_args(args: &Value) -> Result<Self, String> {
        let object = args
            .as_object()
            .ok_or_else(|| "runtime upload args must be an object".to_string())?;
        if object.len() != 2
            || !object.contains_key("payload_bytes")
            || !object.contains_key("sha256")
        {
            return Err("runtime upload args require exactly payload_bytes and sha256".to_string());
        }
        let payload_bytes = object["payload_bytes"].as_u64().ok_or_else(|| {
            "runtime upload payload_bytes must be an unsigned integer".to_string()
        })?;
        if payload_bytes == 0 || payload_bytes > MAX_RUNTIME_UPLOAD_BYTES {
            return Err(format!(
                "runtime upload payload_bytes must be between 1 and {MAX_RUNTIME_UPLOAD_BYTES}"
            ));
        }
        let sha256 = object["sha256"]
            .as_str()
            .ok_or_else(|| "runtime upload sha256 must be a string".to_string())?;
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(
                "runtime upload sha256 must be 64 lowercase hexadecimal characters".to_string(),
            );
        }
        Ok(Self {
            payload_bytes,
            sha256: sha256.to_string(),
        })
    }

    pub fn args(&self) -> Value {
        json!({
            "payload_bytes": self.payload_bytes,
            "sha256": self.sha256,
        })
    }
}

pub fn request(token: &str, id: u64, command: &str, args: Value) -> Value {
    json!({ "token": token, "id": id, "cmd": command, "args": args })
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResponseEnvelope {
    Ok {
        full: Value,
        result: Value,
    },
    Error(String),
    ErrorWithFailure {
        error: String,
        failure: FailureMetadata,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FailureCode {
    UnknownCommand,
    InvalidRequest,
    AuthenticationRequired,
    AccessDenied,
    DeviceBusy,
    DeviceUnavailable,
    ArtifactMismatch,
    OperationFailed,
    Cancelled,
    RecoveryRequired,
    Unknown(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FailurePhase {
    Request,
    Authentication,
    Availability,
    Artifact,
    Operation,
    Recovery,
    Unknown(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RetryPolicy {
    Never,
    Retry,
    ReconcileThenRetry,
    OperatorRequired,
    Unknown(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailureMetadata {
    pub code: FailureCode,
    pub detail: String,
    pub phase: FailurePhase,
    pub retry_policy: RetryPolicy,
    pub recovery_required: bool,
}

macro_rules! wire_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        impl $name {
            pub fn parse(value: &str) -> Self {
                match value {
                    $($value => Self::$variant,)+
                    other => Self::Unknown(other.to_owned()),
                }
            }

            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $value,)+
                    Self::Unknown(value) => value,
                }
            }
        }
    };
}

wire_enum!(FailureCode {
    UnknownCommand => "unknown_command",
    InvalidRequest => "invalid_request",
    AuthenticationRequired => "authentication_required",
    AccessDenied => "access_denied",
    DeviceBusy => "device_busy",
    DeviceUnavailable => "device_unavailable",
    ArtifactMismatch => "artifact_mismatch",
    OperationFailed => "operation_failed",
    Cancelled => "cancelled",
    RecoveryRequired => "recovery_required",
});
wire_enum!(FailurePhase {
    Request => "request",
    Authentication => "authentication",
    Availability => "availability",
    Artifact => "artifact",
    Operation => "operation",
    Recovery => "recovery",
});
wire_enum!(RetryPolicy {
    Never => "never",
    Retry => "retry",
    ReconcileThenRetry => "reconcile_then_retry",
    OperatorRequired => "operator_required",
});

impl FailureMetadata {
    pub fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            code: FailureCode::parse(value.get("code")?.as_str()?),
            detail: value.get("detail")?.as_str()?.to_owned(),
            phase: FailurePhase::parse(value.get("phase")?.as_str()?),
            retry_policy: RetryPolicy::parse(value.get("retry_policy")?.as_str()?),
            recovery_required: value.get("recovery_required")?.as_bool()?,
        })
    }

    pub fn to_value(&self) -> Value {
        json!({
            "code": self.code.as_str(),
            "detail": self.detail,
            "phase": self.phase.as_str(),
            "retry_policy": self.retry_policy.as_str(),
            "recovery_required": self.recovery_required,
        })
    }
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
        let error = full
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("agent command failed")
            .to_string();
        Ok(
            match full.get("failure").and_then(FailureMetadata::from_value) {
                Some(failure) => ResponseEnvelope::ErrorWithFailure { error, failure },
                None => ResponseEnvelope::Error(error),
            },
        )
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
        let enriched = parse_response_line(
            r#"{"ok":false,"error":"busy","failure":{"code":"device_busy","detail":"operation active","phase":"availability","retry_policy":"retry","recovery_required":false}}"#,
        )
        .unwrap();
        assert_eq!(
            enriched,
            ResponseEnvelope::ErrorWithFailure {
                error: "busy".into(),
                failure: FailureMetadata {
                    code: FailureCode::DeviceBusy,
                    detail: "operation active".into(),
                    phase: FailurePhase::Availability,
                    retry_policy: RetryPolicy::Retry,
                    recovery_required: false,
                },
            }
        );
        let ResponseEnvelope::ErrorWithFailure { failure, .. } = enriched else {
            unreachable!()
        };
        assert_eq!(
            FailureMetadata::from_value(&failure.to_value()),
            Some(failure)
        );
        assert_eq!(PROTOCOL_VERSION, 2);
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
    fn runtime_upload_spec_is_exact_and_bounded() {
        let sha256 = "a".repeat(64);
        let spec = RuntimeUploadSpec::from_args(&json!({
            "payload_bytes": 31_593_184,
            "sha256": sha256,
        }))
        .unwrap();
        assert_eq!(spec.payload_bytes, 31_593_184);
        assert_eq!(RuntimeUploadSpec::from_args(&spec.args()).unwrap(), spec);

        for invalid in [
            json!({}),
            json!({"payload_bytes": 1, "sha256": "a".repeat(64), "extra": true}),
            json!({"payload_bytes": 0, "sha256": "a".repeat(64)}),
            json!({"payload_bytes": MAX_RUNTIME_UPLOAD_BYTES + 1, "sha256": "a".repeat(64)}),
            json!({"payload_bytes": 1, "sha256": "A".repeat(64)}),
            json!({"payload_bytes": 1, "sha256": "g".repeat(64)}),
        ] {
            assert!(RuntimeUploadSpec::from_args(&invalid).is_err(), "{invalid}");
        }
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
