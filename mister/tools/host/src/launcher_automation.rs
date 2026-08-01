// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Typed host-side launcher automation and authoritative checkpoint capture.

use super::agent_client::agent_request_at;
use super::{
    NativeDeviceConfig, PngCapture, Result, capture_source_label, encode_hex,
    request_framebuffer_png_at_when_latched, validate_visible_launcher_capture,
};
use mister_tool::transport::{AutomationAction, AutomationButton};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_WAIT: Duration = Duration::from_secs(10);
const HANDOFF_WAIT: Duration = Duration::from_secs(15);
const RETURN_WAIT: Duration = Duration::from_secs(12);

#[derive(Debug)]
pub(super) enum LaunchReturnError {
    Failed(String),
    RecoveryRequired(String),
}

impl std::fmt::Display for LaunchReturnError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(detail) | Self::RecoveryRequired(detail) => formatter.write_str(detail),
        }
    }
}

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

pub(super) fn exercise_launch_return(
    config: &NativeDeviceConfig,
    nonce: &str,
    expected_game_id: &str,
    lifetime_seconds: u64,
) -> std::result::Result<String, LaunchReturnError> {
    if !(1..=120).contains(&lifetime_seconds) {
        return fail_before_launch(config, nonce, "invalid replacement automation lifetime");
    }
    validate_nonce(nonce).map_err(|error| LaunchReturnError::Failed(error.to_string()))?;
    let pre_launch = match snapshot(config, nonce) {
        Ok(snapshot) => snapshot,
        Err(error) => return fail_before_launch(config, nonce, &error.to_string()),
    };
    validate_pre_launch_snapshot(&pre_launch, expected_game_id)
        .or_else(|error| fail_before_launch(config, nonce, &error.to_string()))?;
    validate_selected_mra(config, expected_game_id)
        .or_else(|error| fail_before_launch(config, nonce, &error.to_string()))?;

    let before = match magik_status(config) {
        Ok(status) => status,
        Err(error) => return fail_before_launch(config, nonce, &error.to_string()),
    };
    let identity = (|| -> Result<(u64, u64, u64, String, String)> {
        let main = before
            .pointer("/files/main_status")
            .ok_or("Main status is missing before launch")?;
        let slint = before
            .pointer("/files/slint_status")
            .ok_or("Slint status is missing before launch")?;
        Ok((
            required_u64(main, "main_generation")?,
            required_u64(main, "pid")?,
            required_u64(main, "launcher_pid")?,
            required_text_at(slint, "/build/version")?.to_owned(),
            required_text_at(slint, "/build/source_revision")?.to_owned(),
        ))
    })();
    let (main_generation, main_pid, launcher_pid, build_version, source_revision) = match identity {
        Ok(identity) => identity,
        Err(error) => return fail_before_launch(config, nonce, &error.to_string()),
    };

    let action = match send_action(config, nonce, &AutomationAction::Tap(AutomationButton::A)) {
        Ok(action) => action,
        Err(error) => return fail_before_launch(config, nonce, &error.to_string()),
    };
    let action: Value = serde_json::from_str(&action)
        .map_err(|error| LaunchReturnError::Failed(format!("decode launch action: {error}")))?;
    let action_sequence = action
        .get("action_sequence")
        .and_then(Value::as_u64)
        .ok_or_else(|| LaunchReturnError::Failed("launch action has no sequence".into()))?;
    let _ = await_presented(config, nonce, action_sequence, 1_000);

    let handoff = match wait_for_handoff(config, main_generation, main_pid) {
        Ok(status) => status,
        Err(error) => {
            return recover_after_launch_failure(
                config,
                nonce,
                main_generation,
                main_pid,
                launcher_pid,
                &build_version,
                &source_revision,
                error,
            );
        }
    };
    if let Err(error) = request_return_to_launcher(config, main_generation) {
        return Err(LaunchReturnError::RecoveryRequired(format!(
            "game handoff passed but typed return failed: {error}"
        )));
    }
    let restored = wait_for_returned_launcher(
        config,
        main_generation,
        main_pid,
        launcher_pid,
        &build_version,
        &source_revision,
    )
    .map_err(|error| LaunchReturnError::RecoveryRequired(error.to_string()))?;
    let begun: Value = serde_json::from_str(
        &begin(
            config,
            &build_version,
            &source_revision,
            main_generation,
            lifetime_seconds,
        )
        .map_err(|error| LaunchReturnError::Failed(error.to_string()))?,
    )
    .map_err(|error| LaunchReturnError::Failed(format!("decode replacement session: {error}")))?;
    let new_nonce = begun
        .get("nonce")
        .and_then(Value::as_str)
        .ok_or_else(|| LaunchReturnError::Failed("replacement session has no nonce".into()))?;
    let released: Value = serde_json::from_str(
        &send_action(config, new_nonce, &AutomationAction::ReleaseAll)
            .map_err(|error| LaunchReturnError::Failed(error.to_string()))?,
    )
    .map_err(|error| LaunchReturnError::Failed(format!("decode release action: {error}")))?;
    let post_return_sequence = released
        .get("action_sequence")
        .and_then(Value::as_u64)
        .ok_or_else(|| LaunchReturnError::Failed("release action has no sequence".into()))?;
    await_presented(config, new_nonce, post_return_sequence, 3_000)
        .map_err(|error| LaunchReturnError::Failed(error.to_string()))?;
    let restored_snapshot = snapshot(config, new_nonce)
        .map_err(|error| LaunchReturnError::Failed(error.to_string()))?;
    validate_restored_snapshot(&restored_snapshot, expected_game_id)
        .map_err(|error| LaunchReturnError::Failed(error.to_string()))?;

    serde_json::to_string(&json!({
        "schema": "mister-magik-launcher-automation-launch-return-v1",
        "nonce": new_nonce,
        "post_return_action_sequence": post_return_sequence,
        "pre_launch_snapshot": pre_launch,
        "handoff": handoff,
        "restored_status": restored,
        "restored_snapshot": restored_snapshot,
    }))
    .map_err(|error| LaunchReturnError::Failed(error.to_string()))
}

fn fail_before_launch<T>(
    config: &NativeDeviceConfig,
    nonce: &str,
    detail: &str,
) -> std::result::Result<T, LaunchReturnError> {
    match end(config, nonce) {
        Ok(_) => Err(LaunchReturnError::Failed(detail.to_owned())),
        Err(cleanup) => Err(LaunchReturnError::Failed(format!(
            "{detail}; automation cleanup failed: {cleanup}"
        ))),
    }
}

fn recover_after_launch_failure<T>(
    config: &NativeDeviceConfig,
    nonce: &str,
    main_generation: u64,
    main_pid: u64,
    launcher_pid: u64,
    build_version: &str,
    source_revision: &str,
    launch_error: impl std::fmt::Display,
) -> std::result::Result<T, LaunchReturnError> {
    let _ = end(config, nonce);
    if magik_status(config)
        .ok()
        .and_then(|status| {
            status
                .pointer("/files/main_status/launcher_state")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .as_deref()
        == Some("LauncherActive")
    {
        return Err(LaunchReturnError::Failed(launch_error.to_string()));
    }
    match request_return_to_launcher(config, main_generation).and_then(|_| {
        wait_for_returned_launcher(
            config,
            main_generation,
            main_pid,
            launcher_pid,
            build_version,
            source_revision,
        )
    }) {
        Ok(_) => Err(LaunchReturnError::Failed(launch_error.to_string())),
        Err(recovery) => Err(LaunchReturnError::RecoveryRequired(format!(
            "{launch_error}; typed return recovery failed: {recovery}"
        ))),
    }
}

fn validate_selected_mra(config: &NativeDeviceConfig, game_id: &str) -> Result<()> {
    let relative = game_id
        .strip_prefix("/media/fat")
        .filter(|path| path.starts_with('/') && path.ends_with(".mra"))
        .ok_or("selected game is not a safe MiSTer MRA path")?;
    let stat = agent_result(
        config,
        "sd_stat_item_v1",
        json!({"path": relative}),
        Duration::from_secs(3),
    )?;
    if stat.get("schema").and_then(Value::as_str) != Some("mister-magik-sd-stat-item-v1")
        || stat
            .pointer("/capabilities/mra_parse")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err("selected game is not a parseable MRA".into());
    }
    let parsed = agent_result(
        config,
        "sd_parse_mra_v1",
        json!({"path": relative}),
        Duration::from_secs(3),
    )?;
    if parsed.get("schema").and_then(Value::as_str) != Some("mister-magik-sd-parse-mra-v1") {
        return Err("selected MRA did not pass the typed parser".into());
    }
    Ok(())
}

fn magik_status(config: &NativeDeviceConfig) -> Result<Value> {
    agent_result(
        config,
        "magik",
        json!({"action":"status"}),
        Duration::from_secs(3),
    )
}

fn request_return_to_launcher(
    config: &NativeDeviceConfig,
    expected_generation: u64,
) -> Result<Value> {
    agent_result(
        config,
        "magik",
        json!({
            "action": "return-to-launcher",
            "operation_id": format!(
                "alpha-return-{}-{}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
            ),
            "expected_generation": expected_generation,
            "target": Value::Null,
        }),
        Duration::from_secs(8),
    )
}

fn wait_for_handoff(config: &NativeDeviceConfig, generation: u64, main_pid: u64) -> Result<Value> {
    let deadline = Instant::now() + HANDOFF_WAIT;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        if let Ok(status) = magik_status(config) {
            if validate_handoff_status(&status, generation, main_pid).is_ok() {
                return Ok(status);
            }
            last = status;
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(format!("real core handoff was not proven before timeout; last_status={last}").into())
}

fn wait_for_returned_launcher(
    config: &NativeDeviceConfig,
    generation: u64,
    main_pid: u64,
    previous_launcher_pid: u64,
    version: &str,
    revision: &str,
) -> Result<Value> {
    let deadline = Instant::now() + RETURN_WAIT;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        if let Ok(status) = magik_status(config) {
            if validate_returned_status(
                &status,
                generation,
                main_pid,
                previous_launcher_pid,
                version,
                revision,
            )
            .is_ok()
            {
                return Ok(status);
            }
            last = status;
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(format!("launcher return was not proven before timeout; last_status={last}").into())
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

fn semantic<'a>(snapshot: &'a Value, field: &str) -> Option<&'a Value> {
    snapshot.get("semantic")?.get(field)
}

fn validate_pre_launch_snapshot(snapshot: &Value, expected_game_id: &str) -> Result<()> {
    validate_snapshot(snapshot)?;
    for (field, expected) in [
        ("effective_view", "arcade"),
        ("return_screen", "arcade"),
        ("launch_state", "idle"),
        ("overlay", "none"),
        ("selected_game_id", expected_game_id),
    ] {
        if semantic(snapshot, field).and_then(Value::as_str) != Some(expected) {
            return Err(format!("launch precondition failed: {field} is not {expected}").into());
        }
    }
    if semantic(snapshot, "input_enabled").and_then(Value::as_bool) != Some(true) {
        return Err("launch precondition failed: launcher input is disabled".into());
    }
    Ok(())
}

fn validate_restored_snapshot(snapshot: &Value, expected_game_id: &str) -> Result<()> {
    validate_snapshot(snapshot)?;
    for (field, expected) in [
        ("effective_view", "arcade"),
        ("return_screen", "arcade"),
        ("launch_state", "idle"),
        ("selected_game_id", expected_game_id),
    ] {
        if semantic(snapshot, field).and_then(Value::as_str) != Some(expected) {
            return Err(format!("restored launcher failed: {field} is not {expected}").into());
        }
    }
    if semantic(snapshot, "input_enabled").and_then(Value::as_bool) != Some(true) {
        return Err("restored launcher input is disabled".into());
    }
    Ok(())
}

fn validate_handoff_status(status: &Value, generation: u64, main_pid: u64) -> Result<()> {
    let main = status
        .pointer("/files/main_status")
        .ok_or("handoff status has no Main identity")?;
    if main.get("main_generation").and_then(Value::as_u64) != Some(generation)
        || main.get("pid").and_then(Value::as_u64) != Some(main_pid)
        || main.get("launcher_state").and_then(Value::as_str) != Some("Unconfigured")
        || main.get("last_operation").and_then(Value::as_str) != Some("HandoffComplete")
        || main.get("last_operation_result").and_then(Value::as_str) != Some("completed")
        || main.get("launcher_pid").and_then(Value::as_u64) != Some(0)
    {
        return Err("Main has not completed the real launcher handoff".into());
    }
    if status
        .pointer("/processes/mister-magik-fb")
        .and_then(Value::as_array)
        .is_none_or(|pids| !pids.is_empty())
        || status
            .pointer("/files/slint_status_current")
            .and_then(Value::as_bool)
            != Some(false)
    {
        return Err("launcher process or current Slint status survived handoff".into());
    }
    let core = status
        .pointer("/files/core_name")
        .and_then(Value::as_str)
        .or_else(|| status.pointer("/files/rbf_name").and_then(Value::as_str))
        .ok_or("handoff status has no loaded core identity")?;
    if core.trim().is_empty() || core.to_ascii_lowercase().contains("menu") {
        return Err("handoff did not load a non-Menu core".into());
    }
    Ok(())
}

fn validate_returned_status(
    status: &Value,
    generation: u64,
    main_pid: u64,
    previous_launcher_pid: u64,
    version: &str,
    revision: &str,
) -> Result<()> {
    let main = status
        .pointer("/files/main_status")
        .ok_or("returned status has no Main identity")?;
    let launcher_pid = main
        .get("launcher_pid")
        .and_then(Value::as_u64)
        .ok_or("returned Main status has no launcher pid")?;
    if main.get("main_generation").and_then(Value::as_u64) != Some(generation)
        || main.get("pid").and_then(Value::as_u64) != Some(main_pid)
        || main.get("launcher_state").and_then(Value::as_str) != Some("LauncherActive")
        || launcher_pid == 0
        || launcher_pid == previous_launcher_pid
    {
        return Err("returned launcher does not match the original Main generation".into());
    }
    let slint = status
        .pointer("/files/slint_status")
        .ok_or("returned status has no Slint identity")?;
    if status
        .pointer("/files/slint_status_current")
        .and_then(Value::as_bool)
        != Some(true)
        || slint.get("pid").and_then(Value::as_u64) != Some(launcher_pid)
        || slint.pointer("/build/version").and_then(Value::as_str) != Some(version)
        || slint
            .pointer("/build/source_revision")
            .and_then(Value::as_str)
            != Some(revision)
        || slint.get("startup_mode").and_then(Value::as_str) != Some("return_from_game")
        || slint.get("input_enabled").and_then(Value::as_bool) != Some(true)
    {
        return Err("returned Slint process is not the input-ready candidate build".into());
    }
    Ok(())
}

fn required_u64(value: &Value, field: &str) -> Result<u64> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("status has no positive {field}").into())
}

fn required_text_at<'a>(value: &'a Value, pointer: &str) -> Result<&'a str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("status has no {pointer}").into())
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
    encode_hex(&digest.finalize())
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

    #[test]
    fn launch_requires_the_presented_idle_arcade_selection() {
        let snapshot = json!({
            "state_revision": 1,
            "action_sequence": 2,
            "presented_state_revision": 1,
            "presented_action_sequence": 2,
            "presented_latch_sequence": 3,
            "semantic": {
                "effective_view": "arcade",
                "return_screen": "arcade",
                "launch_state": "idle",
                "overlay": "none",
                "selected_game_id": "/media/fat/_Arcade/game.mra",
                "input_enabled": true,
            }
        });
        assert!(validate_pre_launch_snapshot(&snapshot, "/media/fat/_Arcade/game.mra").is_ok());
        let mut blocked = snapshot;
        blocked["semantic"]["overlay"] = json!("confirm");
        assert!(validate_pre_launch_snapshot(&blocked, "/media/fat/_Arcade/game.mra").is_err());
    }

    #[test]
    fn handoff_requires_same_main_and_a_non_menu_core() {
        let mut status = json!({
            "processes": {"mister-magik-fb": []},
            "files": {
                "main_status": {
                    "main_generation": 7,
                    "pid": 11,
                    "launcher_pid": 0,
                    "launcher_state": "Unconfigured",
                    "last_operation": "HandoffComplete",
                    "last_operation_result": "completed",
                },
                "slint_status_current": false,
                "core_name": "MoonPatrol",
            }
        });
        assert!(validate_handoff_status(&status, 7, 11).is_ok());
        status["files"]["core_name"] = json!("Menu");
        assert!(validate_handoff_status(&status, 7, 11).is_err());
    }

    #[test]
    fn return_requires_new_candidate_launcher_process() {
        let status = json!({
            "files": {
                "main_status": {
                    "main_generation": 7,
                    "pid": 11,
                    "launcher_pid": 23,
                    "launcher_state": "LauncherActive",
                },
                "slint_status_current": true,
                "slint_status": {
                    "pid": 23,
                    "build": {"version": "0.2.4", "source_revision": "abc"},
                    "startup_mode": "return_from_game",
                    "input_enabled": true,
                }
            }
        });
        assert!(validate_returned_status(&status, 7, 11, 19, "0.2.4", "abc").is_ok());
        assert!(validate_returned_status(&status, 7, 11, 23, "0.2.4", "abc").is_err());
    }
}
