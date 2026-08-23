// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Typed host-side launcher automation and authoritative checkpoint capture.

use super::agent_client::agent_request_at;
use super::{
    NativeDeviceConfig, PngCapture, Result, capture_source_label, encode_hex,
    request_framebuffer_png_at_when_latched, validate_visible_launcher_capture,
};
use crate::transport::{AutomationAction, AutomationButton};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_WAIT: Duration = Duration::from_secs(10);
const HANDOFF_WAIT: Duration = Duration::from_secs(15);
const RETURN_WAIT: Duration = Duration::from_secs(12);
const CHECKPOINT_CAPTURE_ATTEMPTS: usize = 3;
const LAUNCH_INPUT_ATTEMPTS: usize = 3;
const LAUNCH_START_WAIT: Duration = Duration::from_secs(2);
const PUBLIC_MAIN_PATH: &str = mister_magik_platform_manifest_contract::PUBLIC_PATHS.main;
const DEVELOPMENT_MAIN_PATH: &str = mister_magik_platform_manifest_contract::DEVELOPMENT_PATHS.main;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MainRoute {
    Public,
    Development,
}

impl MainRoute {
    const fn executable_path(self) -> &'static str {
        match self {
            Self::Public => PUBLIC_MAIN_PATH,
            Self::Development => DEVELOPMENT_MAIN_PATH,
        }
    }

    const fn process_name(self) -> &'static str {
        match self {
            Self::Public => "MiSTer_MagiK",
            Self::Development => "MiSTer_MagiKDev",
        }
    }

    const fn other_process_name(self) -> &'static str {
        match self {
            Self::Public => "MiSTer_MagiKDev",
            Self::Development => "MiSTer_MagiK",
        }
    }
}

#[derive(Clone, Debug)]
struct LaunchIdentity {
    main_route: MainRoute,
    main_generation: u64,
    main_pid: u64,
    executable_path: String,
    launcher_pid: u64,
    build_version: String,
    source_revision: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HandoffMainIdentity {
    generation: u64,
    pid: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReturnedLauncherIdentity {
    main_generation: u64,
    main_pid: u64,
    launcher_pid: u64,
}

#[derive(Debug)]
pub(super) enum LaunchReturnError {
    Failed(String),
    RecoveryRequired(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LaunchProgress {
    Started,
    HandoffObserved,
    AutomationUnavailable,
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
    let nonce = result
        .get("nonce")
        .and_then(Value::as_str)
        .ok_or("automation begin response has no nonce")?;
    validate_nonce(nonce)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut last_error = None;
    while Instant::now() < deadline {
        match snapshot(config, nonce) {
            Ok(_) => return Ok(serde_json::to_string(&result)?),
            Err(error) => last_error = Some(error.to_string()),
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = end(config, nonce);
    Err(format!(
        "launcher automation socket did not become responsive: {}",
        last_error.as_deref().unwrap_or("no response")
    )
    .into())
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
            if *duration_ms == 0
                || *duration_ms > mister_magik_agent_protocol::LAUNCHER_AUTOMATION_MAX_HOLD_MS
            {
                return Err(format!(
                    "launcher automation hold must be in 1..={} milliseconds",
                    mister_magik_agent_protocol::LAUNCHER_AUTOMATION_MAX_HOLD_MS
                )
                .into());
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
    let mut confirmed = None;
    let mut last_stability_error = None;
    for attempt in 0..CHECKPOINT_CAPTURE_ATTEMPTS {
        let before = snapshot(config, nonce)?;
        require_presented_action(&before, action_sequence)?;
        let capture =
            request_framebuffer_png_at_when_latched(config.agent()?, Duration::from_secs(3))?;
        validate_visible_launcher_capture(&capture)?;
        let after = snapshot(config, nonce)?;
        match require_stable_snapshot(&before, &after) {
            Ok(()) => {
                require_capture_sequence(&capture, &after)?;
                confirmed = Some((capture, after));
                break;
            }
            Err(error) => {
                last_stability_error = Some(error.to_string());
                if attempt + 1 < CHECKPOINT_CAPTURE_ATTEMPTS {
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
    let Some((capture, after)) = confirmed else {
        return Err(last_stability_error
            .unwrap_or_else(|| "launcher checkpoint did not reach a stable presentation".into())
            .into());
    };

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
    let result = match agent_result(
        config,
        "launcher_automation_request",
        json!({"nonce":nonce,"kind":"end"}),
        Duration::from_secs(3),
    ) {
        Ok(result) => result,
        Err(error) if automation_end_socket_missing(&error.to_string()) => {
            return Ok(serde_json::to_string(&json!({"already_ended": true}))?);
        }
        Err(error) => return Err(error),
    };
    Ok(serde_json::to_string(&result)?)
}

fn automation_end_socket_missing(error: &str) -> bool {
    error.contains("send automation request: No such file or directory")
}

pub(super) fn ensure_installed_alpha_launcher(
    config: &NativeDeviceConfig,
    expected_build_version: &str,
    expected_source_revision: &str,
) -> Result<String> {
    if expected_build_version.is_empty() || expected_source_revision.is_empty() {
        return Err("installed alpha launcher identity is incomplete".into());
    }
    let status = magik_status(config)?;
    let main = status
        .pointer("/files/main_status")
        .ok_or("installed alpha Main status is missing")?;
    let main_pid = required_u64(main, "pid")?;
    require_public_main_process(&status, main_pid)?;
    let slint = status
        .pointer("/files/slint_status")
        .ok_or("installed alpha Slint status is missing")?;
    validate_candidate_build(slint, expected_build_version, expected_source_revision)?;

    if main.get("launcher_state").and_then(Value::as_str) == Some("LauncherActive") {
        let launcher_pid = required_u64(main, "launcher_pid")?;
        if !is_public_main_executable(main.get("executable_path").and_then(Value::as_str))
            || status
                .pointer("/files/slint_status_current")
                .and_then(Value::as_bool)
                != Some(true)
            || slint.get("pid").and_then(Value::as_u64) != Some(launcher_pid)
            || slint.get("input_enabled").and_then(Value::as_bool) != Some(true)
        {
            return Err("installed alpha launcher is not input-ready".into());
        }
        return Ok(serde_json::to_string(&json!({
            "schema": "mister-magik-ensure-installed-alpha-launcher-v1",
            "mode": "already-active",
            "status": status,
        }))?);
    }

    if main.get("launcher_state").and_then(Value::as_str) != Some("Unconfigured")
        || main.get("launcher_pid").and_then(Value::as_u64) != Some(0)
        || status
            .pointer("/files/slint_status_current")
            .and_then(Value::as_bool)
            != Some(false)
        || status
            .pointer("/processes/mister-magik-fb")
            .and_then(Value::as_array)
            .is_none_or(|pids| !pids.is_empty())
    {
        return Err("installed alpha is neither an active launcher nor a loaded core".into());
    }
    let core = status
        .pointer("/files/core_name")
        .and_then(Value::as_str)
        .or_else(|| status.pointer("/files/rbf_name").and_then(Value::as_str))
        .ok_or("installed alpha has no loaded core identity")?;
    if core.trim().is_empty() || core.to_ascii_lowercase().contains("menu") {
        return Err("installed alpha is not running a returnable non-Menu core".into());
    }

    let identity = LaunchIdentity {
        main_route: MainRoute::Public,
        main_generation: 0,
        main_pid: 0,
        executable_path: required_text_at(main, "/executable_path")?.to_owned(),
        launcher_pid: required_u64(slint, "pid")?,
        build_version: expected_build_version.to_owned(),
        source_revision: expected_source_revision.to_owned(),
    };
    let handoff_main = HandoffMainIdentity {
        generation: required_u64(main, "main_generation")?,
        pid: main_pid,
    };
    request_return_to_launcher(config, handoff_main.generation)?;
    let (returned_status, _) = wait_for_returned_launcher(config, &identity, handoff_main)?;
    Ok(serde_json::to_string(&json!({
        "schema": "mister-magik-ensure-installed-alpha-launcher-v1",
        "mode": "returned-from-core",
        "status": returned_status,
    }))?)
}

pub(super) fn exercise_launch_return(
    config: &NativeDeviceConfig,
    nonce: &str,
    expected_game_id: &str,
    lifetime_seconds: u64,
    game_dwell: Duration,
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
    let identity = (|| -> Result<LaunchIdentity> {
        let main = before
            .pointer("/files/main_status")
            .ok_or("Main status is missing before launch")?;
        let slint = before
            .pointer("/files/slint_status")
            .ok_or("Slint status is missing before launch")?;
        let main_pid = required_u64(main, "pid")?;
        Ok(LaunchIdentity {
            main_route: detect_main_route(&before, main_pid)?,
            main_generation: required_u64(main, "main_generation")?,
            main_pid,
            executable_path: required_text_at(main, "/executable_path")?.to_owned(),
            launcher_pid: required_u64(main, "launcher_pid")?,
            build_version: required_text_at(slint, "/build/version")?.to_owned(),
            source_revision: required_text_at(slint, "/build/source_revision")?.to_owned(),
        })
    })();
    let identity = match identity {
        Ok(identity) => identity,
        Err(error) => return fail_before_launch(config, nonce, &error.to_string()),
    };

    // A presented logical press can arrive while the navigation transition router
    // is still consuming its queued input. Require the launcher lifecycle to begin
    // before we enter the long handoff wait, and retry only while it remains idle.
    let mut launch_progress = None;
    let mut last_presentation_error = None;
    for _ in 0..LAUNCH_INPUT_ATTEMPTS {
        let action = match send_action(
            config,
            nonce,
            &AutomationAction::Hold {
                button: AutomationButton::A,
                duration_ms: 120,
            },
        ) {
            Ok(action) => action,
            Err(error) => return fail_before_launch(config, nonce, &error.to_string()),
        };
        let action: Value = serde_json::from_str(&action)
            .map_err(|error| LaunchReturnError::Failed(format!("decode launch action: {error}")))?;
        let action_sequence = action
            .get("action_sequence")
            .and_then(Value::as_u64)
            .ok_or_else(|| LaunchReturnError::Failed("launch action has no sequence".into()))?;
        last_presentation_error = await_presented(config, nonce, action_sequence, 1_000).err();

        match wait_for_launch_progress(config, nonce, &identity) {
            Ok(Some(progress)) => {
                launch_progress = Some(progress);
                break;
            }
            Ok(None) => {
                if let Err(error) = send_action(config, nonce, &AutomationAction::ReleaseAll) {
                    return fail_before_launch(config, nonce, &error.to_string());
                }
            }
            Err(error) => return fail_before_launch(config, nonce, &error.to_string()),
        }
    }
    if launch_progress.is_none() {
        let detail = last_presentation_error.map_or_else(
            || {
                format!(
                    "launch press did not start the lifecycle after {LAUNCH_INPUT_ATTEMPTS} bounded input attempts"
                )
            },
            |presentation| {
                format!(
                    "launch press did not start the lifecycle after {LAUNCH_INPUT_ATTEMPTS} bounded input attempts; last press was not presented: {presentation}"
                )
            },
        );
        return fail_before_launch(config, nonce, &detail);
    }

    let (handoff, handoff_main) = match wait_for_handoff(config, &identity) {
        Ok(evidence) => evidence,
        Err(error) => {
            return recover_after_launch_failure(config, nonce, &identity, error);
        }
    };
    thread::sleep(game_dwell);
    if let Err(error) = request_return_to_launcher(config, handoff_main.generation) {
        return Err(LaunchReturnError::RecoveryRequired(format!(
            "game handoff passed but typed return failed: {error}"
        )));
    }
    let (restored, returned) = wait_for_returned_launcher(config, &identity, handoff_main)
        .map_err(|error| LaunchReturnError::RecoveryRequired(error.to_string()))?;
    let begun: Value = serde_json::from_str(
        &begin(
            config,
            &identity.build_version,
            &identity.source_revision,
            returned.main_generation,
            lifetime_seconds,
        )
        .map_err(|error| LaunchReturnError::Failed(error.to_string()))?,
    )
    .map_err(|error| LaunchReturnError::Failed(format!("decode replacement session: {error}")))?;
    let new_nonce = begun
        .get("nonce")
        .and_then(Value::as_str)
        .ok_or_else(|| LaunchReturnError::Failed("replacement session has no nonce".into()))?
        .to_owned();
    let (post_return_sequence, restored_snapshot) =
        match prepare_replacement_session(config, &new_nonce, expected_game_id) {
            Ok(evidence) => evidence,
            Err(error) => {
                let _ = end(config, &new_nonce);
                return Err(LaunchReturnError::Failed(error.to_string()));
            }
        };

    serde_json::to_string(&json!({
        "schema": "mister-magik-launcher-automation-launch-return-v1",
        "nonce": new_nonce,
        "post_return_action_sequence": post_return_sequence,
        "pre_launch_snapshot": pre_launch,
        "handoff": handoff,
        "game_dwell_ms": game_dwell.as_millis(),
        "restored_status": restored,
        "restored_snapshot": restored_snapshot,
    }))
    .map_err(|error| LaunchReturnError::Failed(error.to_string()))
}

fn prepare_replacement_session(
    config: &NativeDeviceConfig,
    new_nonce: &str,
    expected_game_id: &str,
) -> Result<(u64, Value)> {
    let released: Value = serde_json::from_str(
        &send_action(config, new_nonce, &AutomationAction::ReleaseAll)
            .map_err(|error| format!("release replacement automation input: {error}"))?,
    )
    .map_err(|error| format!("decode release action: {error}"))?;
    let post_return_sequence = released
        .get("action_sequence")
        .and_then(Value::as_u64)
        .ok_or("release action has no sequence")?;
    await_presented(config, new_nonce, post_return_sequence, 3_000)?;
    let restored_snapshot = wait_for_restored_snapshot(config, new_nonce, expected_game_id)?;
    Ok((post_return_sequence, restored_snapshot))
}

fn wait_for_restored_snapshot(
    config: &NativeDeviceConfig,
    nonce: &str,
    expected_game_id: &str,
) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut last_error = String::from("no restored snapshot");
    while Instant::now() < deadline {
        match snapshot(config, nonce) {
            Ok(value) => match validate_restored_snapshot(&value, expected_game_id) {
                Ok(()) => return Ok(value),
                Err(error) => last_error = error.to_string(),
            },
            Err(error) => last_error = error.to_string(),
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(format!("returned launcher did not satisfy Arcade state: {last_error}").into())
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
    identity: &LaunchIdentity,
    launch_error: impl std::fmt::Display,
) -> std::result::Result<T, LaunchReturnError> {
    let _ = end(config, nonce);
    let status = magik_status(config).ok();
    if status
        .as_ref()
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
    let handoff_main = status
        .as_ref()
        .and_then(|status| validate_handoff_status(status, identity).ok());
    let Some(handoff_main) = handoff_main else {
        return Err(LaunchReturnError::RecoveryRequired(format!(
            "{launch_error}; current Main state is neither the launcher nor a proven core handoff"
        )));
    };
    match request_return_to_launcher(config, handoff_main.generation)
        .and_then(|_| wait_for_returned_launcher(config, identity, handoff_main))
    {
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

fn wait_for_handoff(
    config: &NativeDeviceConfig,
    identity: &LaunchIdentity,
) -> Result<(Value, HandoffMainIdentity)> {
    let deadline = Instant::now() + HANDOFF_WAIT;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        if let Ok(status) = magik_status(config) {
            if let Ok(handoff_main) = validate_handoff_status(&status, identity) {
                return Ok((status, handoff_main));
            }
            last = status;
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(format!("real core handoff was not proven before timeout; last_status={last}").into())
}

fn wait_for_launch_progress(
    config: &NativeDeviceConfig,
    nonce: &str,
    identity: &LaunchIdentity,
) -> Result<Option<LaunchProgress>> {
    let deadline = Instant::now() + LAUNCH_START_WAIT;
    let mut last_snapshot_error = None;
    let mut observed_snapshot = false;
    while Instant::now() < deadline {
        match snapshot(config, nonce) {
            Ok(value) => {
                observed_snapshot = true;
                validate_snapshot(&value)?;
                if semantic(&value, "launch_state").and_then(Value::as_str) == Some("launching") {
                    return Ok(Some(LaunchProgress::Started));
                }
                if semantic(&value, "overlay").and_then(Value::as_str) == Some("confirm") {
                    let title = semantic(&value, "dialog_title")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("launch failed");
                    let message = semantic(&value, "dialog_message")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("launcher did not provide a failure detail");
                    return Err(
                        format!("launcher rejected selected core: {title}: {message}").into(),
                    );
                }
            }
            Err(error) => {
                last_snapshot_error = Some(error.to_string());
                if magik_status(config)
                    .ok()
                    .is_some_and(|status| validate_handoff_status(&status, identity).is_ok())
                {
                    return Ok(Some(LaunchProgress::HandoffObserved));
                }
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    if !observed_snapshot && last_snapshot_error.is_some() {
        // The launcher process normally exits during a real core handoff, and
        // its socket can disappear before the replacement Main status becomes
        // observable. This is only permission to enter the longer handoff
        // proof; wait_for_handoff still requires a fresh Main epoch, no live
        // launcher process, and a non-Menu core before return is requested.
        return Ok(Some(LaunchProgress::AutomationUnavailable));
    }
    Ok(None)
}

fn wait_for_returned_launcher(
    config: &NativeDeviceConfig,
    identity: &LaunchIdentity,
    handoff_main: HandoffMainIdentity,
) -> Result<(Value, ReturnedLauncherIdentity)> {
    let deadline = Instant::now() + RETURN_WAIT;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        if let Ok(status) = magik_status(config) {
            if let Ok(returned) = validate_returned_status(&status, identity, handoff_main) {
                return Ok((status, returned));
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
    let reply = agent_request_at(config.agent()?, command, args, timeout)?;
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

fn require_public_main_process(status: &Value, expected_pid: u64) -> Result<()> {
    if status
        .pointer("/processes/MiSTer_MagiK")
        .and_then(Value::as_array)
        .is_none_or(|pids| !pids.iter().any(|pid| pid.as_u64() == Some(expected_pid)))
        || status
            .pointer("/processes/MiSTer_MagiKDev")
            .and_then(Value::as_array)
            .is_some_and(|pids| !pids.is_empty())
    {
        Err("Main is not the public alpha process".into())
    } else {
        Ok(())
    }
}

fn validate_candidate_build(
    slint: &Value,
    expected_build_version: &str,
    expected_source_revision: &str,
) -> Result<()> {
    if slint.pointer("/build/version").and_then(Value::as_str) != Some(expected_build_version)
        || slint
            .pointer("/build/source_revision")
            .and_then(Value::as_str)
            != Some(expected_source_revision)
    {
        Err("Slint status does not match the installed alpha candidate".into())
    } else {
        Ok(())
    }
}

fn is_public_main_executable(path: Option<&str>) -> bool {
    is_main_executable(MainRoute::Public, path)
}

fn is_main_executable(route: MainRoute, path: Option<&str>) -> bool {
    matches!(path, Some("unknown")) || path == Some(route.executable_path())
}

fn route_process_is_current(status: &Value, route: MainRoute, pid: u64) -> bool {
    status
        .pointer(&format!("/processes/{}", route.process_name()))
        .and_then(Value::as_array)
        .is_some_and(|pids| pids.iter().any(|value| value.as_u64() == Some(pid)))
        && status
            .pointer(&format!("/processes/{}", route.other_process_name()))
            .and_then(Value::as_array)
            .is_none_or(|pids| pids.is_empty())
}

fn detect_main_route(status: &Value, pid: u64) -> Result<MainRoute> {
    match (
        route_process_is_current(status, MainRoute::Public, pid),
        route_process_is_current(status, MainRoute::Development, pid),
    ) {
        (true, false) => Ok(MainRoute::Public),
        (false, true) => Ok(MainRoute::Development),
        _ => Err("active Main process route is ambiguous or missing".into()),
    }
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
        let actual = semantic(snapshot, field).and_then(Value::as_str);
        if actual != Some(expected) {
            return Err(format!(
                "restored launcher failed: {field} expected={expected} actual={}",
                actual.unwrap_or("missing")
            )
            .into());
        }
    }
    if semantic(snapshot, "input_enabled").and_then(Value::as_bool) != Some(true) {
        return Err("restored launcher input is disabled".into());
    }
    Ok(())
}

fn validate_handoff_status(
    status: &Value,
    identity: &LaunchIdentity,
) -> Result<HandoffMainIdentity> {
    let main = status
        .pointer("/files/main_status")
        .ok_or("handoff status has no Main identity")?;
    let handoff_main = HandoffMainIdentity {
        generation: required_u64(main, "main_generation")?,
        pid: required_u64(main, "pid")?,
    };
    if main.get("launcher_state").and_then(Value::as_str) != Some("Unconfigured")
        || main.get("launcher_pid").and_then(Value::as_u64) != Some(0)
    {
        return Err("Main has not completed the real launcher handoff".into());
    }
    if handoff_main.generation == identity.main_generation
        || handoff_main.pid == identity.main_pid
        || !is_main_executable(identity.main_route, Some(identity.executable_path.as_str()))
        || !is_main_executable(
            identity.main_route,
            main.get("executable_path").and_then(Value::as_str),
        )
        || main.get("command_channel").and_then(Value::as_str) != Some("ready")
    {
        return Err("core handoff did not create the expected replacement Main epoch".into());
    }
    if !route_process_is_current(status, identity.main_route, handoff_main.pid) {
        return Err("replacement Main changed its selected process route".into());
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
    Ok(handoff_main)
}

fn validate_returned_status(
    status: &Value,
    identity: &LaunchIdentity,
    handoff_main: HandoffMainIdentity,
) -> Result<ReturnedLauncherIdentity> {
    let main = status
        .pointer("/files/main_status")
        .ok_or("returned status has no Main identity")?;
    let launcher_pid = main
        .get("launcher_pid")
        .and_then(Value::as_u64)
        .ok_or("returned Main status has no launcher pid")?;
    let returned = ReturnedLauncherIdentity {
        main_generation: required_u64(main, "main_generation")?,
        main_pid: required_u64(main, "pid")?,
        launcher_pid,
    };
    if returned.main_generation == handoff_main.generation
        || returned.main_pid == handoff_main.pid
        || main.get("executable_path").and_then(Value::as_str)
            != Some(identity.main_route.executable_path())
        || main.get("launcher_state").and_then(Value::as_str) != Some("LauncherActive")
        || launcher_pid == 0
        || launcher_pid == identity.launcher_pid
    {
        return Err("returned launcher is not a new Main/menu epoch".into());
    }
    if !route_process_is_current(status, identity.main_route, returned.main_pid) {
        return Err("returned launcher changed its selected Main process route".into());
    }
    let slint = status
        .pointer("/files/slint_status")
        .ok_or("returned status has no Slint identity")?;
    if status
        .pointer("/files/slint_status_current")
        .and_then(Value::as_bool)
        != Some(true)
        || slint.get("pid").and_then(Value::as_u64) != Some(launcher_pid)
        || slint.pointer("/build/version").and_then(Value::as_str)
            != Some(identity.build_version.as_str())
        || slint
            .pointer("/build/source_revision")
            .and_then(Value::as_str)
            != Some(identity.source_revision.as_str())
        || slint.get("startup_mode").and_then(Value::as_str) != Some("return_from_game")
        || slint.get("input_enabled").and_then(Value::as_bool) != Some(true)
    {
        return Err("returned Slint process is not the input-ready candidate build".into());
    }
    Ok(returned)
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
    fn handoff_requires_a_successor_main_and_non_menu_core() {
        let identity = LaunchIdentity {
            main_route: MainRoute::Public,
            main_generation: 5,
            main_pid: 9,
            executable_path: "unknown".into(),
            launcher_pid: 19,
            build_version: "0.2.4".into(),
            source_revision: "abc".into(),
        };
        let mut status = json!({
            "processes": {
                "MiSTer_MagiK": [11],
                "MiSTer_MagiKDev": [],
                "mister-magik-fb": [],
            },
            "files": {
                "main_status": {
                    "main_generation": 7,
                    "pid": 11,
                    "executable_path": "unknown",
                    "command_channel": "ready",
                    "launcher_pid": 0,
                    "launcher_state": "Unconfigured",
                    "last_operation": "HandoffComplete",
                    "last_operation_result": "completed",
                },
                "slint_status_current": false,
                "core_name": "MoonPatrol",
            }
        });
        assert_eq!(
            validate_handoff_status(&status, &identity).unwrap(),
            HandoffMainIdentity {
                generation: 7,
                pid: 11,
            }
        );
        status["files"]["core_name"] = json!("Menu");
        assert!(validate_handoff_status(&status, &identity).is_err());
    }

    #[test]
    fn handoff_accepts_public_main_recreated_by_core_load() {
        let identity = LaunchIdentity {
            main_route: MainRoute::Public,
            main_generation: 7,
            main_pid: 11,
            executable_path: "unknown".into(),
            launcher_pid: 19,
            build_version: "0.2.4".into(),
            source_revision: "abc".into(),
        };
        let status = json!({
            "processes": {
                "MiSTer_MagiK": [31],
                "MiSTer_MagiKDev": [],
                "mister-magik-fb": [],
            },
            "files": {
                "main_status": {
                    "main_generation": 29,
                    "pid": 31,
                    "executable_path": "unknown",
                    "command_channel": "ready",
                    "launcher_pid": 0,
                    "launcher_state": "Unconfigured",
                    "last_operation": "startup",
                    "last_operation_result": "completed",
                },
                "slint_status_current": false,
                "core_name": "1943mii",
            }
        });
        assert_eq!(
            validate_handoff_status(&status, &identity).unwrap(),
            HandoffMainIdentity {
                generation: 29,
                pid: 31,
            }
        );
    }

    #[test]
    fn handoff_and_return_preserve_development_main_route() {
        let identity = LaunchIdentity {
            main_route: MainRoute::Development,
            main_generation: 7,
            main_pid: 11,
            executable_path: DEVELOPMENT_MAIN_PATH.into(),
            launcher_pid: 19,
            build_version: "0.2.4".into(),
            source_revision: "abc".into(),
        };
        let handoff = json!({
            "processes": {
                "MiSTer_MagiK": [],
                "MiSTer_MagiKDev": [31],
                "mister-magik-fb": [],
            },
            "files": {
                "main_status": {
                    "main_generation": 29,
                    "pid": 31,
                    "executable_path": "unknown",
                    "command_channel": "ready",
                    "launcher_pid": 0,
                    "launcher_state": "Unconfigured",
                },
                "slint_status_current": false,
                "core_name": "1943kai",
            }
        });
        assert_eq!(
            validate_handoff_status(&handoff, &identity).unwrap(),
            HandoffMainIdentity {
                generation: 29,
                pid: 31,
            }
        );

        let returned = json!({
            "processes": {
                "MiSTer_MagiK": [],
                "MiSTer_MagiKDev": [41],
            },
            "files": {
                "main_status": {
                    "main_generation": 39,
                    "pid": 41,
                    "executable_path": DEVELOPMENT_MAIN_PATH,
                    "launcher_pid": 43,
                    "launcher_state": "LauncherActive",
                },
                "slint_status_current": true,
                "slint_status": {
                    "pid": 43,
                    "build": {"version": "0.2.4", "source_revision": "abc"},
                    "startup_mode": "return_from_game",
                    "input_enabled": true,
                }
            }
        });
        assert_eq!(
            validate_returned_status(
                &returned,
                &identity,
                HandoffMainIdentity {
                    generation: 29,
                    pid: 31,
                },
            )
            .unwrap(),
            ReturnedLauncherIdentity {
                main_generation: 39,
                main_pid: 41,
                launcher_pid: 43,
            }
        );
    }

    #[test]
    fn return_requires_new_candidate_launcher_process() {
        let identity = LaunchIdentity {
            main_route: MainRoute::Public,
            main_generation: 7,
            main_pid: 11,
            executable_path: "unknown".into(),
            launcher_pid: 19,
            build_version: "0.2.4".into(),
            source_revision: "abc".into(),
        };
        let status = json!({
            "processes": {
                "MiSTer_MagiK": [31],
                "MiSTer_MagiKDev": [],
            },
            "files": {
                "main_status": {
                    "main_generation": 29,
                    "pid": 31,
                    "executable_path": "/media/fat/MiSTer_MagiK",
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
        let handoff_main = HandoffMainIdentity {
            generation: 7,
            pid: 11,
        };
        assert_eq!(
            validate_returned_status(&status, &identity, handoff_main).unwrap(),
            ReturnedLauncherIdentity {
                main_generation: 29,
                main_pid: 31,
                launcher_pid: 23,
            }
        );
        let previous_identity = LaunchIdentity {
            launcher_pid: 23,
            ..identity
        };
        assert!(validate_returned_status(&status, &previous_identity, handoff_main).is_err());
    }

    #[test]
    fn automation_identity_and_snapshot_boundaries_are_strict() {
        assert!(validate_nonce(&"a".repeat(32)).is_ok());
        assert!(validate_nonce(&"F".repeat(128)).is_ok());
        for invalid in ["short".to_string(), "g".repeat(32), "a".repeat(129)] {
            assert!(validate_nonce(&invalid).is_err());
        }

        let mut snapshot = json!({
            "state_revision": 1,
            "action_sequence": 2,
            "presented_state_revision": 3,
            "presented_action_sequence": 4,
            "presented_latch_sequence": 5,
            "semantic": {},
        });
        validate_snapshot(&snapshot).unwrap();
        require_presented_action(&snapshot, 4).unwrap();
        assert!(require_presented_action(&snapshot, 5).is_err());
        assert!(require_presented_action(&snapshot, 0).is_err());
        for field in [
            "state_revision",
            "action_sequence",
            "presented_state_revision",
            "presented_action_sequence",
            "presented_latch_sequence",
        ] {
            let saved = snapshot[field].take();
            assert!(
                validate_snapshot(&snapshot).is_err(),
                "accepted missing {field}"
            );
            snapshot[field] = saved;
        }
        snapshot["semantic"] = Value::Null;
        assert!(validate_snapshot(&snapshot).is_err());
    }

    #[test]
    fn automation_end_treats_missing_socket_as_idempotent_cleanup() {
        assert!(automation_end_socket_missing(
            "device_operation_failed: send automation request: No such file or directory (os error 2)"
        ));
        assert!(!automation_end_socket_missing(
            "device_operation_failed: receive automation response: Resource temporarily unavailable"
        ));
    }

    #[test]
    fn checkpoint_capture_requires_authoritative_nonzero_sequences() {
        let snapshot = json!({"presented_latch_sequence": 9});
        let valid = PngCapture {
            result: json!({"capture_source": {"active_sequence": 7}}),
            png: Vec::new(),
            elapsed_ms: 0,
        };
        require_capture_sequence(&valid, &snapshot).unwrap();

        let zero = PngCapture {
            result: json!({"capture_source": {"active_sequence": 0}}),
            png: Vec::new(),
            elapsed_ms: 0,
        };
        assert!(require_capture_sequence(&zero, &snapshot).is_err());
        assert!(require_capture_sequence(&valid, &json!({})).is_err());
    }

    #[test]
    fn restored_snapshot_rejects_each_stale_semantic_boundary() {
        let game = "/media/fat/_Arcade/game.mra";
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
                "selected_game_id": game,
                "input_enabled": true,
            }
        });
        validate_restored_snapshot(&snapshot, game).unwrap();
        for field in [
            "effective_view",
            "return_screen",
            "launch_state",
            "selected_game_id",
        ] {
            let mut invalid = snapshot.clone();
            invalid["semantic"][field] = json!("stale");
            assert!(validate_restored_snapshot(&invalid, game).is_err());
        }
        let mut disabled = snapshot;
        disabled["semantic"]["input_enabled"] = json!(false);
        assert!(validate_restored_snapshot(&disabled, game).is_err());
    }
}
