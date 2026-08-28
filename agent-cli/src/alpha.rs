// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::archive::read_distribution_zip;
use crate::device::DeviceClient;
use crate::error::{AgentError, AgentResult};
use crate::platform_manifest::{self, Layout};
use crate::progress::{EventKind, Reporter};
use crate::transport::{AlphaCandidateHashes, AutomationAction, AutomationButton};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
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
    evidence_mode: &'static str,
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

trait AlphaDevice {
    fn install_candidate(
        &mut self,
        tag: String,
        hashes: AlphaCandidateHashes,
        restore_on_failure: bool,
    ) -> AgentResult<Value>;
    fn restore_host_mode(&mut self, original_main: Option<String>) -> AgentResult<()>;
    fn ensure_launcher(&mut self, version: String, revision: String) -> AgentResult<()>;
    fn status(&mut self) -> AgentResult<Value>;
    fn inspect_public_catalog(&mut self) -> AgentResult<Value>;
    fn begin_automation(
        &mut self,
        version: String,
        revision: String,
        main_generation: u64,
        lifetime_seconds: u64,
    ) -> AgentResult<Value>;
    fn end_automation(&mut self, nonce: String) -> AgentResult<()>;
    fn action(&mut self, nonce: String, action: AutomationAction) -> AgentResult<u64>;
    fn snapshot(&mut self, nonce: String) -> AgentResult<Value>;
    fn checkpoint(
        &mut self,
        nonce: String,
        action_sequence: u64,
        label: String,
        output_dir: PathBuf,
    ) -> AgentResult<Value>;
    fn exercise_launch_return(
        &mut self,
        nonce: String,
        expected_game_id: String,
        lifetime_seconds: u64,
    ) -> AgentResult<Value>;
}

impl AlphaDevice for DeviceClient {
    fn install_candidate(
        &mut self,
        tag: String,
        hashes: AlphaCandidateHashes,
        restore_on_failure: bool,
    ) -> AgentResult<Value> {
        self.mutate(|device| device.install_alpha_candidate(&tag, &hashes, restore_on_failure))
    }

    fn restore_host_mode(&mut self, original_main: Option<String>) -> AgentResult<()> {
        self.mutate(|device| device.restore_alpha_host_mode(original_main))
    }

    fn ensure_launcher(&mut self, version: String, revision: String) -> AgentResult<()> {
        self.mutate(|device| device.ensure_installed_alpha_launcher(&version, &revision))
    }

    fn status(&mut self) -> AgentResult<Value> {
        self.read(crate::NativeDevice::status)
    }

    fn inspect_public_catalog(&mut self) -> AgentResult<Value> {
        self.read(crate::NativeDevice::inspect_public_catalog)
    }

    fn begin_automation(
        &mut self,
        version: String,
        revision: String,
        main_generation: u64,
        lifetime_seconds: u64,
    ) -> AgentResult<Value> {
        self.mutate(|device| {
            device.begin_launcher_automation(&version, &revision, main_generation, lifetime_seconds)
        })
    }

    fn end_automation(&mut self, nonce: String) -> AgentResult<()> {
        self.mutate(|device| device.end_launcher_automation(&nonce))
    }

    fn action(&mut self, nonce: String, action: AutomationAction) -> AgentResult<u64> {
        let sequence =
            self.mutate(|device| device.send_launcher_automation_action(&nonce, &action))?;
        self.read(|device| device.await_launcher_automation_presented(&nonce, sequence, 3_000))?;
        Ok(sequence)
    }

    fn snapshot(&mut self, nonce: String) -> AgentResult<Value> {
        self.read(|device| device.launcher_automation_snapshot(&nonce))
    }

    fn checkpoint(
        &mut self,
        nonce: String,
        action_sequence: u64,
        label: String,
        output_dir: PathBuf,
    ) -> AgentResult<Value> {
        self.mutate(|device| {
            device.capture_launcher_automation_checkpoint(
                &nonce,
                action_sequence,
                &label,
                &output_dir,
            )
        })
    }

    fn exercise_launch_return(
        &mut self,
        nonce: String,
        expected_game_id: String,
        lifetime_seconds: u64,
    ) -> AgentResult<Value> {
        self.mutate(|device| {
            device.exercise_launcher_automation_launch_return(
                &nonce,
                &expected_game_id,
                lifetime_seconds,
            )
        })
    }
}

pub fn execute(
    candidate_root: &Path,
    output: &Path,
    reuse_installed: bool,
    restore_host_mode: bool,
    framebuffer_only: bool,
    reporter: &mut Reporter<'_>,
) -> AgentResult<PathBuf> {
    reporter.emit(
        EventKind::Progress,
        "candidate",
        "Verifying the downloaded alpha release",
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
            "Installing the downloaded alpha through MiSTer Downloader",
            Some(10),
        )?;
        let activation = device.install_candidate(
            candidate.candidate_tag.clone(),
            candidate_hashes(&candidate)?,
            restore_host_mode,
        )?;
        (
            Some(alpha_original_main(&activation)?),
            alpha_catalog_start(&activation)?,
        )
    };
    let acceptance = accept_installed_candidate(
        &mut device,
        candidate,
        catalog_start,
        output,
        framebuffer_only,
        reporter,
    );
    let restored = if restore_host_mode {
        device.restore_host_mode(
            original_main.ok_or("alpha acceptance has no host-mode restore target")?,
        )
    } else {
        Ok(())
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
        "Alpha release passed the real-UI journey",
        Some(100),
    )?;
    Ok(receipt_path)
}

fn reuse_installed_catalog_start(
    device: &mut impl AlphaDevice,
    candidate: &CandidateIdentity,
) -> AgentResult<Value> {
    let started_at_unix_ms = unix_millis();
    let deadline_unix_ms = started_at_unix_ms.saturating_add(8 * 60 * 1_000);
    device.ensure_launcher(candidate.version.clone(), candidate.magik_revision.clone())?;
    let mut first = true;
    loop {
        let status = device.status()?;
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
    device: &mut impl AlphaDevice,
    candidate: CandidateIdentity,
    catalog_start: Value,
    output: &Path,
    framebuffer_only: bool,
    reporter: &mut Reporter<'_>,
) -> AgentResult<AcceptanceReceipt> {
    let status = device.status()?;
    let runtime = require_installed_candidate(&status, &candidate)?;
    fs::create_dir_all(output).map_err(|error| error.to_string())?;

    reporter.emit(
        EventKind::Progress,
        "ui",
        "Running the deterministic real-UI acceptance journey",
        Some(20),
    )?;
    let (checkpoints, launch_return, usb_video) =
        run_ui_journey(device, output, !framebuffer_only)?;
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
        evidence_mode: if framebuffer_only {
            "framebuffer-only"
        } else {
            "framebuffer-and-usb-video"
        },
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
    device: &mut impl AlphaDevice,
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
        let status = device.status()?;
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
            let catalog = device.inspect_public_catalog()?;
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
    _device: &mut impl AlphaDevice,
    output: &Path,
    _capture_usb_video: bool,
) -> AgentResult<(Vec<Value>, Value, Vec<UsbEvidence>)> {
    let repository = std::env::var_os("MISTER_UI_TEST_REPOSITORY")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .ok_or("UI test suite has no repository directory")?;
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    let fixture = std::env::var("MISTER_UI_TEST_FIXTURE")
        .unwrap_or_else(|_| "deterministic-arcade-v1".to_string());
    let cases = std::env::var("MISTER_UI_TEST_CASES")
        .unwrap_or_else(|_| "startup-home system-hub arcade-navigation arcade-filters settings-display screensaver-motion about-licenses effect-sandbox profile-matrix".to_string());
    let case_names = cases.split_whitespace().collect::<Vec<_>>();
    if case_names.is_empty()
        || case_names.iter().any(|case| {
            case.is_empty()
                || !case
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
    {
        return classified("alpha_ui_suite_invalid", "MISTER_UI_TEST_CASES is invalid");
    }
    let mut command =
        Command::new(std::env::var_os("MISTER_UI_TEST_RUNNER").unwrap_or_else(|| "uv".into()));
    command
        .arg("run")
        .arg("python")
        .arg("-m")
        .arg("apps.mister.ui_tests.suite");
    for case in &case_names {
        command.arg(case);
    }
    command
        .arg("--fixture")
        .arg(&fixture)
        .arg("--attended")
        .current_dir(&repository)
        .env("MISTER_UI_TEST_FIXTURE", &fixture)
        .env("MISTER_UI_TEST_REPOSITORY", &repository);
    if let Some(destination) = std::env::var_os("MISTER_UI_TEST_SSH_DESTINATION") {
        command.env("MISTER_UI_TEST_SSH_DESTINATION", destination);
    }
    let result = command.output().map_err(|error| error.to_string())?;
    let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    if !result.status.success() {
        return classified(
            "alpha_ui_suite_failed",
            format!("runner failed: stdout={stdout:?} stderr={stderr:?}"),
        );
    }
    let evidence = json!({
        "schema": "mister-magik-alpha-ui-suite-v1",
        "fixture": fixture,
        "cases": case_names,
        "stdout": stdout,
        "stderr": stderr,
    });
    let evidence_path = output.join("ui-suite.json");
    fs::write(&evidence_path, format!("{evidence}\n")).map_err(|error| error.to_string())?;
    Ok((vec![evidence.clone()], evidence, Vec::new()))
}

fn select_home_item(
    device: &mut impl AlphaDevice,
    nonce: &Option<String>,
    expected_item_id: &str,
) -> AgentResult<()> {
    let initial = snapshot(device, nonce)?;
    let count = semantic(&initial, "selected_count")
        .and_then(Value::as_u64)
        .ok_or("home menu has no selected count")?;
    let mut state = initial;
    let mut move_left = semantic(&state, "selected_index")
        .and_then(Value::as_u64)
        .is_some_and(|index| index > 0);
    for _ in 0..count.saturating_mul(2) {
        if semantic(&state, "selected_item_id").and_then(Value::as_str) == Some(expected_item_id) {
            return Ok(());
        }
        let index = semantic(&state, "selected_index")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if (move_left && index == 0) || (!move_left && index.saturating_add(1) >= count) {
            move_left = !move_left;
        }
        let previous = semantic(&state, "selected_item_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        state = tap_until_semantic_change(
            device,
            nonce,
            if move_left {
                AutomationButton::Left
            } else {
                AutomationButton::Right
            },
            "selected_item_id",
            &previous,
        )?;
    }
    classified(
        "alpha_ui_assertion_failed",
        format!("home menu has no {expected_item_id} item"),
    )
}

fn tap_until_semantic_change(
    device: &mut impl AlphaDevice,
    nonce: &Option<String>,
    button: AutomationButton,
    field: &str,
    previous: &str,
) -> AgentResult<Value> {
    for _ in 0..3 {
        tap(device, nonce, button)?;
        for _ in 0..100 {
            let value = snapshot(device, nonce)?;
            if semantic(&value, field).and_then(Value::as_str) != Some(previous) {
                return Ok(value);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    classified(
        "alpha_ui_assertion_failed",
        format!("{field} did not change from {previous} after bounded input retries"),
    )
}

fn tap(
    device: &mut impl AlphaDevice,
    nonce: &Option<String>,
    button: AutomationButton,
) -> AgentResult<u64> {
    action(device, nonce, AutomationAction::Tap(button))
}

fn action(
    device: &mut impl AlphaDevice,
    nonce: &Option<String>,
    action: AutomationAction,
) -> AgentResult<u64> {
    device.action(active_nonce(nonce)?.to_owned(), action)
}

fn snapshot(device: &mut impl AlphaDevice, nonce: &Option<String>) -> AgentResult<Value> {
    device.snapshot(active_nonce(nonce)?.to_owned())
}

fn checkpoint(
    device: &mut impl AlphaDevice,
    nonce: &Option<String>,
    sequence: u64,
    label: &str,
    output: &Path,
) -> AgentResult<Value> {
    device.checkpoint(
        active_nonce(nonce)?.to_owned(),
        sequence,
        label.to_owned(),
        output.to_owned(),
    )
}

fn await_semantic(
    device: &mut impl AlphaDevice,
    nonce: &Option<String>,
    field: &str,
    expected: &str,
) -> AgentResult<Value> {
    let mut last_actual = None;
    for _ in 0..100 {
        let value = snapshot(device, nonce)?;
        let actual = semantic(&value, field).and_then(Value::as_str);
        if actual == Some(expected) {
            return Ok(value);
        }
        last_actual = actual.map(str::to_owned);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    classified(
        "alpha_ui_assertion_failed",
        format!(
            "{field} did not become {expected}; actual={}",
            last_actual.as_deref().unwrap_or("missing")
        ),
    )
}

fn await_semantic_not(
    device: &mut impl AlphaDevice,
    nonce: &Option<String>,
    field: &str,
    unexpected: &str,
) -> AgentResult<Value> {
    let mut steady_since = None;
    for _ in 0..300 {
        let value = snapshot(device, nonce)?;
        if semantic(&value, field).and_then(Value::as_str) != Some(unexpected) {
            let since = steady_since.get_or_insert_with(std::time::Instant::now);
            if since.elapsed() >= std::time::Duration::from_millis(250) {
                return Ok(value);
            }
        } else {
            steady_since = None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    classified(
        "alpha_ui_assertion_failed",
        format!("{field} remained {unexpected}"),
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

fn maybe_capture_usb(
    capture_usb_video: bool,
    evidence: &mut Vec<UsbEvidence>,
    label: &str,
    output: &Path,
) -> AgentResult<()> {
    if capture_usb_video {
        evidence.push(capture_usb(label, output)?);
    }
    Ok(())
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

    Ok(CandidateIdentity {
        format: "mister-magik-alpha-release-v1",
        version: receipt.version,
        build_number: receipt.build_number,
        candidate_tag: "alpha".into(),
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
    use serde_json::json;

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
            format: "mister-magik-alpha-release-v1",
            version: "0.2.2954".into(),
            build_number: 2954,
            candidate_tag: "alpha".into(),
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

    #[test]
    fn semantic_acceptance_assertions_reject_missing_wrong_and_empty_values() {
        let snapshot = json!({
            "semantic": {
                "view": "arcade",
                "ready": true,
                "generation": 1,
                "selection": "game.mra",
            }
        });
        require_semantic(&snapshot, "view", "arcade").unwrap();
        require_bool(&snapshot, "ready", true).unwrap();
        require_nonzero(&snapshot, "generation").unwrap();
        require_nonempty(&snapshot, "selection").unwrap();
        assert!(require_semantic(&snapshot, "view", "settings").is_err());
        assert!(require_bool(&snapshot, "ready", false).is_err());
        assert!(require_nonzero(&json!({"semantic": {"generation": 0}}), "generation").is_err());
        assert!(require_nonempty(&json!({"semantic": {"selection": ""}}), "selection").is_err());
        assert!(active_nonce(&None).is_err());
        assert_eq!(active_nonce(&Some("nonce".into())).unwrap(), "nonce");
    }

    #[test]
    fn release_field_and_hash_validation_is_closed() {
        assert_eq!(
            parse_fields(b"format=release-v1\nversion=0.2.4\n").unwrap()["version"],
            "0.2.4"
        );
        assert!(parse_fields(b"missing-separator\n").is_err());
        assert!(parse_fields(b"=empty-key\n").is_err());
        assert!(require_sha("sha", &"a".repeat(64)).is_ok());
        assert!(require_sha("sha", &"z".repeat(64)).is_err());
        assert!(require_relative("/absolute").is_err());
        assert!(require_relative("nested/../escape").is_err());
    }
}
