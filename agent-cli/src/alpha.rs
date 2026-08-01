// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::archive::read_distribution_zip;
use crate::device::DeviceClient;
use crate::error::{AgentError, AgentResult};
use crate::platform_manifest::{self, Layout};
use crate::progress::{EventKind, Reporter};
use mister_tool::transport::{
    AlphaCandidateHashes, AutomationAction, AutomationButton, DeviceRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const ASSET_FORMAT: &str = "mister-magik-release-assets-v1";
const RELEASE_FORMAT: &str = "mister-magik-release-v1";

#[derive(Clone, Debug, Deserialize)]
struct AssetReceipt {
    format: String,
    version: String,
    build_number: u64,
    archive: String,
    archive_sha256: String,
    files: Vec<AssetFile>,
}

#[derive(Clone, Debug, Deserialize)]
struct AssetFile {
    path: String,
    asset: String,
    size: u64,
    sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CandidateIdentity {
    pub format: &'static str,
    pub version: String,
    pub build_number: u64,
    pub candidate_tag: String,
    pub archive: String,
    pub archive_sha256: String,
    pub release_assets_sha256: String,
    pub magik_revision: String,
    pub gui_sha256: String,
    pub platform_manifest_sha256: String,
    pub platform_bundle_id: String,
    pub qualification_candidate_id: String,
    pub component_sha256: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
struct AcceptanceReceipt {
    format: &'static str,
    accepted: bool,
    accepted_at_unix: u64,
    candidate: CandidateIdentity,
    catalog_creation: Value,
    installed_runtime: Value,
    checkpoints: Vec<Value>,
    launch_return: Value,
    usb_video: Vec<UsbEvidence>,
}

#[derive(Debug, Serialize)]
struct UsbEvidence {
    label: String,
    path: String,
    bytes: usize,
    width: u32,
    height: u32,
    sha256: String,
}

pub fn execute(
    candidate_root: &Path,
    output: &Path,
    reuse_installed: bool,
    restore_host_mode: bool,
    reporter: &mut Reporter<'_>,
) -> AgentResult<PathBuf> {
    reporter.emit(
        EventKind::Progress,
        "candidate",
        "Verifying the immutable alpha candidate",
        Some(5),
    )?;
    let candidate = verify_candidate(candidate_root)?;
    if output.exists() {
        return classified(
            "alpha_evidence_exists",
            format!("output already exists: {}", output.display()),
        );
    }
    if reuse_installed && restore_host_mode {
        return classified(
            "invalid_alpha_acceptance_mode",
            "--reuse-installed cannot restore an unknown pre-install host mode",
        );
    }
    let mut device = DeviceClient::default();
    let (original_main, catalog_start) = if reuse_installed {
        reporter.emit(
            EventKind::Progress,
            "reuse",
            "Reusing the identity-verified public alpha without reinstalling or rebooting",
            Some(10),
        )?;
        (
            None,
            reuse_installed_catalog_start(&mut device, &candidate)?,
        )
    } else {
        reporter.emit(
            EventKind::Progress,
            "install",
            "Installing the exact candidate through MiSTer Downloader",
            Some(10),
        )?;
        let activation: Value =
            serde_json::from_str(&device.execute(DeviceRequest::InstallAlphaCandidate {
                tag: candidate.candidate_tag.clone(),
                hashes: candidate_hashes(&candidate)?,
                restore_on_failure: restore_host_mode,
            })?)
            .map_err(|error| format!("cannot parse alpha activation: {error}"))?;
        (
            Some(alpha_original_main(&activation)?),
            alpha_catalog_start(&activation)?,
        )
    };
    let acceptance =
        accept_installed_candidate(&mut device, candidate, catalog_start, output, reporter);
    let restored = if restore_host_mode {
        device.execute(DeviceRequest::RestoreAlphaHostMode {
            original_main: original_main
                .ok_or("alpha acceptance has no host-mode restore target")?,
        })
    } else {
        Ok("host-mode=kept-public-alpha".into())
    };
    let receipt = match (acceptance, restored) {
        (Ok(receipt), Ok(_)) => receipt,
        (Err(error), Ok(_)) => return Err(error),
        (Ok(_), Err(restore)) => {
            return Err(AgentError::recovery_required(
                "alpha UI journey passed but the original MiSTer host mode was not restored",
                restore.to_string(),
            ));
        }
        (Err(error), Err(restore)) => {
            return Err(AgentError::recovery_required(
                error.to_string(),
                format!("alpha host-mode restore failed: {restore}"),
            ));
        }
    };
    let receipt_path = output.join("alpha-acceptance.json");
    write_receipt_atomically(&receipt_path, &receipt)?;
    reporter.emit(
        EventKind::Progress,
        "accepted",
        "Alpha candidate passed the real-UI journey",
        Some(100),
    )?;
    Ok(receipt_path)
}

fn reuse_installed_catalog_start(
    device: &mut DeviceClient,
    candidate: &CandidateIdentity,
) -> AgentResult<Value> {
    let started_at_unix_ms = unix_millis();
    let deadline_unix_ms = started_at_unix_ms.saturating_add(8 * 60 * 1_000);
    let mut first = true;
    loop {
        let status: Value = serde_json::from_str(&device.execute(DeviceRequest::Status)?)
            .map_err(|error| format!("cannot parse installed alpha status: {error}"))?;
        let runtime = require_installed_candidate(&status, candidate)?;
        let ready = runtime.get("catalog_ready").and_then(Value::as_bool);
        let games = runtime
            .get("catalog_games")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if ready == Some(true) && games > 0 {
            return Ok(json!({
                "schema": "mister-magik-alpha-catalog-start-v1",
                "mode": "cached",
                "started_at_unix_ms": started_at_unix_ms,
                "deadline_unix_ms": deadline_unix_ms,
                "initial_catalog_ready": if first { ready } else { Some(false) },
                "initial_refresh_done": runtime.get("catalog_refresh_done"),
                "timing": {"first_visible_ms": unix_millis().saturating_sub(started_at_unix_ms)},
                "first_visible": {
                    "generation": runtime.get("catalog_generation"),
                    "games": games,
                    "systems": runtime.get("catalog_systems"),
                    "refresh_done": runtime.get("catalog_refresh_done"),
                },
            }));
        }
        if unix_millis() >= deadline_unix_ms {
            return classified(
                "alpha_catalog_creation_timeout",
                "installed alpha catalog did not become first-visible before its deadline",
            );
        }
        first = false;
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn alpha_original_main(activation: &Value) -> AgentResult<Option<String>> {
    if activation.get("schema").and_then(Value::as_str)
        != Some("mister-magik-alpha-candidate-activation-v1")
    {
        return classified(
            "invalid_alpha_activation",
            "alpha activation has an unsupported schema",
        );
    }
    let value = activation
        .get("original_main")
        .ok_or_else(|| AgentError::Classified {
            code: "invalid_alpha_activation",
            detail: "alpha activation has no original_main".into(),
        })?;
    if value.is_null() {
        return Ok(None);
    }
    let value = value.as_str().ok_or_else(|| AgentError::Classified {
        code: "invalid_alpha_activation",
        detail: "alpha activation has invalid original_main".into(),
    })?;
    if !matches!(value, "MiSTer" | "MiSTer_MagiK" | "MiSTer_MagiKDev") {
        return classified(
            "invalid_alpha_activation",
            "alpha activation has an unsafe original_main",
        );
    }
    Ok(Some(value.to_owned()))
}

fn alpha_catalog_start(activation: &Value) -> AgentResult<Value> {
    let catalog = activation
        .get("catalog")
        .ok_or_else(|| AgentError::Classified {
            code: "invalid_alpha_activation",
            detail: "alpha activation has no catalog evidence".into(),
        })?;
    if catalog.get("schema").and_then(Value::as_str) != Some("mister-magik-alpha-catalog-start-v1")
        || catalog
            .pointer("/first_visible/games")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            == 0
        || catalog
            .pointer("/timing/first_visible_ms")
            .and_then(Value::as_u64)
            .is_none()
        || catalog
            .get("started_at_unix_ms")
            .and_then(Value::as_u64)
            .is_none()
        || catalog
            .get("deadline_unix_ms")
            .and_then(Value::as_u64)
            .is_none()
    {
        return classified(
            "invalid_alpha_activation",
            "alpha activation first-visible catalog evidence is incomplete",
        );
    }
    Ok(catalog.clone())
}

fn accept_installed_candidate(
    device: &mut DeviceClient,
    candidate: CandidateIdentity,
    catalog_start: Value,
    output: &Path,
    reporter: &mut Reporter<'_>,
) -> AgentResult<AcceptanceReceipt> {
    let status: Value = serde_json::from_str(&device.execute(DeviceRequest::Status)?)
        .map_err(|error| format!("cannot parse device status: {error}"))?;
    let runtime = require_installed_candidate(&status, &candidate)?;
    let main_generation = status
        .pointer("/runtime/main_status/main_generation")
        .and_then(Value::as_u64)
        .ok_or("device status has no Main generation")?;
    fs::create_dir_all(output).map_err(|error| error.to_string())?;

    reporter.emit(
        EventKind::Progress,
        "ui",
        "Running the deterministic real-UI acceptance journey",
        Some(20),
    )?;
    let begin: Value =
        serde_json::from_str(&device.execute(DeviceRequest::BeginLauncherAutomation {
            expected_build_version: candidate.version.clone(),
            expected_source_revision: candidate.magik_revision.clone(),
            expected_main_generation: main_generation,
            lifetime_seconds: 120,
        })?)
        .map_err(|error| format!("cannot parse automation session: {error}"))?;
    let nonce = begin
        .get("nonce")
        .and_then(Value::as_str)
        .ok_or("automation session has no nonce")?
        .to_owned();
    let mut nonce = Some(nonce);
    let journey = run_ui_journey(device, &mut nonce, output);
    let ended = match nonce.as_ref() {
        Some(nonce) => device.execute(DeviceRequest::EndLauncherAutomation {
            nonce: nonce.clone(),
        }),
        None => Ok(String::from("automation session already closed")),
    };
    let (checkpoints, launch_return, usb_video) = match (journey, ended) {
        (Ok(evidence), Ok(_)) => evidence,
        (Err(error), Ok(_)) => return Err(error),
        (Ok(_), Err(restore)) => {
            return Err(AgentError::recovery_required(
                "UI journey passed but the volatile session did not close",
                restore.to_string(),
            ));
        }
        (Err(error), Err(restore)) => {
            return Err(AgentError::recovery_required(
                error.to_string(),
                format!("automation cleanup failed: {restore}"),
            ));
        }
    };
    reporter.emit(
        EventKind::Progress,
        "catalog-complete",
        "Waiting for the background catalog refresh to complete",
        Some(90),
    )?;
    let catalog_creation = complete_alpha_catalog_creation(device, &catalog_start)?;

    Ok(AcceptanceReceipt {
        format: "mister-magik-alpha-hil-v1",
        accepted: true,
        accepted_at_unix: unix_secs(),
        candidate,
        catalog_creation,
        installed_runtime: runtime,
        checkpoints,
        launch_return,
        usb_video,
    })
}

fn complete_alpha_catalog_creation(
    device: &mut DeviceClient,
    catalog_start: &Value,
) -> AgentResult<Value> {
    let started_at_unix_ms = catalog_start
        .get("started_at_unix_ms")
        .and_then(Value::as_u64)
        .ok_or("catalog start has no start time")?;
    let deadline_unix_ms = catalog_start
        .get("deadline_unix_ms")
        .and_then(Value::as_u64)
        .ok_or("catalog start has no deadline")?;
    loop {
        let status: Value =
            serde_json::from_str(&device.execute(DeviceRequest::Status)?).map_err(|error| {
                format!("cannot parse device status during catalog refresh: {error}")
            })?;
        let runtime = status
            .pointer("/runtime/slint_status")
            .filter(|value| value.is_object())
            .ok_or("device status has no Slint runtime during catalog refresh")?;
        let ready = runtime.get("catalog_ready").and_then(Value::as_bool);
        let refresh_done = runtime.get("catalog_refresh_done").and_then(Value::as_bool);
        let games = runtime
            .get("catalog_games")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let last = json!({
            "catalog_ready": ready,
            "catalog_refresh_done": refresh_done,
            "catalog_games": games,
            "catalog_systems": runtime.get("catalog_systems"),
            "catalog_generation": runtime.get("catalog_generation"),
        });
        if ready == Some(true) && refresh_done == Some(true) && games > 0 {
            let catalog: Value =
                serde_json::from_str(&device.execute(DeviceRequest::InspectPublicCatalog)?)
                    .map_err(|error| format!("cannot parse completed public catalog: {error}"))?;
            if catalog.get("valid").and_then(Value::as_bool) != Some(true)
                || catalog
                    .get("total_games")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    == 0
            {
                return classified(
                    "alpha_catalog_creation_failed",
                    "completed public catalog inspection is invalid or empty",
                );
            }
            return Ok(json!({
                "schema": "mister-magik-alpha-catalog-creation-v1",
                "mode": catalog_start.get("mode"),
                "initial_catalog_ready": catalog_start.get("initial_catalog_ready"),
                "initial_refresh_done": catalog_start.get("initial_refresh_done"),
                "timing": {
                    "first_visible_ms": catalog_start.pointer("/timing/first_visible_ms"),
                    "complete_ms": unix_millis().saturating_sub(started_at_unix_ms),
                },
                "catalog": catalog,
            }));
        }
        if unix_millis() >= deadline_unix_ms {
            return classified(
                "alpha_catalog_creation_timeout",
                format!("background catalog refresh missed its deadline; last_status={last}"),
            );
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn run_ui_journey(
    device: &mut DeviceClient,
    nonce: &mut Option<String>,
    output: &Path,
) -> AgentResult<(Vec<Value>, Value, Vec<UsbEvidence>)> {
    let rgb_dir = output.join("rgb565");
    let usb_dir = output.join("usb-video");
    fs::create_dir_all(&rgb_dir).map_err(|error| error.to_string())?;
    fs::create_dir_all(&usb_dir).map_err(|error| error.to_string())?;
    let mut checkpoints = Vec::new();
    let mut usb = Vec::new();

    let home = tap(device, nonce, AutomationButton::Home)?;
    let state = snapshot(device, nonce)?;
    require_semantic(&state, "effective_view", "home")?;
    require_bool(&state, "catalog_ready", true)?;
    checkpoints.push(checkpoint(device, nonce, home, "home", &rgb_dir)?);
    usb.push(capture_usb("home", &usb_dir)?);

    select_home_item(device, nonce, "menu:arcade")?;
    let arcade = tap(device, nonce, AutomationButton::A)?;
    let state = snapshot(device, nonce)?;
    require_semantic(&state, "effective_view", "arcade")?;
    require_nonzero(&state, "selected_count")?;
    require_nonempty(&state, "selected_game_id")?;
    checkpoints.push(checkpoint(device, nonce, arcade, "arcade", &rgb_dir)?);
    usb.push(capture_usb("arcade", &usb_dir)?);

    let before_index = semantic(&state, "selected_index")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let velocity = action(
        device,
        nonce,
        AutomationAction::Hold {
            button: AutomationButton::Down,
            duration_ms: 350,
        },
    )?;
    std::thread::sleep(std::time::Duration::from_millis(400));
    let velocity_settled = action(device, nonce, AutomationAction::ReleaseAll)?;
    let state = snapshot(device, nonce)?;
    if semantic(&state, "selected_count")
        .and_then(Value::as_u64)
        .is_some_and(|count| count > 1)
        && semantic(&state, "selected_index").and_then(Value::as_u64) == Some(before_index)
    {
        return classified("alpha_ui_assertion_failed", "arcade velocity did not move");
    }
    checkpoints.push(checkpoint(
        device,
        nonce,
        velocity.max(velocity_settled),
        "arcade-velocity",
        &rgb_dir,
    )?);

    let pre_launch = snapshot(device, nonce)?;
    let expected_game_id = semantic(&pre_launch, "selected_game_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("arcade launch has no selected game")?
        .to_owned();
    let launch_nonce = nonce.take().ok_or("automation session is not active")?;
    let launch_return: Value = serde_json::from_str(&device.execute(
        DeviceRequest::ExerciseLauncherAutomationLaunchReturn {
            nonce: launch_nonce,
            expected_game_id: expected_game_id.clone(),
            lifetime_seconds: 120,
        },
    )?)
    .map_err(|error| format!("cannot parse launch-return evidence: {error}"))?;
    let replacement_nonce = launch_return
        .get("nonce")
        .and_then(Value::as_str)
        .ok_or("launch-return evidence has no replacement nonce")?
        .to_owned();
    let return_sequence = launch_return
        .get("post_return_action_sequence")
        .and_then(Value::as_u64)
        .ok_or("launch-return evidence has no presented sequence")?;
    *nonce = Some(replacement_nonce);
    let returned_arcade = snapshot(device, nonce)?;
    require_semantic(&returned_arcade, "effective_view", "arcade")?;
    require_semantic(&returned_arcade, "selected_game_id", &expected_game_id)?;
    checkpoints.push(checkpoint(
        device,
        nonce,
        return_sequence,
        "arcade-return",
        &rgb_dir,
    )?);
    usb.push(capture_usb("arcade-return", &usb_dir)?);

    tap(device, nonce, AutomationButton::Left)?;
    require_bool(&snapshot(device, nonce)?, "drawer_open", true)?;
    tap(device, nonce, AutomationButton::B)?;
    require_semantic(&snapshot(device, nonce)?, "drawer_level", "Filters")?;
    tap(device, nonce, AutomationButton::Down)?;
    let search = tap(device, nonce, AutomationButton::A)?;
    require_bool(&snapshot(device, nonce)?, "search_active", true)?;
    let typed = tap(device, nonce, AutomationButton::A)?;
    let search_state = await_semantic(device, nonce, "search_query", "A")?;
    require_bool(&search_state, "search_active", true)?;
    checkpoints.push(checkpoint(
        device,
        nonce,
        typed.max(search),
        "arcade-search",
        &rgb_dir,
    )?);

    let returned = tap(device, nonce, AutomationButton::Home)?;
    require_semantic(&snapshot(device, nonce)?, "effective_view", "home")?;
    checkpoints.push(checkpoint(
        device,
        nonce,
        returned,
        "post-navigation",
        &rgb_dir,
    )?);
    usb.push(capture_usb("home-restored", &usb_dir)?);

    let root = snapshot(device, nonce)?;
    let root_count = semantic(&root, "selected_count")
        .and_then(Value::as_u64)
        .ok_or("root menu has no selected count")?;
    let mut root_state = root;
    for _ in 0..root_count {
        if semantic(&root_state, "selected_item_id")
            .and_then(Value::as_str)
            .is_some_and(|item| item.starts_with("menu:") && item != "menu:arcade")
        {
            break;
        }
        tap(device, nonce, AutomationButton::Right)?;
        root_state = snapshot(device, nonce)?;
    }
    if !semantic(&root_state, "selected_item_id")
        .and_then(Value::as_str)
        .is_some_and(|item| item.starts_with("menu:") && item != "menu:arcade")
    {
        return classified(
            "alpha_ui_assertion_failed",
            "root catalog has no nested hierarchy",
        );
    }
    let nested = tap(device, nonce, AutomationButton::A)?;
    let nested_state = snapshot(device, nonce)?;
    if semantic(&nested_state, "menu_id").and_then(Value::as_str) == Some("menu:root") {
        return classified("alpha_ui_assertion_failed", "menu hierarchy did not open");
    }
    let nested_menu = semantic(&nested_state, "menu_id")
        .and_then(Value::as_str)
        .ok_or("nested menu has no identity")?
        .to_owned();
    let mut nested_checkpoint_sequence = nested;
    if semantic(&nested_state, "selected_count")
        .and_then(Value::as_u64)
        .is_some_and(|count| count > 1)
    {
        nested_checkpoint_sequence = tap(device, nonce, AutomationButton::Right)?;
    }
    let remembered_item = semantic(&snapshot(device, nonce)?, "selected_item_id")
        .and_then(Value::as_str)
        .ok_or("nested menu has no selected item")?
        .to_owned();
    checkpoints.push(checkpoint(
        device,
        nonce,
        nested_checkpoint_sequence,
        "nested-menu",
        &rgb_dir,
    )?);
    tap(device, nonce, AutomationButton::B)?;
    require_semantic(&snapshot(device, nonce)?, "menu_id", "menu:root")?;
    tap(device, nonce, AutomationButton::A)?;
    let restored_nested = snapshot(device, nonce)?;
    require_semantic(&restored_nested, "menu_id", &nested_menu)?;
    require_semantic(&restored_nested, "selected_item_id", &remembered_item)?;
    tap(device, nonce, AutomationButton::B)?;

    tap(device, nonce, AutomationButton::Up)?;
    let settings = tap(device, nonce, AutomationButton::A)?;
    require_semantic(&snapshot(device, nonce)?, "effective_view", "settings")?;
    checkpoints.push(checkpoint(device, nonce, settings, "settings", &rgb_dir)?);
    tap(device, nonce, AutomationButton::Down)?;
    tap(device, nonce, AutomationButton::B)?;
    require_semantic(&snapshot(device, nonce)?, "effective_view", "home")?;

    Ok((checkpoints, launch_return, usb))
}

fn select_home_item(
    device: &mut DeviceClient,
    nonce: &Option<String>,
    expected_item_id: &str,
) -> AgentResult<()> {
    let initial = snapshot(device, nonce)?;
    let count = semantic(&initial, "selected_count")
        .and_then(Value::as_u64)
        .ok_or("home menu has no selected count")?;
    let mut state = initial;
    for _ in 0..count {
        if semantic(&state, "selected_item_id").and_then(Value::as_str) == Some(expected_item_id) {
            return Ok(());
        }
        tap(device, nonce, AutomationButton::Right)?;
        state = snapshot(device, nonce)?;
    }
    classified(
        "alpha_ui_assertion_failed",
        format!("home menu has no {expected_item_id} item"),
    )
}

fn tap(
    device: &mut DeviceClient,
    nonce: &Option<String>,
    button: AutomationButton,
) -> AgentResult<u64> {
    action(device, nonce, AutomationAction::Tap(button))
}

fn action(
    device: &mut DeviceClient,
    nonce: &Option<String>,
    action: AutomationAction,
) -> AgentResult<u64> {
    let response: Value = serde_json::from_str(&device.execute(
        DeviceRequest::SendLauncherAutomationAction {
            nonce: active_nonce(nonce)?.to_owned(),
            action,
        },
    )?)
    .map_err(|error| format!("cannot parse automation action: {error}"))?;
    let sequence = response
        .get("action_sequence")
        .and_then(Value::as_u64)
        .ok_or("automation action has no sequence")?;
    device.execute(DeviceRequest::AwaitLauncherAutomationPresented {
        nonce: active_nonce(nonce)?.to_owned(),
        action_sequence: sequence,
        timeout_ms: 3_000,
    })?;
    Ok(sequence)
}

fn snapshot(device: &mut DeviceClient, nonce: &Option<String>) -> AgentResult<Value> {
    serde_json::from_str(
        &device.execute(DeviceRequest::ReadLauncherAutomationSnapshot {
            nonce: active_nonce(nonce)?.to_owned(),
        })?,
    )
    .map_err(|error| format!("cannot parse automation snapshot: {error}").into())
}

fn checkpoint(
    device: &mut DeviceClient,
    nonce: &Option<String>,
    sequence: u64,
    label: &str,
    output: &Path,
) -> AgentResult<Value> {
    serde_json::from_str(
        &device.execute(DeviceRequest::CaptureLauncherAutomationCheckpoint {
            nonce: active_nonce(nonce)?.to_owned(),
            action_sequence: sequence,
            label: label.to_owned(),
            output_dir: output.to_owned(),
        })?,
    )
    .map_err(|error| format!("cannot parse checkpoint {label}: {error}").into())
}

fn await_semantic(
    device: &mut DeviceClient,
    nonce: &Option<String>,
    field: &str,
    expected: &str,
) -> AgentResult<Value> {
    for _ in 0..100 {
        let value = snapshot(device, nonce)?;
        if semantic(&value, field).and_then(Value::as_str) == Some(expected) {
            return Ok(value);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    classified(
        "alpha_ui_assertion_failed",
        format!("{field} did not become {expected}"),
    )
}

fn active_nonce(nonce: &Option<String>) -> AgentResult<&str> {
    nonce
        .as_deref()
        .ok_or_else(|| "automation session is not active".into())
}

fn require_installed_candidate(
    status: &Value,
    candidate: &CandidateIdentity,
) -> AgentResult<Value> {
    let runtime = status
        .pointer("/runtime/slint_status")
        .filter(|value| value.is_object())
        .ok_or("device status has no Slint runtime identity")?;
    let actual_version = runtime.pointer("/build/version").and_then(Value::as_str);
    let actual_revision = runtime
        .pointer("/build/source_revision")
        .and_then(Value::as_str);
    if actual_version != Some(&candidate.version)
        || actual_revision != Some(&candidate.magik_revision)
    {
        return classified(
            "alpha_candidate_not_installed",
            format!(
                "running UI identity does not match candidate: expected version={} revision={} actual version={} revision={}",
                candidate.version,
                candidate.magik_revision,
                actual_version.unwrap_or("missing"),
                actual_revision.unwrap_or("missing"),
            ),
        );
    }
    Ok(runtime.clone())
}

fn semantic<'a>(snapshot: &'a Value, field: &str) -> Option<&'a Value> {
    snapshot.get("semantic")?.get(field)
}

fn require_semantic(snapshot: &Value, field: &str, expected: &str) -> AgentResult<()> {
    if semantic(snapshot, field).and_then(Value::as_str) == Some(expected) {
        Ok(())
    } else {
        classified(
            "alpha_ui_assertion_failed",
            format!("{field} is not {expected}"),
        )
    }
}

fn require_bool(snapshot: &Value, field: &str, expected: bool) -> AgentResult<()> {
    if semantic(snapshot, field).and_then(Value::as_bool) == Some(expected) {
        Ok(())
    } else {
        classified(
            "alpha_ui_assertion_failed",
            format!("{field} is not {expected}"),
        )
    }
}

fn require_nonzero(snapshot: &Value, field: &str) -> AgentResult<()> {
    if semantic(snapshot, field)
        .and_then(Value::as_u64)
        .is_some_and(|value| value > 0)
    {
        Ok(())
    } else {
        classified("alpha_ui_assertion_failed", format!("{field} is empty"))
    }
}

fn require_nonempty(snapshot: &Value, field: &str) -> AgentResult<()> {
    if semantic(snapshot, field)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
    {
        Ok(())
    } else {
        classified("alpha_ui_assertion_failed", format!("{field} is empty"))
    }
}

fn capture_usb(label: &str, output: &Path) -> AgentResult<UsbEvidence> {
    let path = output.join(format!("{label}.jpg"));
    let artifact = crate::capture::execute(Some(&path))?;
    Ok(UsbEvidence {
        label: label.to_owned(),
        path: format!("usb-video/{label}.jpg"),
        bytes: artifact.bytes,
        width: artifact.width,
        height: artifact.height,
        sha256: digest_file(&artifact.path)?,
    })
}

pub fn verify_acceptance(
    candidate_root: &Path,
    receipt_path: &Path,
    marker_path: &Path,
) -> AgentResult<PathBuf> {
    let candidate = verify_candidate(candidate_root)?;
    let receipt_bytes = fs::read(receipt_path)
        .map_err(|error| format!("cannot read {}: {error}", receipt_path.display()))?;
    let receipt: Value = serde_json::from_slice(&receipt_bytes)
        .map_err(|error| format!("cannot parse {}: {error}", receipt_path.display()))?;
    if receipt.get("format").and_then(Value::as_str) != Some("mister-magik-alpha-hil-v1")
        || receipt.get("accepted").and_then(Value::as_bool) != Some(true)
    {
        return invalid_acceptance("receipt is not an accepted alpha HIL result");
    }
    let expected_candidate = serde_json::to_value(&candidate).unwrap();
    if receipt.get("candidate") != Some(&expected_candidate) {
        return invalid_acceptance("receipt candidate identity does not match immutable assets");
    }
    let runtime = receipt
        .get("installed_runtime")
        .ok_or_else(|| AgentError::from("receipt has no installed runtime"))?;
    if runtime.pointer("/build/version").and_then(Value::as_str) != Some(&candidate.version)
        || runtime
            .pointer("/build/source_revision")
            .and_then(Value::as_str)
            != Some(&candidate.magik_revision)
    {
        return invalid_acceptance("installed runtime identity does not match the candidate");
    }
    let catalog = receipt
        .get("catalog_creation")
        .ok_or("receipt has no catalog-creation evidence")?;
    if catalog.get("schema").and_then(Value::as_str)
        != Some("mister-magik-alpha-catalog-creation-v1")
        || catalog.pointer("/catalog/valid").and_then(Value::as_bool) != Some(true)
        || catalog
            .pointer("/catalog/total_games")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            == 0
        || catalog
            .pointer("/timing/complete_ms")
            .and_then(Value::as_u64)
            .is_none()
    {
        return invalid_acceptance("catalog-creation evidence is invalid");
    }
    let launch_return = receipt
        .get("launch_return")
        .ok_or_else(|| AgentError::from("receipt has no launch-return evidence"))?;
    if launch_return.get("schema").and_then(Value::as_str)
        != Some("mister-magik-launcher-automation-launch-return-v1")
        || launch_return
            .pointer("/handoff/files/core_name")
            .and_then(Value::as_str)
            .or_else(|| {
                launch_return
                    .pointer("/handoff/files/rbf_name")
                    .and_then(Value::as_str)
            })
            .is_none_or(|core| core.is_empty() || core.to_ascii_lowercase().contains("menu"))
    {
        return invalid_acceptance("receipt does not prove a real non-Menu core launch and return");
    }

    let evidence_root = receipt_path
        .parent()
        .ok_or("acceptance receipt has no evidence directory")?;
    verify_checkpoint_evidence(
        evidence_root,
        receipt
            .get("checkpoints")
            .and_then(Value::as_array)
            .ok_or("receipt has no checkpoint array")?,
    )?;
    verify_usb_evidence(
        evidence_root,
        receipt
            .get("usb_video")
            .and_then(Value::as_array)
            .ok_or("receipt has no USB evidence array")?,
    )?;

    let marker = json_map(&[
        (
            "schema",
            Value::String("mister-magik-alpha-acceptance-marker-v1".into()),
        ),
        (
            "candidate_tag",
            Value::String(candidate.candidate_tag.clone()),
        ),
        ("version", Value::String(candidate.version.clone())),
        ("build_number", Value::from(candidate.build_number)),
        (
            "source_revision",
            Value::String(candidate.magik_revision.clone()),
        ),
        (
            "archive_sha256",
            Value::String(candidate.archive_sha256.clone()),
        ),
        (
            "acceptance_receipt_sha256",
            Value::String(digest(&receipt_bytes)),
        ),
        (
            "accepted_at_unix",
            receipt
                .get("accepted_at_unix")
                .cloned()
                .unwrap_or(Value::Null),
        ),
    ]);
    write_value_atomically(marker_path, &marker)?;
    Ok(marker_path.to_owned())
}

fn verify_checkpoint_evidence(root: &Path, checkpoints: &[Value]) -> AgentResult<()> {
    if checkpoints.len() < 6 {
        return invalid_acceptance("receipt has too few visual checkpoints");
    }
    let mut labels = BTreeSet::new();
    for checkpoint in checkpoints {
        if checkpoint.get("schema").and_then(Value::as_str)
            != Some("mister-magik-launcher-checkpoint-v1")
        {
            return invalid_acceptance("checkpoint schema is invalid");
        }
        let label = checkpoint
            .get("label")
            .and_then(Value::as_str)
            .ok_or("checkpoint has no label")?;
        require_evidence_leaf(label)?;
        if !labels.insert(label.to_owned()) {
            return invalid_acceptance("checkpoint labels are not unique");
        }
        let png = checkpoint
            .get("png")
            .and_then(Value::as_str)
            .ok_or("checkpoint has no PNG")?;
        require_evidence_leaf(png)?;
        let png_path = root.join("rgb565").join(png);
        verify_evidence_file(
            &png_path,
            checkpoint.get("png_bytes").and_then(Value::as_u64),
            checkpoint.get("png_sha256").and_then(Value::as_str),
        )?;
        let metadata_path = root.join("rgb565").join(format!("{label}.json"));
        let metadata: Value = serde_json::from_slice(
            &fs::read(&metadata_path)
                .map_err(|error| format!("cannot read {}: {error}", metadata_path.display()))?,
        )
        .map_err(|error| format!("cannot parse {}: {error}", metadata_path.display()))?;
        if metadata != *checkpoint {
            return invalid_acceptance("checkpoint metadata does not match its receipt entry");
        }
    }
    for required in [
        "home",
        "arcade",
        "arcade-return",
        "arcade-search",
        "nested-menu",
        "settings",
    ] {
        if !labels.contains(required) {
            return invalid_acceptance(format!("receipt is missing checkpoint {required}"));
        }
    }
    Ok(())
}

fn verify_usb_evidence(root: &Path, evidence: &[Value]) -> AgentResult<()> {
    if evidence.len() < 3 {
        return invalid_acceptance("receipt has too few physical USB captures");
    }
    let mut labels = BTreeSet::new();
    for item in evidence {
        let label = item
            .get("label")
            .and_then(Value::as_str)
            .ok_or("USB evidence has no label")?;
        require_evidence_leaf(label)?;
        labels.insert(label.to_owned());
        let relative = item
            .get("path")
            .and_then(Value::as_str)
            .ok_or("USB evidence has no path")?;
        require_relative(relative)?;
        if !relative.starts_with("usb-video/") {
            return invalid_acceptance("USB evidence path is outside its evidence directory");
        }
        verify_evidence_file(
            &root.join(relative),
            item.get("bytes").and_then(Value::as_u64),
            item.get("sha256").and_then(Value::as_str),
        )?;
        if item.get("width").and_then(Value::as_u64) != Some(1920)
            || item.get("height").and_then(Value::as_u64) != Some(1080)
        {
            return invalid_acceptance("USB evidence is not the required 1920x1080 capture");
        }
    }
    for required in ["home", "arcade", "arcade-return"] {
        if !labels.contains(required) {
            return invalid_acceptance(format!("receipt is missing USB capture {required}"));
        }
    }
    Ok(())
}

fn verify_evidence_file(path: &Path, bytes: Option<u64>, sha256: Option<&str>) -> AgentResult<()> {
    let expected_bytes = bytes.ok_or("evidence entry has no byte length")?;
    let expected_sha = sha256.ok_or("evidence entry has no SHA-256")?;
    require_sha("evidence_sha256", expected_sha)?;
    if fs::metadata(path).map(|metadata| metadata.len()).ok() != Some(expected_bytes)
        || digest_file(path)? != expected_sha
    {
        return invalid_acceptance(format!("evidence file does not match: {}", path.display()));
    }
    Ok(())
}

fn require_evidence_leaf(value: &str) -> AgentResult<()> {
    if value.is_empty()
        || value.len() > 128
        || Path::new(value).components().count() != 1
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        invalid_acceptance("evidence contains an unsafe filename")
    } else {
        Ok(())
    }
}

fn json_map(entries: &[(&str, Value)]) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect(),
    )
}

fn write_value_atomically(path: &Path, value: &Value) -> AgentResult<()> {
    if path.exists() {
        return classified("alpha_evidence_exists", path.display().to_string());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension(format!("json.partial-{}", std::process::id()));
    fs::write(
        &temporary,
        format!("{}\n", serde_json::to_string_pretty(value).unwrap()),
    )
    .map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    Ok(())
}

fn invalid_acceptance<T>(detail: impl Into<String>) -> AgentResult<T> {
    classified("invalid_alpha_acceptance_receipt", detail)
}

fn write_receipt_atomically(path: &Path, receipt: &AcceptanceReceipt) -> AgentResult<()> {
    if path.exists() {
        return classified("alpha_evidence_exists", path.display().to_string());
    }
    let temporary = path.with_extension(format!("json.partial-{}", std::process::id()));
    if temporary.exists() {
        return classified("alpha_evidence_exists", temporary.display().to_string());
    }
    let bytes = format!("{}\n", serde_json::to_string_pretty(receipt).unwrap());
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    Ok(())
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u128::from(u64::MAX)) as u64
        })
}

pub fn verify_candidate(root: &Path) -> AgentResult<CandidateIdentity> {
    let receipt_path = root.join("release-assets.json");
    let receipt_bytes = fs::read(&receipt_path)
        .map_err(|error| format!("cannot read {}: {error}", receipt_path.display()))?;
    let receipt: AssetReceipt = serde_json::from_slice(&receipt_bytes)
        .map_err(|error| format!("cannot parse {}: {error}", receipt_path.display()))?;
    if receipt.format != ASSET_FORMAT || receipt.version != format!("0.2.{}", receipt.build_number)
    {
        return classified(
            "invalid_alpha_candidate",
            "release identity is inconsistent",
        );
    }
    require_leaf("archive", &receipt.archive)?;
    require_sha("archive_sha256", &receipt.archive_sha256)?;
    let checksums = verify_checksums(root)?;
    if !checksums.contains("mister-magik-alpha-db.json.zip") {
        return classified(
            "invalid_alpha_candidate",
            "candidate checksums do not cover the alpha Downloader database",
        );
    }

    let archive_path = root.join(&receipt.archive);
    if digest_file(&archive_path)? != receipt.archive_sha256 {
        return classified("alpha_candidate_hash_mismatch", receipt.archive);
    }
    let archive = read_distribution_zip(&archive_path)?;
    let mut expected = BTreeSet::new();
    for entry in &receipt.files {
        require_relative(&entry.path)?;
        require_leaf("asset", &entry.asset)?;
        require_sha("asset_sha256", &entry.sha256)?;
        if !expected.insert(entry.path.clone()) {
            return classified(
                "invalid_alpha_candidate",
                format!("duplicate {}", entry.path),
            );
        }
        let bytes = archive
            .get(&entry.path)
            .ok_or_else(|| AgentError::Classified {
                code: "alpha_candidate_archive_mismatch",
                detail: format!("archive is missing {}", entry.path),
            })?;
        if bytes.len() as u64 != entry.size || digest(bytes) != entry.sha256 {
            return classified("alpha_candidate_hash_mismatch", entry.path.clone());
        }
        let asset_path = if root.join("files").is_dir() {
            root.join("files").join(&entry.asset)
        } else {
            root.join(&entry.asset)
        };
        if fs::metadata(&asset_path).map(|value| value.len()).ok() != Some(entry.size)
            || digest_file(&asset_path)? != entry.sha256
        {
            return classified("alpha_candidate_asset_mismatch", entry.asset.clone());
        }
    }
    if archive.keys().cloned().collect::<BTreeSet<_>>() != expected {
        return classified(
            "alpha_candidate_archive_mismatch",
            "archive and release receipt contain different files",
        );
    }

    let release = parse_fields(member(&archive, "mister-magik/release-v1.txt")?)?;
    if release.get("format").map(String::as_str) != Some(RELEASE_FORMAT)
        || release.get("version") != Some(&receipt.version)
        || release.get("build_number") != Some(&receipt.build_number.to_string())
    {
        return classified("invalid_alpha_candidate", "release-v1 identity disagrees");
    }
    let manifest_bytes = member(&archive, "mister-magik/platform-v3.manifest")?;
    let manifest_text = std::str::from_utf8(manifest_bytes)
        .map_err(|error| format!("platform manifest is not UTF-8: {error}"))?;
    let manifest = platform_manifest::parse_installed(manifest_text, Layout::Public)?;
    let gui = member(&archive, "mister-magik/mister-magik-fb")?;
    if digest(gui) != manifest.gui_sha256()
        || release.get("magik_revision").map(String::as_str) != Some(manifest.magik_revision())
    {
        return classified(
            "alpha_candidate_identity_mismatch",
            "runtime and platform manifest disagree",
        );
    }

    let candidate_tag = format!(
        "alpha-candidate-v{}-{}",
        receipt.version,
        &receipt.archive_sha256[..12]
    );
    Ok(CandidateIdentity {
        format: "mister-magik-alpha-candidate-v1",
        version: receipt.version,
        build_number: receipt.build_number,
        candidate_tag,
        archive: receipt.archive,
        archive_sha256: receipt.archive_sha256,
        release_assets_sha256: digest(&receipt_bytes),
        magik_revision: manifest.magik_revision().to_owned(),
        gui_sha256: manifest.gui_sha256().to_owned(),
        platform_manifest_sha256: digest(manifest_bytes),
        platform_bundle_id: manifest.platform_bundle_id().to_owned(),
        qualification_candidate_id: manifest.qualification_candidate_id().to_owned(),
        component_sha256: BTreeMap::from([
            ("main".into(), manifest.main_sha256().into()),
            ("gui".into(), manifest.gui_sha256().into()),
            ("manager".into(), manifest.manager_sha256().into()),
            (
                "scanout_module".into(),
                manifest.scanout_module_sha256().into(),
            ),
            (
                "scanout_metadata".into(),
                manifest.scanout_metadata_sha256().into(),
            ),
            ("latch_rbf".into(), manifest.latch_rbf_sha256().into()),
            (
                "latch_metadata".into(),
                manifest.latch_metadata_sha256().into(),
            ),
        ]),
    })
}

fn verify_checksums(root: &Path) -> AgentResult<BTreeSet<String>> {
    let path = root.join("SHA256SUMS");
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        let (sha, relative) = line
            .split_once("  ")
            .ok_or_else(|| "invalid SHA256SUMS line".to_owned())?;
        require_sha("checksum", sha)?;
        require_relative(relative)?;
        if !seen.insert(relative) || digest_file(&root.join(relative))? != sha {
            return classified("alpha_candidate_checksum_mismatch", relative);
        }
    }
    if !seen.contains("release-assets.json") {
        return classified(
            "invalid_alpha_candidate",
            "SHA256SUMS does not cover release-assets.json",
        );
    }
    Ok(seen.into_iter().map(str::to_string).collect())
}

fn candidate_hashes(candidate: &CandidateIdentity) -> AgentResult<AlphaCandidateHashes> {
    let component = |name: &str| -> AgentResult<String> {
        candidate
            .component_sha256
            .get(name)
            .cloned()
            .ok_or_else(|| AgentError::from(format!("candidate has no {name} component hash")))
    };
    Ok(AlphaCandidateHashes {
        platform_manifest: candidate.platform_manifest_sha256.clone(),
        main: component("main")?,
        gui: component("gui")?,
        manager: component("manager")?,
        scanout_module: component("scanout_module")?,
        scanout_metadata: component("scanout_metadata")?,
        latch_rbf: component("latch_rbf")?,
        latch_metadata: component("latch_metadata")?,
    })
}

fn parse_fields(bytes: &[u8]) -> AgentResult<BTreeMap<String, String>> {
    let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| "invalid release-v1 field".to_owned())?;
        if key.is_empty() || value.is_empty() || fields.insert(key.into(), value.into()).is_some() {
            return classified("invalid_alpha_candidate", "invalid release-v1 fields");
        }
    }
    Ok(fields)
}

fn member<'a>(archive: &'a BTreeMap<String, Vec<u8>>, path: &str) -> AgentResult<&'a [u8]> {
    archive
        .get(path)
        .map(Vec::as_slice)
        .ok_or_else(|| AgentError::Classified {
            code: "invalid_alpha_candidate",
            detail: format!("missing {path}"),
        })
}

fn require_leaf(field: &'static str, value: &str) -> AgentResult<()> {
    if value.is_empty() || Path::new(value).components().count() != 1 {
        classified(
            "invalid_alpha_candidate",
            format!("unsafe {field}: {value}"),
        )
    } else {
        Ok(())
    }
}

fn require_relative(value: &str) -> AgentResult<()> {
    let path = PathBuf::from(value);
    if value.is_empty()
        || value.starts_with('/')
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        classified("invalid_alpha_candidate", format!("unsafe path: {value}"))
    } else {
        Ok(())
    }
}

fn require_sha(field: &'static str, value: &str) -> AgentResult<()> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        classified("invalid_alpha_candidate", format!("invalid {field}"))
    }
}

fn digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    digest.iter().fold(
        String::with_capacity(digest.len() * 2),
        |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        },
    )
}

fn digest_file(path: &Path) -> AgentResult<String> {
    fs::read(path)
        .map(|bytes| digest(&bytes))
        .map_err(|error| format!("cannot read {}: {error}", path.display()).into())
}

fn classified<T>(code: &'static str, detail: impl Into<String>) -> AgentResult<T> {
    Err(AgentError::Classified {
        code,
        detail: detail.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_fields_reject_duplicates() {
        assert!(parse_fields(b"format=x\nformat=y\n").is_err());
    }

    #[test]
    fn candidate_paths_are_bounded() {
        assert!(require_relative("mister-magik/release-v1.txt").is_ok());
        assert!(require_relative("../release-v1.txt").is_err());
        assert!(require_leaf("archive", "candidate.zip").is_ok());
        assert!(require_leaf("archive", "nested/candidate.zip").is_err());
    }

    #[test]
    fn alpha_activation_requires_a_safe_restore_target() {
        assert_eq!(
            alpha_original_main(&json!({
                "schema": "mister-magik-alpha-candidate-activation-v1",
                "original_main": "MiSTer_MagiKDev",
            }))
            .unwrap(),
            Some("MiSTer_MagiKDev".into())
        );
        assert_eq!(
            alpha_original_main(&json!({
                "schema": "mister-magik-alpha-candidate-activation-v1",
                "original_main": null,
            }))
            .unwrap(),
            None
        );
        assert!(
            alpha_original_main(&json!({
                "schema": "mister-magik-alpha-candidate-activation-v1",
            }))
            .is_err()
        );
        assert!(
            alpha_original_main(&json!({
                "schema": "mister-magik-alpha-candidate-activation-v1",
                "original_main": "custom/unsafe",
            }))
            .is_err()
        );
    }

    #[test]
    fn alpha_activation_requires_first_visible_catalog_evidence() {
        let activation = json!({
            "catalog": {
                "schema": "mister-magik-alpha-catalog-start-v1",
                "mode": "built-or-upgraded",
                "started_at_unix_ms": 1000,
                "deadline_unix_ms": 481000,
                "timing": {"first_visible_ms": 1200},
                "first_visible": {"games": 42, "systems": 3},
            }
        });
        assert_eq!(
            alpha_catalog_start(&activation).unwrap(),
            activation["catalog"]
        );
        assert!(
            alpha_catalog_start(&json!({"catalog": {"first_visible": {"games": 42}}})).is_err()
        );
    }

    #[test]
    fn installed_candidate_uses_the_runtime_build_identity_object() {
        let candidate = CandidateIdentity {
            format: "mister-magik-alpha-candidate-v1",
            version: "0.2.2954".into(),
            build_number: 2954,
            candidate_tag: "alpha-candidate-v0.2.2954-test".into(),
            archive: "candidate.zip".into(),
            archive_sha256: "a".repeat(64),
            release_assets_sha256: "b".repeat(64),
            magik_revision: "c".repeat(40),
            gui_sha256: "d".repeat(64),
            platform_manifest_sha256: "e".repeat(64),
            platform_bundle_id: "bundle".into(),
            qualification_candidate_id: "f".repeat(64),
            component_sha256: BTreeMap::new(),
        };
        let status = json!({
            "runtime": {
                "slint_status": {
                    "build": {
                        "version": candidate.version.clone(),
                        "source_revision": candidate.magik_revision.clone(),
                    }
                }
            }
        });

        assert!(require_installed_candidate(&status, &candidate).is_ok());
        assert!(
            require_installed_candidate(
                &json!({"runtime": {"slint_status": {"build_version": "0.2.2954"}}}),
                &candidate,
            )
            .is_err()
        );
    }
}
