// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Typed host-side launcher automation and authoritative checkpoint capture.

use super::agent_client::agent_request_at;
use super::{
    NativeDeviceConfig, PngCapture, Result, capture_source_label,
    request_framebuffer_png_at_when_latched, validate_visible_launcher_capture,
};
use mister_tool::transport::{AutomationAction, AutomationButton};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

const MAX_WAIT: Duration = Duration::from_secs(10);

pub(super) fn begin(
    config: &NativeDeviceConfig,
    expected_build_version: &str,
    expected_source_revision: &str,
    expected_main_generation: u64,
    lifetime_seconds: u64,
) -> Result<String> {
    if expected_build_version.is_empty()
        || expected_source_revision.is_empty()
        || expected_main_generation == 0
        || !(1..=120).contains(&lifetime_seconds)
    {
        return Err("invalid launcher automation session identity or lifetime".into());
    }
    let result = agent_result(
        config,
        "launcher_automation_begin",
        json!({
            "expected_build_version": expected_build_version,
            "expected_source_revision": expected_source_revision,
            "expected_main_generation": expected_main_generation,
            "lifetime_seconds": lifetime_seconds,
        }),
        Duration::from_secs(3),
    )?;
    validate_nonce(
        result
            .get("nonce")
            .and_then(Value::as_str)
            .ok_or("automation begin response has no nonce")?,
    )?;
    Ok(serde_json::to_string(&result)?)
}

pub(super) fn send_action(
    config: &NativeDeviceConfig,
    nonce: &str,
    action: &AutomationAction,
) -> Result<String> {
    validate_nonce(nonce)?;
    let args = match action {
        AutomationAction::Tap(button) => action_args(nonce, "tap", Some(*button), None),
        AutomationAction::Hold {
            button,
            duration_ms,
        } => {
            if *duration_ms == 0 || *duration_ms > 2_000 {
                return Err("launcher automation hold must be in 1..=2000 milliseconds".into());
            }
            action_args(nonce, "hold", Some(*button), Some(*duration_ms))
        }
        AutomationAction::ReleaseAll => action_args(nonce, "release_all", None, None),
    };
    let result = agent_result(
        config,
        "launcher_automation_request",
        args,
        Duration::from_secs(3),
    )?;
    let sequence = result
        .get("action_sequence")
        .and_then(Value::as_u64)
        .ok_or("automation action response has no sequence")?;
    Ok(serde_json::to_string(&json!({
        "action_sequence": sequence,
        "result": result,
    }))?)
}

pub(super) fn snapshot(config: &NativeDeviceConfig, nonce: &str) -> Result<Value> {
    validate_nonce(nonce)?;
    let snapshot = agent_result(
        config,
        "launcher_automation_request",
        json!({"nonce":nonce,"kind":"snapshot"}),
        Duration::from_secs(3),
    )?;
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

pub(super) fn await_presented(
    config: &NativeDeviceConfig,
    nonce: &str,
    action_sequence: u64,
    timeout_ms: u64,
) -> Result<String> {
    if action_sequence == 0 || timeout_ms == 0 || timeout_ms > MAX_WAIT.as_millis() as u64 {
        return Err("invalid launcher automation wait sequence or timeout".into());
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let snapshot = snapshot(config, nonce)?;
        if snapshot
            .get("presented_action_sequence")
            .and_then(Value::as_u64)
            .is_some_and(|presented| presented >= action_sequence)
        {
            return Ok(serde_json::to_string(&snapshot)?);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "launcher action {action_sequence} was not presented within {timeout_ms}ms"
            )
            .into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn capture_checkpoint(
    config: &NativeDeviceConfig,
    nonce: &str,
    action_sequence: u64,
    label: &str,
    output_dir: &Path,
) -> Result<String> {
    validate_checkpoint_label(label)?;
    let before = snapshot(config, nonce)?;
    require_presented_action(&before, action_sequence)?;
    let capture = request_framebuffer_png_at_when_latched(&config.agent, Duration::from_secs(3))?;
    validate_visible_launcher_capture(&capture)?;
    let after = snapshot(config, nonce)?;
    require_stable_snapshot(&before, &after)?;
    require_capture_sequence(&capture, &after)?;

    fs::create_dir_all(output_dir)?;
    let png_path = output_dir.join(format!("{label}.png"));
    let json_path = output_dir.join(format!("{label}.json"));
    if png_path.exists() || json_path.exists() {
        return Err(format!("checkpoint output already exists for {label}").into());
    }
    let png_sha256 = sha256_hex(&capture.png);
    let metadata = json!({
        "schema": "mister-magik-launcher-checkpoint-v1",
        "label": label,
        "requested_action_sequence": action_sequence,
        "snapshot": after,
        "capture": capture.result,
        "capture_source": capture_source_label(&capture.result)?,
        "png": png_path.file_name().and_then(|name| name.to_str()).unwrap_or(""),
        "png_bytes": capture.png.len(),
        "png_sha256": png_sha256,
        "capture_elapsed_ms": capture.elapsed_ms,
    });
    fs::write(&png_path, &capture.png)?;
    if let Err(error) = fs::write(
        &json_path,
        format!("{}\n", serde_json::to_string_pretty(&metadata)?),
    ) {
        let _ = fs::remove_file(&png_path);
        return Err(error.into());
    }
    Ok(serde_json::to_string(&metadata)?)
}

pub(super) fn end(config: &NativeDeviceConfig, nonce: &str) -> Result<String> {
    validate_nonce(nonce)?;
    let result = agent_result(
        config,
        "launcher_automation_request",
        json!({"nonce":nonce,"kind":"end"}),
        Duration::from_secs(3),
    )?;
    Ok(serde_json::to_string(&result)?)
}

fn agent_result(
    config: &NativeDeviceConfig,
    command: &str,
    args: Value,
    timeout: Duration,
) -> Result<Value> {
    let reply = agent_request_at(&config.agent, command, args, timeout)?;
    reply
        .response
        .get("result")
        .cloned()
        .ok_or_else(|| format!("agent {command} response has no result").into())
}

fn action_args(
    nonce: &str,
    kind: &str,
    button: Option<AutomationButton>,
    duration_ms: Option<u64>,
) -> Value {
    let mut args = json!({"nonce":nonce,"kind":kind});
    if let Some(object) = args.as_object_mut() {
        if let Some(button) = button {
            object.insert("button".to_string(), json!(button.label()));
        }
        if let Some(duration_ms) = duration_ms {
            object.insert("duration_ms".to_string(), json!(duration_ms));
        }
    }
    args
}

fn validate_snapshot(snapshot: &Value) -> Result<()> {
    for field in [
        "state_revision",
        "action_sequence",
        "presented_state_revision",
        "presented_action_sequence",
        "presented_latch_sequence",
    ] {
        if snapshot.get(field).and_then(Value::as_u64).is_none() {
            return Err(format!("automation snapshot has no numeric {field}").into());
        }
    }
    if !snapshot.get("semantic").is_some_and(Value::is_object) {
        return Err("automation snapshot has no semantic state".into());
    }
    Ok(())
}

fn require_presented_action(snapshot: &Value, expected: u64) -> Result<()> {
    let actual = snapshot
        .get("presented_action_sequence")
        .and_then(Value::as_u64)
        .ok_or("automation snapshot has no presented action sequence")?;
    if expected == 0 || actual < expected {
        return Err(format!(
            "checkpoint action is not presented expected={expected} actual={actual}"
        )
        .into());
    }
    Ok(())
}

fn require_stable_snapshot(before: &Value, after: &Value) -> Result<()> {
    for field in ["presented_state_revision", "presented_action_sequence"] {
        if before.get(field) != after.get(field) {
            return Err(
                format!("launcher state changed during checkpoint capture: {field}").into(),
            );
        }
    }
    Ok(())
}

fn require_capture_sequence(capture: &PngCapture, snapshot: &Value) -> Result<()> {
    let captured = capture
        .result
        .pointer("/capture_source/active_sequence")
        .and_then(Value::as_u64)
        .ok_or("authoritative capture has no active latch sequence")?;
    if captured == 0
        || snapshot
            .get("presented_latch_sequence")
            .and_then(Value::as_u64)
            .is_none()
    {
        return Err("checkpoint has no valid captured/presented latch sequence".into());
    }
    Ok(())
}

fn validate_nonce(nonce: &str) -> Result<()> {
    if (32..=128).contains(&nonce.len()) && nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("invalid launcher automation nonce".into())
    }
}

fn validate_checkpoint_label(label: &str) -> Result<()> {
    if !label.is_empty()
        && label.len() <= 64
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err("checkpoint label must be 1..=64 ASCII letters, digits, '-' or '_'".into())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_actions_map_to_agent_contract() {
        assert_eq!(
            action_args(&"a".repeat(32), "tap", Some(AutomationButton::Home), None)["button"],
            "home"
        );
        assert_eq!(
            action_args(
                &"b".repeat(32),
                "hold",
                Some(AutomationButton::Down),
                Some(500)
            )["duration_ms"],
            500
        );
    }

    #[test]
    fn checkpoint_requires_stable_presented_identity() {
        let stable = json!({
            "presented_state_revision": 4,
            "presented_action_sequence": 3,
        });
        assert!(require_stable_snapshot(&stable, &stable).is_ok());
        let changed = json!({
            "presented_state_revision": 5,
            "presented_action_sequence": 3,
        });
        assert!(require_stable_snapshot(&stable, &changed).is_err());
    }

    #[test]
    fn checkpoint_labels_cannot_be_paths() {
        assert!(validate_checkpoint_label("arcade_search").is_ok());
        assert!(validate_checkpoint_label("../arcade").is_err());
        assert!(validate_checkpoint_label("").is_err());
    }
}
