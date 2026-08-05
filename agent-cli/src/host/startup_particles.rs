// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Volatile attended Magik/cabinet particle development sessions.

use super::remote::{connect_with, put, shell_quote as sh};
use super::{
    AttendedOperationSignalGuard, DeviceAccess, DeviceFailure, LauncherRestartOptions,
    NativeDevice, Result, acknowledged_main_command, attended_operation_interrupted,
    device_failure, exec_checked, file_sha256, install_prepared_device_environment, remote_read,
    restart_launcher_with_one_shot_env, wait_launcher_ready,
};
use crate::commands::device::StartupParticleRuntime;
use serde_json::Value;
use serde_json::json;
use ssh2::{ExtendedData, Session};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const REMOTE_DIR: &str = "/tmp/mister-magik/startup-particles";
const REMOTE_BINARY: &str =
    "/tmp/mister-magik/startup-particles/mister-magik-framebuffer-scene-lab";
const DEV_SCREENSHOT_ARCHIVE: &str =
    "/media/fat/mister-magik-dev/assets/arcade-screenshots-320x320.mmlz4b";
const REMOTE_LAB_RECIPE: &str = "/tmp/mister-magik/startup-particles/recipe.json";
const REMOTE_MAGIK_RECIPE: &str = "/tmp/mister-magik/startup-particles/magik.json";
const REMOTE_STATUS: &str = "/tmp/mister-magik/startup-particles/status.json";
const REMOTE_CARD_ASSESSMENT_DIR: &str = "/tmp/mister-magik/card-flip-assessment";
const DEVELOPMENT_LAUNCHER_ENV: &str = "/media/fat/mister-magik-dev/launcher.env";
const WATCH_INTERVAL: Duration = Duration::from_millis(100);
const ACK_DEADLINE: Duration = Duration::from_secs(1);
const LAUNCHER_START_DEADLINE: Duration = Duration::from_secs(45);
const MAGIK_SCHEMA: &str = "mister-magik-particle-magik-v1";
const CABINET_SCHEMA: &str = "mister-magik-particle-cabinet-v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LabDisplayContracts {
    pub(super) settings: String,
    pub(super) display: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SceneLabRequest<'a> {
    pub(crate) binary: &'a Path,
    pub(crate) scene: &'a str,
    pub(crate) recipe: Option<&'a Path>,
    pub(crate) fixture: Option<&'a str>,
    pub(crate) seed: Option<u64>,
    pub(crate) case: Option<&'a str>,
    pub(crate) profile: bool,
    pub(crate) assess: bool,
    pub(crate) output_dir: Option<&'a Path>,
}

#[derive(Debug)]
struct RemoteLabRequest {
    display_contracts: LabDisplayContracts,
    scene: String,
    has_recipe: bool,
    fixture: Option<String>,
    screenshot: Option<RemoteScreenshotArgs>,
    case: Option<String>,
    profile: bool,
    assess: bool,
    output_dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct RemoteScreenshotArgs {
    archive: &'static str,
    seed: u64,
    sampling_profile: &'static str,
}

pub(super) fn run(
    device: &mut NativeDevice,
    binary: Option<&Path>,
    recipe: &Path,
    runtime: StartupParticleRuntime,
) -> std::result::Result<(), DeviceFailure> {
    validate_local_input(recipe, "startup particle recipe").map_err(device_failure)?;
    if let Some(binary) = binary {
        validate_local_input(binary, "startup particle lab binary").map_err(device_failure)?;
    }
    let prepared = device.prepare(DeviceAccess::SSH_MUTATION)?;
    install_prepared_device_environment(&prepared.config);
    match runtime {
        StartupParticleRuntime::Lab => {
            let scene = local_recipe_scene(recipe).map_err(device_failure)?;
            run_lab(
                &prepared,
                SceneLabRequest {
                    binary: binary.ok_or_else(|| {
                        DeviceFailure::InvalidRequest(
                            "lab runtime requires a built lab binary".into(),
                        )
                    })?,
                    scene,
                    recipe: Some(recipe),
                    fixture: None,
                    seed: None,
                    case: None,
                    profile: false,
                    assess: false,
                    output_dir: None,
                },
            )
        }
        StartupParticleRuntime::DevLauncher => run_dev_launcher(&prepared, recipe),
    }
    .map_err(device_failure)
}

pub(super) fn run_scene_lab(
    device: &mut NativeDevice,
    request: SceneLabRequest<'_>,
) -> std::result::Result<(), DeviceFailure> {
    validate_local_input(request.binary, "framebuffer scene lab binary").map_err(device_failure)?;
    if let Some(recipe) = request.recipe {
        validate_local_input(recipe, "framebuffer scene recipe").map_err(device_failure)?;
    }
    let prepared = device.prepare(DeviceAccess::SSH_MUTATION)?;
    install_prepared_device_environment(&prepared.config);
    run_lab(&prepared, request).map_err(device_failure)
}

fn run_lab(prepared: &super::PreparedDevice, request: SceneLabRequest<'_>) -> Result<()> {
    let SceneLabRequest {
        binary,
        scene,
        recipe,
        fixture,
        seed,
        case,
        profile,
        assess,
        output_dir,
    } = request;
    let has_recipe = recipe.is_some();
    let session = connect_with(&prepared.config.connection, 10)?;
    let display_contracts = active_lab_display_contracts(&session)?;
    let screenshot = if scene == "screenshot-screensaver" {
        validate_installed_screenshot_archive(&session)?;
        Some(RemoteScreenshotArgs {
            archive: DEV_SCREENSHOT_ARCHIVE,
            seed: seed.unwrap_or(0x4d61_6769_4b54_696c),
            sampling_profile: if display_contracts.settings.contains("output=hdmi") {
                "hdmi"
            } else {
                "crt"
            },
        })
    } else {
        None
    };
    if let Err(error) = prepare_lab_files(
        &session,
        binary,
        recipe,
        scene,
        fixture,
        screenshot.as_ref(),
    ) {
        let cleanup = remove_volatile_directory(&session);
        return combine_results(Err(error), cleanup);
    }
    if assess {
        let Some(output_dir) = output_dir else {
            let cleanup = remove_volatile_directory(&session);
            return combine_results(
                Err("card assessment output directory is missing".into()),
                cleanup,
            );
        };
        if let Err(error) = prepare_card_assessment_output(output_dir, binary) {
            let cleanup = remove_volatile_directory(&session);
            return combine_results(Err(error), cleanup);
        }
    }
    let mut publisher = match recipe
        .filter(|_| case.is_none())
        .map(|recipe| RecipePublisher::new(recipe, REMOTE_LAB_RECIPE))
        .transpose()
    {
        Ok(publisher) => publisher,
        Err(error) => {
            let cleanup = remove_volatile_directory(&session);
            return combine_results(Err(error), cleanup);
        }
    };
    let _signal_guard = AttendedOperationSignalGuard::install();
    let run_config = prepared.config.connection.clone();
    let output_dir = output_dir.map(Path::to_path_buf);
    let scene = scene.to_owned();
    let fixture = fixture.map(str::to_owned);
    let case = case.map(str::to_owned);
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let worker = match thread::Builder::new()
        .name("framebuffer-scene-lab-device".into())
        .spawn(move || {
            let result = run_remote_lab(
                &run_config,
                RemoteLabRequest {
                    display_contracts,
                    scene,
                    has_recipe,
                    fixture,
                    screenshot,
                    case,
                    profile,
                    assess,
                    output_dir,
                },
            )
            .map_err(|error| error.to_string());
            let _ = finished_tx.send(result);
        }) {
        Ok(worker) => worker,
        Err(error) => {
            let cleanup = remove_volatile_directory(&session);
            return combine_results(
                Err(format!("start startup particle lab worker: {error}").into()),
                cleanup,
            );
        }
    };
    let mut stop_required = false;
    let watch_result = loop {
        match finished_rx.recv_timeout(WATCH_INTERVAL) {
            Ok(result) => break result.map_err(Into::into),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err("startup particle lab worker disconnected".into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if attended_operation_interrupted() {
            stop_required = true;
            break Ok(());
        }
        if let Some(publisher) = publisher.as_mut()
            && let Err(error) = publisher.poll(&session)
        {
            stop_required = true;
            break Err(error);
        }
    };
    let stop_result = if stop_required {
        stop_remote_lab_connection(&prepared.config.connection)
    } else {
        Ok(())
    };
    let (worker_result, worker_completed) = if stop_required {
        match finished_rx.recv_timeout(Duration::from_secs(45)) {
            Ok(result) => (result.map_err(Into::into), true),
            Err(mpsc::RecvTimeoutError::Timeout) => (
                Err("startup particle lab did not stop within 45 seconds".into()),
                false,
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => (
                Err("startup particle lab worker disconnected during cleanup".into()),
                true,
            ),
        }
    } else {
        (Ok(()), true)
    };
    let join_result = if worker_completed {
        worker
            .join()
            .map_err(|_| "startup particle lab worker panicked".into())
    } else {
        Err("startup particle lab worker could not be joined after bounded cleanup".into())
    };
    let run_result = combine_results(
        combine_results(watch_result, stop_result),
        combine_results(worker_result, join_result),
    );
    let run_result = combine_results(run_result, remove_volatile_directory(&session));
    let launcher_result = wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45));
    let safety_result = launcher_result.and_then(|_| verify_safety_clear(&session));
    combine_results(run_result, safety_result)
}

pub(super) fn active_lab_display_contracts(session: &Session) -> Result<LabDisplayContracts> {
    let reply = super::exec_checked_output(
        session,
        "query startup particle display mode",
        &acknowledged_main_command("mister_magik_display_get_v1"),
    )?;
    let reply = reply.stdout.trim();
    if super::parse_display_reply_pending(reply)?.is_some() {
        return Err("startup particle lab cannot run during a display transaction".into());
    }
    let active = super::parse_display_reply_active(reply)?;
    if active != "custom"
        && !super::DISPLAY_MATRIX_MODES
            .iter()
            .any(|mode| mode.id == active)
    {
        return Err(format!("unsupported active display mode {active}").into());
    }
    let settings = super::exec_checked_output(
        session,
        "query startup particle resolved output route",
        &acknowledged_main_command("mister_magik_settings_get_v1"),
    )?;
    let output = settings
        .stdout
        .split_ascii_whitespace()
        .find_map(|field| field.strip_prefix("output="))
        .ok_or("resolved settings reply omitted output")?;
    let settings = format!("schema=1&output={output}");
    if !matches!(
        output,
        "hdmi" | "crt-240p60" | "crt-288p50" | "crt-480p60" | "crt-576p50"
    ) {
        return Err(format!("unsupported resolved output route {output}").into());
    }
    Ok(LabDisplayContracts {
        settings,
        display: format!("schema=1&mode={active}"),
    })
}

fn run_dev_launcher(prepared: &super::PreparedDevice, recipe: &Path) -> Result<()> {
    let session = connect_with(&prepared.config.connection, 10)?;
    exec_checked(
        &session,
        "startup particle Dev launcher preflight",
        &dev_preflight_command(),
    )?;
    let _signal_guard = AttendedOperationSignalGuard::install();
    let restart_result = restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars: vec![
                ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                (
                    "MISTER_SCREENSAVER_START_PREVIEW_WHEN_READY".into(),
                    "1".into(),
                ),
                (
                    "MISTER_SCREENSAVER_RENDERER".into(),
                    "particle-magik".into(),
                ),
            ],
            timeout_secs: 45,
            remote_env: DEVELOPMENT_LAUNCHER_ENV.into(),
            ..LauncherRestartOptions::default()
        },
    );

    let run_result = restart_result.and_then(|()| {
        let embedded = wait_status_state(&session, "embedded", LAUNCHER_START_DEADLINE)?;
        let embedded_generation = status_generation(&embedded)?;
        publish_recipe(&session, recipe, REMOTE_MAGIK_RECIPE)?;
        let initial = wait_status_after(&session, embedded_generation, ACK_DEADLINE)?;
        require_status(&initial, "applied")?;
        let mut publisher = RecipePublisher::with_generation(
            recipe,
            REMOTE_MAGIK_RECIPE,
            status_generation(&initial)?,
        )?;
        while !attended_operation_interrupted() {
            thread::sleep(WATCH_INTERVAL);
            publisher.poll(&session)?;
        }
        Ok(publisher)
    });

    let cleanup_result = cleanup_dev_launcher(&session, run_result.as_ref().ok());
    match (run_result, cleanup_result) {
        (Ok(_), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(format!("Dev launcher cleanup failed: {error}").into()),
        (Err(run_error), Err(cleanup_error)) => {
            Err(format!("{run_error}; Dev launcher cleanup also failed: {cleanup_error}").into())
        }
    }
}

fn wait_status_state(session: &Session, expected: &str, timeout: Duration) -> Result<Value> {
    let started = Instant::now();
    loop {
        if let Some(text) = remote_read(session, REMOTE_STATUS)
            && let Ok(status) = serde_json::from_str::<Value>(text.trim())
            && status.get("state").and_then(Value::as_str) == Some(expected)
        {
            status_generation(&status)?;
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            return Err(format!(
                "startup particle status did not reach {expected:?} within {} ms",
                timeout.as_millis()
            )
            .into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn validate_local_input(path: &Path, label: &str) -> Result<()> {
    if !path.is_file() {
        return Err(format!("{label} is missing: {}", path.display()).into());
    }
    Ok(())
}

fn prepare_lab_files(
    session: &Session,
    binary: &Path,
    recipe: Option<&Path>,
    scene: &str,
    fixture: Option<&str>,
    screenshot: Option<&RemoteScreenshotArgs>,
) -> Result<()> {
    exec_checked(
        session,
        "startup particle lab preflight",
        &lab_preflight_command(),
    )?;
    put(session, binary, &format!("{REMOTE_BINARY}.upload"))?;
    let binary_hash = file_sha256(binary.to_path_buf())?;
    let recipe_hash = if let Some(recipe) = recipe {
        put(session, recipe, &format!("{REMOTE_LAB_RECIPE}.next"))?;
        Some(file_sha256(recipe.to_path_buf())?)
    } else {
        None
    };
    exec_checked(
        session,
        "publish startup particle lab",
        &remote_publish_lab_command(&binary_hash, recipe_hash.as_deref()),
    )?;
    exec_checked(
        session,
        "validate startup particle lab",
        &format!(
            "{} --check {}",
            sh(REMOTE_BINARY),
            remote_scene_arguments(scene, recipe.is_some(), fixture, screenshot)
        ),
    )
}

fn validate_installed_screenshot_archive(session: &Session) -> Result<()> {
    let reply = super::exec_checked_output(
        session,
        "validate installed Dev screenshot archive",
        &format!(
            "set -eu; test -f {path}; test -r {path}; bytes=$(wc -c < {path}); hash=$(sha256sum {path} | awk '{{print $1}}'); test \"$bytes\" -gt 0; printf 'bytes=%s sha256=%s\\n' \"$bytes\" \"$hash\"",
            path = sh(DEV_SCREENSHOT_ARCHIVE),
        ),
    )?;
    println!(
        "installed screenshot archive path={} {}",
        DEV_SCREENSHOT_ARCHIVE,
        reply.stdout.trim()
    );
    Ok(())
}

fn publish_recipe(session: &Session, recipe: &Path, remote_recipe: &str) -> Result<()> {
    let next = format!("{remote_recipe}.next");
    put(session, recipe, &next)?;
    let hash = file_sha256(recipe.to_path_buf())?;
    exec_checked(
        session,
        "publish startup particle recipe",
        &format!(
            "set -eu; test \"$(sha256sum {} | awk '{{print $1}}')\" = {}; mv -f {} {}",
            sh(&next),
            sh(&hash),
            sh(&next),
            sh(remote_recipe)
        ),
    )
}

struct RecipePublisher<'a> {
    local: &'a Path,
    remote: &'static str,
    hash: String,
    generation: u64,
    missing_polls: u8,
    remote_present: bool,
}

impl<'a> RecipePublisher<'a> {
    fn new(local: &'a Path, remote: &'static str) -> Result<Self> {
        Self::with_generation(local, remote, 0)
    }

    fn with_generation(local: &'a Path, remote: &'static str, generation: u64) -> Result<Self> {
        Ok(Self {
            local,
            remote,
            hash: file_sha256(local.to_path_buf())?,
            generation,
            missing_polls: 0,
            remote_present: true,
        })
    }

    fn poll(&mut self, session: &Session) -> Result<()> {
        let candidate = match file_sha256(self.local.to_path_buf()) {
            Ok(hash) => hash,
            Err(_) if !self.local.exists() => {
                self.missing_polls = self.missing_polls.saturating_add(1);
                if self.remote_present && self.missing_polls >= 2 {
                    exec_checked(
                        session,
                        "remove startup particle recipe",
                        &format!("rm -f {}", sh(self.remote)),
                    )?;
                    let status = wait_status_after(session, self.generation, ACK_DEADLINE)?;
                    require_status(&status, "embedded")?;
                    self.generation = status_generation(&status)?;
                    self.remote_present = false;
                }
                return Ok(());
            }
            Err(error) => {
                eprintln!("startup particle recipe read failed; retaining device data: {error}");
                return Ok(());
            }
        };
        self.missing_polls = 0;
        if self.remote_present && candidate == self.hash {
            return Ok(());
        }
        publish_recipe(session, self.local, self.remote)?;
        let status = wait_status_after(session, self.generation, ACK_DEADLINE)?;
        self.generation = status_generation(&status)?;
        self.hash = candidate;
        self.remote_present = true;
        report_status(&status);
        Ok(())
    }
}

fn wait_status_after(session: &Session, generation: u64, timeout: Duration) -> Result<Value> {
    let started = Instant::now();
    loop {
        if let Some(text) = remote_read(session, REMOTE_STATUS)
            && let Ok(status) = serde_json::from_str::<Value>(text.trim())
            && status_is_after(&status, generation, None)
        {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            return Err(format!(
                "startup particle status did not advance beyond generation {generation} within {} ms",
                timeout.as_millis()
            )
            .into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_status_state_after(
    session: &Session,
    generation: u64,
    expected: &str,
    timeout: Duration,
) -> Result<Value> {
    let started = Instant::now();
    loop {
        if let Some(text) = remote_read(session, REMOTE_STATUS)
            && let Ok(status) = serde_json::from_str::<Value>(text.trim())
            && status_is_after(&status, generation, Some(expected))
        {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            return Err(format!(
                "startup particle status did not reach {expected:?} after generation {generation} within {} ms",
                timeout.as_millis()
            )
            .into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn status_is_after(status: &Value, generation: u64, expected: Option<&str>) -> bool {
    status_generation(status).is_ok_and(|candidate| candidate > generation)
        && expected
            .is_none_or(|expected| status.get("state").and_then(Value::as_str) == Some(expected))
}

fn status_generation(status: &Value) -> Result<u64> {
    status
        .get("generation")
        .and_then(Value::as_u64)
        .ok_or_else(|| "startup particle status has no generation".into())
}

fn require_status(status: &Value, expected: &str) -> Result<()> {
    let state = status
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("missing");
    if state == expected {
        return Ok(());
    }
    let error = status
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("no error detail");
    Err(format!("startup particle recipe was {state}: {error}").into())
}

fn report_status(status: &Value) {
    let generation = status_generation(status).unwrap_or_default();
    let state = status
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if state == "rejected" {
        let error = status
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("no error detail");
        eprintln!("startup particle recipe rejected generation={generation}: {error}");
    } else {
        println!("startup particle recipe {state} generation={generation}");
    }
}

fn cleanup_dev_launcher(session: &Session, publisher: Option<&RecipePublisher<'_>>) -> Result<()> {
    let publisher_generation = publisher.map_or(0, |publisher| publisher.generation);
    let current_status = remote_read(session, REMOTE_STATUS)
        .and_then(|text| serde_json::from_str::<Value>(text.trim()).ok());
    let generation = current_status
        .as_ref()
        .and_then(|status| status_generation(status).ok())
        .unwrap_or_default()
        .max(publisher_generation);
    let already_embedded = current_status
        .as_ref()
        .is_some_and(|status| status.get("state").and_then(Value::as_str) == Some("embedded"));
    let removal = super::exec_checked_output(
        session,
        "remove Dev launcher startup particle recipe",
        &format!(
            "if test -e {recipe}; then rm -f {recipe}; printf 'removed=1\\n'; else printf 'removed=0\\n'; fi",
            recipe = sh(REMOTE_MAGIK_RECIPE),
        ),
    )
    .and_then(|reply| parse_recipe_removal(&reply.stdout));
    let acknowledgement_required = removal
        .as_ref()
        .is_ok_and(|removed| cleanup_requires_embedded_ack(*removed, already_embedded));
    let acknowledgement_result = if acknowledgement_required {
        wait_status_state_after(session, generation, "embedded", ACK_DEADLINE).map(|_| ())
    } else {
        Ok(())
    };
    let removal_result = removal.map(|_| ());
    let volatile_result = exec_checked(
        session,
        "clean startup particle Dev files",
        &format!(
            "set -eu; rm -f {} {}; rmdir {} 2>/dev/null || true",
            sh(REMOTE_STATUS),
            sh(&format!("{REMOTE_MAGIK_RECIPE}.next")),
            sh(REMOTE_DIR)
        ),
    );
    let safety_result = verify_safety_clear(session);
    combine_results(
        combine_results(removal_result, acknowledgement_result),
        combine_results(volatile_result, safety_result),
    )
}

fn parse_recipe_removal(output: &str) -> Result<bool> {
    match output.trim() {
        "removed=1" => Ok(true),
        "removed=0" => Ok(false),
        other => Err(format!("invalid startup particle removal reply {other:?}").into()),
    }
}

const fn cleanup_requires_embedded_ack(recipe_removed: bool, already_embedded: bool) -> bool {
    recipe_removed || !already_embedded
}

fn run_remote_lab(
    config: &super::remote::ConnectionConfig,
    request: RemoteLabRequest,
) -> Result<()> {
    let RemoteLabRequest {
        display_contracts,
        scene,
        has_recipe,
        fixture,
        screenshot,
        case,
        profile,
        assess,
        output_dir,
    } = request;
    let session = connect_with(config, 10)?;
    stream_exec(
        &session,
        &remote_run_lab_command(
            &display_contracts,
            &scene,
            has_recipe,
            fixture.as_deref(),
            screenshot.as_ref(),
            case.as_deref(),
            profile,
            assess,
        ),
    )?;
    if assess {
        let output_dir = output_dir
            .as_deref()
            .ok_or("card assessment output directory is missing")?;
        retrieve_card_assessment(&session, output_dir)?;
    }
    Ok(())
}

fn stream_exec(session: &Session, command: &str) -> Result<()> {
    let mut channel = session.channel_session()?;
    channel.handle_extended_data(ExtendedData::Merge)?;
    channel.exec(command)?;
    let mut buffer = [0_u8; 4096];
    loop {
        match channel.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                std::io::stdout().write_all(&buffer[..count])?;
                std::io::stdout().flush()?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    channel.wait_close()?;
    let status = channel.exit_status()?;
    if status == 0 || attended_operation_interrupted() {
        Ok(())
    } else {
        Err(format!("startup particle lab exited with status {status}").into())
    }
}

fn prepare_card_assessment_output(output_dir: &Path, binary: &Path) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    let receipt_path = PathBuf::from(format!("{}.build-receipt.tsv", binary.display()));
    let receipt = fs::read_to_string(&receipt_path).map_err(|error| {
        format!(
            "read card assessment build receipt {}: {error}",
            receipt_path.display()
        )
    })?;
    let source_commit = receipt
        .split_ascii_whitespace()
        .find_map(|field| field.strip_prefix("source_commit="))
        .filter(|value| !value.is_empty())
        .ok_or("card assessment build receipt has no source commit")?;
    let manifest = json!({
        "schema": "mister-magik-card-flip-assessment-manifest-v1",
        "git_sha": source_commit,
        "binary_sha256": file_sha256(binary.to_path_buf())?,
        "binary": binary.display().to_string(),
        "build_receipt": receipt.trim(),
    });
    fs::write(
        output_dir.join("manifest.json"),
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
    Ok(())
}

fn retrieve_card_assessment(session: &Session, output_dir: &Path) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    let cadence_dir = format!("{REMOTE_CARD_ASSESSMENT_DIR}/cadence");
    let profile_dir = format!("{REMOTE_CARD_ASSESSMENT_DIR}/profile");
    let cadence_frames = require_remote_artifact(session, &format!("{cadence_dir}/frames.jsonl"))?;
    let cadence_summary = require_remote_artifact(session, &format!("{cadence_dir}/summary.json"))?;
    let profile_frames = require_remote_artifact(session, &format!("{profile_dir}/frames.jsonl"))?;
    let profile_pass_summary =
        require_remote_artifact(session, &format!("{profile_dir}/summary.json"))?;
    let profile = require_remote_artifact(session, &format!("{profile_dir}/profile.json"))?;
    let flamegraph = require_remote_artifact(session, &format!("{profile_dir}/flamegraph.svg"))?;
    let folded = require_remote_artifact(session, &format!("{profile_dir}/stacks.folded"))?;

    validate_card_profile_artifacts(&profile, &flamegraph, &folded)?;
    let cadence_value: Value = serde_json::from_str(cadence_summary.trim())?;
    let profile_pass_value: Value = serde_json::from_str(profile_pass_summary.trim())?;
    let cadence_frame_values = parse_card_frames(&cadence_frames)?;
    let profile_frame_values = parse_card_frames(&profile_frames)?;
    let combined = summarize_card_assessment(
        cadence_value,
        profile_pass_value,
        &cadence_frame_values,
        &profile_frame_values,
    )?;
    bind_card_assessment_manifest(output_dir, &combined)?;
    let report = card_assessment_report(&combined);

    for (name, contents) in [
        ("cadence-frames.jsonl", cadence_frames.as_str()),
        ("cadence-summary.json", cadence_summary.as_str()),
        ("profile-frames.jsonl", profile_frames.as_str()),
        ("profile.json", profile.as_str()),
        ("flamegraph.svg", flamegraph.as_str()),
        ("stacks.folded", folded.as_str()),
    ] {
        fs::write(output_dir.join(name), contents)?;
    }
    fs::write(
        output_dir.join("summary.json"),
        format!("{}\n", serde_json::to_string_pretty(&combined)?),
    )?;
    fs::write(output_dir.join("report.md"), report)?;
    println!("card-flip assessment evidence: {}", output_dir.display());

    let failures = combined
        .get("qualification_failures")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if failures > 0 {
        return Err(format!(
            "card-flip cadence assessment found {failures} failure(s); evidence retained at {}",
            output_dir.display()
        )
        .into());
    }
    Ok(())
}

fn bind_card_assessment_manifest(output_dir: &Path, summary: &Value) -> Result<()> {
    let path = output_dir.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path)?)?;
    manifest["display_plan"] = summary["cadence_pass"]["display"].clone();
    manifest["card_geometry"] = summary["cadence_pass"]["card"].clone();
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
    Ok(())
}

fn require_remote_artifact(session: &Session, path: &str) -> Result<String> {
    remote_read(session, path)
        .filter(|contents| !contents.is_empty())
        .ok_or_else(|| format!("card assessment artifact is missing: {path}").into())
}

fn validate_card_profile_artifacts(profile: &str, flamegraph: &str, folded: &str) -> Result<()> {
    let metadata: Value = serde_json::from_str(profile.trim())?;
    if metadata.get("schema").and_then(Value::as_str) != Some("mister-magik-scene-lab-pprof-v1")
        || metadata.get("state").and_then(Value::as_str) != Some("complete")
        || metadata.get("scene").and_then(Value::as_str) != Some("card-flip")
        || metadata
            .get("sample_hits")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            <= 0
        || metadata
            .get("sample_stacks")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            == 0
    {
        return Err("card assessment profiler metadata is incomplete".into());
    }
    if !flamegraph.contains("<svg") || !flamegraph.contains("</svg>") {
        return Err("card assessment flamegraph is not a complete SVG".into());
    }
    if !folded.contains("card_flip") && !folded.contains("run_card_flip_mister") {
        return Err("card assessment folded stacks have no resolved card symbols".into());
    }
    Ok(())
}

fn parse_card_frames(text: &str) -> Result<Vec<Value>> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

fn summarize_card_assessment(
    cadence: Value,
    profile: Value,
    cadence_frames: &[Value],
    profile_frames: &[Value],
) -> Result<Value> {
    let cadence_physical = cadence
        .get("cadence")
        .ok_or("card cadence summary has no physical cadence")?;
    let profile_physical = profile
        .get("cadence")
        .ok_or("card profile summary has no physical cadence")?;
    if cadence_physical
        .get("cadence_authoritative")
        .and_then(Value::as_bool)
        != Some(true)
        || profile_physical
            .get("cadence_authoritative")
            .and_then(Value::as_bool)
            != Some(false)
    {
        return Err("card assessment pass authority is invalid".into());
    }
    let refresh_period_us = cadence_physical
        .get("refresh_period_us")
        .and_then(Value::as_u64)
        .filter(|period| *period > 0)
        .ok_or("card cadence refresh period is invalid")?;
    let cadence_repeats = value_u64(cadence_physical, "repeated_refreshes");
    let profile_repeats = value_u64(profile_physical, "repeated_refreshes");
    let mut failures = Vec::new();
    for (kind, count) in [
        ("repeated-refreshes", cadence_repeats),
        (
            "sequence-failures",
            value_u64(cadence_physical, "sequence_failures"),
        ),
        (
            "latch-drops",
            value_u64(cadence_physical, "latch_drop_delta"),
        ),
        (
            "completion-failures",
            value_u64(cadence_physical, "completion_failures"),
        ),
    ] {
        if count > 0 {
            failures.push(json!({"kind": kind, "count": count}));
        }
    }
    let attribution = if cadence_repeats == 0 && profile_repeats == 0 {
        "no physical repeated refreshes observed in either pass"
    } else if cadence_repeats == 0 {
        "repeats occurred only with SIGPROF enabled; sampling overhead is the likely cause"
    } else {
        "physical repeats occurred in the unprofiled control; inspect repeat contexts and CPU stacks"
    };
    Ok(json!({
        "schema": "mister-magik-card-flip-assessment-v1",
        "cadence_pass": cadence,
        "profile_pass": profile,
        "cadence_phase_timings": card_phase_summary(cadence_frames),
        "profile_phase_timings": card_phase_summary(profile_frames),
        "cadence_repeat_contexts": card_repeat_contexts(cadence_frames, refresh_period_us),
        "profile_repeat_contexts": card_repeat_contexts(profile_frames, refresh_period_us),
        "cadence_long_confirmation_gaps": card_long_confirmation_gaps(cadence_frames),
        "profile_long_confirmation_gaps": card_long_confirmation_gaps(profile_frames),
        "cadence_pre_post_outliers": card_pre_post_outliers(cadence_frames),
        "profile_pre_post_outliers": card_pre_post_outliers(profile_frames),
        "attribution": attribution,
        "qualification_failures": failures,
    }))
}

fn value_u64(value: &Value, field: &str) -> u64 {
    value.get(field).and_then(Value::as_u64).unwrap_or(0)
}

fn card_phase_summary(frames: &[Value]) -> Value {
    let fields = [
        "render_wall_us",
        "render_cpu_us",
        "transfer_wall_us",
        "transfer_cpu_us",
        "post_wall_us",
        "post_cpu_us",
        "settle_wall_us",
        "settle_cpu_us",
        "post_to_confirm_wall_us",
        "frame_to_confirm_wall_us",
        "process_cpu_us",
    ];
    let summaries = fields
        .iter()
        .map(|field| {
            let mut values = frames
                .iter()
                .map(|frame| value_u64(frame, field))
                .collect::<Vec<_>>();
            values.sort_unstable();
            let average = if values.is_empty() {
                0
            } else {
                values.iter().sum::<u64>() / values.len() as u64
            };
            let p99 = values
                .get(values.len().saturating_mul(99).div_ceil(100).saturating_sub(1))
                .copied()
                .unwrap_or(0);
            ((*field).to_string(), json!({"average_us": average, "p99_us": p99, "max_us": values.last().copied().unwrap_or(0)}))
        })
        .collect::<serde_json::Map<_, _>>();
    Value::Object(summaries)
}

fn card_repeat_contexts(frames: &[Value], refresh_period_us: u64) -> Vec<Value> {
    frames
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(index, frame)| {
            let interval = value_u64(frame, "completion_interval_us");
            let expected = interval
                .saturating_add(refresh_period_us / 2)
                .checked_div(refresh_period_us)
                .unwrap_or(1)
                .max(1);
            let flips = value_u64(frame, "flip_delta");
            (expected > flips).then(|| {
                let start = index.saturating_sub(2);
                let end = (index + 3).min(frames.len());
                json!({
                    "frame": value_u64(frame, "frame"),
                    "expected_refreshes": expected,
                    "flip_delta": flips,
                    "context": frames[start..end].to_vec(),
                })
            })
        })
        .collect()
}

fn card_pre_post_outliers(frames: &[Value]) -> Vec<Value> {
    let mut ranked = frames
        .iter()
        .map(|frame| {
            let pre_post_us = value_u64(frame, "render_wall_us")
                .saturating_add(value_u64(frame, "transfer_wall_us"))
                .saturating_add(value_u64(frame, "post_wall_us"));
            (pre_post_us, frame.clone())
        })
        .collect::<Vec<_>>();
    ranked.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.0));
    ranked
        .into_iter()
        .take(20)
        .map(|(pre_post_us, frame)| json!({"pre_post_wall_us": pre_post_us, "frame": frame}))
        .collect()
}

fn card_long_confirmation_gaps(frames: &[Value]) -> Vec<Value> {
    let mut ranked = frames
        .iter()
        .enumerate()
        .skip(1)
        .map(|(index, frame)| (value_u64(frame, "completion_interval_us"), index))
        .collect::<Vec<_>>();
    ranked.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.0));
    ranked
        .into_iter()
        .take(20)
        .map(|(completion_interval_us, index)| {
            let start = index.saturating_sub(2);
            let end = (index + 3).min(frames.len());
            json!({
                "frame": value_u64(&frames[index], "frame"),
                "completion_interval_us": completion_interval_us,
                "context": frames[start..end].to_vec(),
            })
        })
        .collect()
}

fn timing(summary: &Value, pass: &str, phase: &str, statistic: &str) -> u64 {
    summary[pass][phase][statistic].as_u64().unwrap_or(0)
}

fn card_assessment_report(summary: &Value) -> String {
    let cadence = &summary["cadence_pass"]["cadence"];
    let profile = &summary["profile_pass"]["cadence"];
    let cadence_cpu = summary["cadence_pass"]["process_cpu_pct_of_one_core"]
        .as_f64()
        .unwrap_or(0.0);
    let profile_cpu = summary["profile_pass"]["process_cpu_pct_of_one_core"]
        .as_f64()
        .unwrap_or(0.0);
    let worst_gap = summary["cadence_long_confirmation_gaps"]
        .as_array()
        .and_then(|gaps| gaps.first());
    let worst_gap_frame = worst_gap.map_or(0, |gap| value_u64(gap, "frame"));
    let worst_gap_us = worst_gap.map_or(0, |gap| value_u64(gap, "completion_interval_us"));
    let worst_pre_post = summary["cadence_pre_post_outliers"]
        .as_array()
        .and_then(|outliers| outliers.first());
    let worst_pre_post_us =
        worst_pre_post.map_or(0, |outlier| value_u64(outlier, "pre_post_wall_us"));
    let worst_pre_post_frame = worst_pre_post
        .and_then(|outlier| outlier.get("frame"))
        .map_or(0, |frame| value_u64(frame, "frame"));
    let worst_pre_post_face = worst_pre_post
        .and_then(|outlier| outlier.get("frame"))
        .and_then(|frame| frame.get("face"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let worst_pre_post_progress = worst_pre_post
        .and_then(|outlier| outlier.get("frame"))
        .map_or(0, |frame| value_u64(frame, "progress_q16"));
    format!(
        "# Card-flip cadence and CPU assessment\n\n## Physical cadence authority\n\n- Unprofiled physical FPS: {:.3}\n- Unprofiled repeated refreshes: {}\n- Unprofiled sequence failures: {}\n- Unprofiled latch drops: {}\n- Unprofiled completion failures: {}\n- Profiled repeated refreshes (attribution only): {}\n- Attribution: {}\n\nThe unprofiled confirmation stream is the cadence authority. The 99 Hz sampled pass cannot qualify cadence.\n\n## Full timing and CPU\n\n| Pass | Process CPU | Render avg / p99 | Transfer avg / p99 | Post avg / p99 | Settle avg / p99 | Post-to-confirm avg / p99 | Frame-to-confirm avg / p99 |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n| Unprofiled | {:.2}% | {:.3} / {:.3} ms | {:.3} / {:.3} ms | {:.3} / {:.3} ms | {:.3} / {:.3} ms | {:.3} / {:.3} ms | {:.3} / {:.3} ms |\n| 99 Hz sampled | {:.2}% | {:.3} / {:.3} ms | {:.3} / {:.3} ms | {:.3} / {:.3} ms | {:.3} / {:.3} ms | {:.3} / {:.3} ms | {:.3} / {:.3} ms |\n\nThe longest unprofiled confirmation interval was {:.3} ms at frame {}. The largest unprofiled pre-post workload was {:.3} ms at frame {} (face `{}`, progress {}/65535). Ranked contexts around both categories are retained in `summary.json`. Wall time materially above CPU time in settle/post-to-confirm is expected vblank waiting; renderer or transfer pressure instead appears as matching wall and CPU growth before the latch post.\n\n## Artifacts\n\n[Flamegraph](flamegraph.svg) · [Folded stacks](stacks.folded) · [Cadence frames](cadence-frames.jsonl) · [Profile frames](profile-frames.jsonl) · [Machine summary](summary.json)\n",
        cadence
            .get("unique_fps")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        value_u64(cadence, "repeated_refreshes"),
        value_u64(cadence, "sequence_failures"),
        value_u64(cadence, "latch_drop_delta"),
        value_u64(cadence, "completion_failures"),
        value_u64(profile, "repeated_refreshes"),
        summary
            .get("attribution")
            .and_then(Value::as_str)
            .unwrap_or("unavailable"),
        cadence_cpu,
        timing(
            summary,
            "cadence_phase_timings",
            "render_wall_us",
            "average_us"
        ) as f64
            / 1_000.0,
        timing(summary, "cadence_phase_timings", "render_wall_us", "p99_us") as f64 / 1_000.0,
        timing(
            summary,
            "cadence_phase_timings",
            "transfer_wall_us",
            "average_us"
        ) as f64
            / 1_000.0,
        timing(
            summary,
            "cadence_phase_timings",
            "transfer_wall_us",
            "p99_us"
        ) as f64
            / 1_000.0,
        timing(
            summary,
            "cadence_phase_timings",
            "post_wall_us",
            "average_us"
        ) as f64
            / 1_000.0,
        timing(summary, "cadence_phase_timings", "post_wall_us", "p99_us") as f64 / 1_000.0,
        timing(
            summary,
            "cadence_phase_timings",
            "settle_wall_us",
            "average_us"
        ) as f64
            / 1_000.0,
        timing(summary, "cadence_phase_timings", "settle_wall_us", "p99_us") as f64 / 1_000.0,
        timing(
            summary,
            "cadence_phase_timings",
            "post_to_confirm_wall_us",
            "average_us"
        ) as f64
            / 1_000.0,
        timing(
            summary,
            "cadence_phase_timings",
            "post_to_confirm_wall_us",
            "p99_us"
        ) as f64
            / 1_000.0,
        timing(
            summary,
            "cadence_phase_timings",
            "frame_to_confirm_wall_us",
            "average_us"
        ) as f64
            / 1_000.0,
        timing(
            summary,
            "cadence_phase_timings",
            "frame_to_confirm_wall_us",
            "p99_us"
        ) as f64
            / 1_000.0,
        profile_cpu,
        timing(
            summary,
            "profile_phase_timings",
            "render_wall_us",
            "average_us"
        ) as f64
            / 1_000.0,
        timing(summary, "profile_phase_timings", "render_wall_us", "p99_us") as f64 / 1_000.0,
        timing(
            summary,
            "profile_phase_timings",
            "transfer_wall_us",
            "average_us"
        ) as f64
            / 1_000.0,
        timing(
            summary,
            "profile_phase_timings",
            "transfer_wall_us",
            "p99_us"
        ) as f64
            / 1_000.0,
        timing(
            summary,
            "profile_phase_timings",
            "post_wall_us",
            "average_us"
        ) as f64
            / 1_000.0,
        timing(summary, "profile_phase_timings", "post_wall_us", "p99_us") as f64 / 1_000.0,
        timing(
            summary,
            "profile_phase_timings",
            "settle_wall_us",
            "average_us"
        ) as f64
            / 1_000.0,
        timing(summary, "profile_phase_timings", "settle_wall_us", "p99_us") as f64 / 1_000.0,
        timing(
            summary,
            "profile_phase_timings",
            "post_to_confirm_wall_us",
            "average_us"
        ) as f64
            / 1_000.0,
        timing(
            summary,
            "profile_phase_timings",
            "post_to_confirm_wall_us",
            "p99_us"
        ) as f64
            / 1_000.0,
        timing(
            summary,
            "profile_phase_timings",
            "frame_to_confirm_wall_us",
            "average_us"
        ) as f64
            / 1_000.0,
        timing(
            summary,
            "profile_phase_timings",
            "frame_to_confirm_wall_us",
            "p99_us"
        ) as f64
            / 1_000.0,
        worst_gap_us as f64 / 1_000.0,
        worst_gap_frame,
        worst_pre_post_us as f64 / 1_000.0,
        worst_pre_post_frame,
        worst_pre_post_face,
        worst_pre_post_progress,
    )
}

fn stop_remote_lab(session: &Session) -> Result<()> {
    exec_checked(
        session,
        "stop startup particle lab",
        "set -eu; pid=$(pidof mister-magik-framebuffer-scene-lab || true); test -z \"$pid\" || kill -TERM $pid",
    )
}

fn stop_remote_lab_connection(config: &super::remote::ConnectionConfig) -> Result<()> {
    let session = connect_with(config, 10)?;
    stop_remote_lab(&session)
}

fn lab_preflight_command() -> String {
    format!(
        "set -eu; test \"$(cat /sys/class/graphics/fb0/bits_per_pixel)\" = 16; ! pidof mister-magik-framebuffer-scene-lab >/dev/null 2>&1; {}; rm -rf {} {}; mkdir -p {}",
        safety_clear_checks(),
        sh(REMOTE_DIR),
        sh(REMOTE_CARD_ASSESSMENT_DIR),
        sh(REMOTE_DIR)
    )
}

fn dev_preflight_command() -> String {
    format!(
        "set -eu; set -- $(pidof MiSTer_MagiKDev); test \"$#\" -eq 1; main_pid=$1; test \"$(readlink /proc/$main_pid/exe)\" = /media/fat/MiSTer_MagiKDev; set -- $(pidof mister-magik-fb); test \"$#\" -eq 1; launcher_pid=$1; test \"$(readlink /proc/$launcher_pid/exe)\" = /media/fat/mister-magik-dev/mister-magik-fb; test \"$(cat /sys/class/graphics/fb0/bits_per_pixel)\" = 16; {}; rm -rf {}; mkdir -p {}",
        safety_clear_checks(),
        sh(REMOTE_DIR),
        sh(REMOTE_DIR)
    )
}

fn safety_clear_checks() -> &'static str {
    "for path in /media/fat/mister-magik/launcher.env /media/fat/mister-magik-dev/launcher.env /tmp/mister-magik/fs-fault-launcher.env /tmp/mister-magik/fs-fault-session /tmp/mister-magik/fs-fault.json /media/fat/mister-magik/rebuild-on-next-boot /media/fat/mister-magik-dev/rebuild-on-next-boot; do test ! -e \"$path\"; done"
}

fn verify_safety_clear(session: &Session) -> Result<()> {
    exec_checked(
        session,
        "verify startup particle safety cleanup",
        &format!(
            "set -eu; {}; test ! -e {}; test ! -e {}",
            safety_clear_checks(),
            sh(REMOTE_DIR),
            sh(REMOTE_CARD_ASSESSMENT_DIR),
        ),
    )
}

fn remove_volatile_directory(session: &Session) -> Result<()> {
    exec_checked(
        session,
        "clean startup particle volatile directory",
        &format!(
            "rm -rf {} {}",
            sh(REMOTE_DIR),
            sh(REMOTE_CARD_ASSESSMENT_DIR)
        ),
    )
}

fn remote_publish_lab_command(binary_hash: &str, recipe_hash: Option<&str>) -> String {
    let recipe_publish = recipe_hash.map_or_else(String::new, |recipe_hash| {
        format!(
            "; test \"$(sha256sum {} | awk '{{print $1}}')\" = {}; mv -f {} {}",
            sh(&format!("{REMOTE_LAB_RECIPE}.next")),
            sh(recipe_hash),
            sh(&format!("{REMOTE_LAB_RECIPE}.next")),
            sh(REMOTE_LAB_RECIPE)
        )
    });
    format!(
        "set -eu; test \"$(sha256sum {} | awk '{{print $1}}')\" = {}; chmod 755 {}; mv -f {} {}{}",
        sh(&format!("{REMOTE_BINARY}.upload")),
        sh(binary_hash),
        sh(&format!("{REMOTE_BINARY}.upload")),
        sh(&format!("{REMOTE_BINARY}.upload")),
        sh(REMOTE_BINARY),
        recipe_publish,
    )
}

fn remote_scene_arguments(
    scene: &str,
    has_recipe: bool,
    fixture: Option<&str>,
    screenshot: Option<&RemoteScreenshotArgs>,
) -> String {
    if has_recipe {
        format!("--scene {} --recipe {}", sh(scene), sh(REMOTE_LAB_RECIPE))
    } else if let Some(fixture) = fixture {
        format!("--scene {} --fixture {}", sh(scene), sh(fixture))
    } else if let Some(screenshot) = screenshot {
        format!(
            "--scene {} --archive {} --seed {} --sampling-profile {}",
            sh(scene),
            sh(screenshot.archive),
            screenshot.seed,
            sh(screenshot.sampling_profile)
        )
    } else {
        format!("--scene {}", sh(scene))
    }
}

#[allow(clippy::too_many_arguments)]
fn remote_run_lab_command(
    display_contracts: &LabDisplayContracts,
    scene: &str,
    has_recipe: bool,
    fixture: Option<&str>,
    screenshot: Option<&RemoteScreenshotArgs>,
    case: Option<&str>,
    profile: bool,
    assess: bool,
) -> String {
    let suspend = acknowledged_main_command("mister_magik_suspend");
    let resume = acknowledged_main_command("mister_magik_resume");
    let invocation = if assess {
        let cadence_dir = format!("{REMOTE_CARD_ASSESSMENT_DIR}/cadence");
        let profile_dir = format!("{REMOTE_CARD_ASSESSMENT_DIR}/profile");
        format!(
            "rm -rf {assessment}; mkdir -p {cadence} {profile}; {environment} {binary} --scene card-flip --assessment-pass cadence --evidence-dir {cadence}; MISTER_SCENE_LAB_PPROF_OUT={svg} MISTER_SCENE_LAB_PPROF_FOLDED_OUT={folded} MISTER_SCENE_LAB_PPROF_COMPLETE={complete} {environment} {binary} --scene card-flip --assessment-pass profile --evidence-dir {profile}",
            assessment = sh(REMOTE_CARD_ASSESSMENT_DIR),
            cadence = sh(&cadence_dir),
            profile = sh(&profile_dir),
            environment = format!(
                "MISTER_MAGIK_RUNTIME_SETTINGS_V1={} MISTER_MAGIK_RUNTIME_DISPLAY_V1={}",
                sh(&display_contracts.settings),
                sh(&display_contracts.display),
            ),
            binary = sh(REMOTE_BINARY),
            svg = sh(&format!("{profile_dir}/flamegraph.svg")),
            folded = sh(&format!("{profile_dir}/stacks.folded")),
            complete = sh(&format!("{profile_dir}/profile.json")),
        )
    } else {
        format!(
            "MISTER_MAGIK_RUNTIME_SETTINGS_V1={} MISTER_MAGIK_RUNTIME_DISPLAY_V1={} {} {} {} {}",
            sh(&display_contracts.settings),
            sh(&display_contracts.display),
            sh(REMOTE_BINARY),
            remote_scene_arguments(scene, has_recipe, fixture, screenshot),
            case.map_or_else(String::new, |case| format!("--case {}", sh(case))),
            if profile { "--profile" } else { "" },
        )
    };
    format!(
        "suspended=0; cleanup() {{ rc=$?; trap - EXIT HUP INT TERM; resume_rc=0; if test \"$suspended\" = 1; then {resume} || resume_rc=$?; fi; rm -rf {dir}; if test \"$rc\" -ne 0; then exit \"$rc\"; fi; exit \"$resume_rc\"; }}; trap cleanup EXIT HUP INT TERM; set -eu; {suspend}; suspended=1; {invocation}",
        dir = sh(REMOTE_DIR),
    )
}

fn local_recipe_scene(recipe: &Path) -> Result<&'static str> {
    let bytes = std::fs::read(recipe)?;
    let value: Value = serde_json::from_slice(&bytes)?;
    match value.get("schema").and_then(Value::as_str) {
        Some(MAGIK_SCHEMA) => Ok("magik"),
        Some(CABINET_SCHEMA) => Ok("cabinet"),
        _ => Err("scene lab accepts only MagiK V1 or cabinet V1 recipes".into()),
    }
}

fn combine_results(run: Result<()>, cleanup: Result<()>) -> Result<()> {
    match (run, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(run), Ok(())) => Err(run),
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Err(run), Err(cleanup)) => {
            Err(format!("{run}; launcher recovery also failed: {cleanup}").into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdmi_contracts() -> LabDisplayContracts {
        LabDisplayContracts {
            settings: "schema=1&output=hdmi".into(),
            display: "schema=1&mode=hdmi-1920x1080p60".into(),
        }
    }

    #[test]
    fn lab_is_volatile_and_restores_main() {
        let run = remote_run_lab_command(
            &hdmi_contracts(),
            "magik",
            true,
            None,
            None,
            None,
            false,
            false,
        );
        assert!(run.contains(REMOTE_DIR));
        assert!(run.contains("mister_magik_suspend"));
        assert!(run.contains("mister_magik_resume"));
        assert!(!run.contains("/media/fat/mister-magik/mister-magik-fb"));
        assert!(run.contains("--recipe"));
        assert!(run.contains("MISTER_MAGIK_RUNTIME_SETTINGS_V1="));
        assert!(run.contains("MISTER_MAGIK_RUNTIME_DISPLAY_V1="));
        assert!(!run.contains("--destination-width"));
        assert!(!run.contains("--destination-height"));
    }

    #[test]
    fn navigation_fixture_lab_is_volatile_and_recipe_free() {
        let run = remote_run_lab_command(
            &hdmi_contracts(),
            "navigation-transition",
            false,
            Some("home-arcade"),
            None,
            None,
            false,
            false,
        );
        assert!(run.contains(&format!(
            "--scene {} --fixture {}",
            sh("navigation-transition"),
            sh("home-arcade")
        )));
        assert!(!run.contains("--recipe"));
        assert!(run.contains("mister_magik_suspend"));
        assert!(run.contains("mister_magik_resume"));
    }

    #[test]
    fn card_flip_lab_is_self_contained() {
        let run = remote_run_lab_command(
            &hdmi_contracts(),
            "card-flip",
            false,
            None,
            None,
            None,
            false,
            false,
        );
        assert!(run.contains(&format!("--scene {}", sh("card-flip"))));
        assert!(!run.contains("--recipe"));
        assert!(!run.contains("--fixture"));
        assert!(run.contains("mister_magik_suspend"));
        assert!(run.contains("mister_magik_resume"));
    }

    #[test]
    fn screenshot_lab_uses_installed_pack_seed_and_route_profile() {
        let screenshot = RemoteScreenshotArgs {
            archive: DEV_SCREENSHOT_ARCHIVE,
            seed: 0x1234,
            sampling_profile: "crt",
        };
        let run = remote_run_lab_command(
            &hdmi_contracts(),
            "screenshot-screensaver",
            false,
            None,
            Some(&screenshot),
            None,
            false,
            false,
        );
        assert!(run.contains(&format!("--archive {}", sh(DEV_SCREENSHOT_ARCHIVE))));
        assert!(run.contains("--seed 4660"));
        assert!(run.contains(&format!("--sampling-profile {}", sh("crt"))));
        assert!(!run.contains("--recipe"));
        assert!(run.contains("mister_magik_suspend"));
        assert!(run.contains("mister_magik_resume"));
    }

    #[test]
    fn card_flip_assessment_runs_two_passes_with_fixed_artifacts() {
        let run = remote_run_lab_command(
            &hdmi_contracts(),
            "card-flip",
            false,
            None,
            None,
            None,
            false,
            true,
        );
        assert!(run.contains("--assessment-pass cadence"));
        assert!(run.contains("--assessment-pass profile"));
        assert!(run.contains("MISTER_SCENE_LAB_PPROF_OUT="));
        assert_eq!(run.matches(REMOTE_BINARY).count(), 2);
        assert!(run.contains(REMOTE_CARD_ASSESSMENT_DIR));
    }

    #[test]
    fn card_profile_artifacts_require_samples_svg_and_symbols() {
        let metadata = serde_json::json!({
            "schema": "mister-magik-scene-lab-pprof-v1",
            "state": "complete",
            "scene": "card-flip",
            "sample_hits": 10,
            "sample_stacks": 2,
        })
        .to_string();
        assert!(
            validate_card_profile_artifacts(
                &metadata,
                "<svg><g></g></svg>",
                "thread;run_card_flip_mister 10\n"
            )
            .is_ok()
        );
        assert!(validate_card_profile_artifacts(&metadata, "not-svg", "card_flip 1").is_err());
        assert!(validate_card_profile_artifacts(&metadata, "<svg></svg>", "unresolved 1").is_err());
    }

    #[test]
    fn sampled_card_pass_cannot_be_cadence_authority() {
        let cadence = serde_json::json!({
            "cadence": {
                "cadence_authoritative": true,
                "refresh_period_us": 16_667,
                "repeated_refreshes": 0,
                "sequence_failures": 0,
                "latch_drop_delta": 0,
                "completion_failures": 0,
                "unique_fps": 60.0,
            }
        });
        let sampled = serde_json::json!({
            "cadence": {
                "cadence_authoritative": true,
                "refresh_period_us": 16_667,
                "repeated_refreshes": 0,
            }
        });
        assert!(summarize_card_assessment(cadence, sampled, &[], &[]).is_err());
    }

    #[test]
    fn long_confirmation_gaps_are_ranked_with_surrounding_frames() {
        let frames = [
            serde_json::json!({"frame": 0, "completion_interval_us": 0}),
            serde_json::json!({"frame": 1, "completion_interval_us": 16_667}),
            serde_json::json!({"frame": 2, "completion_interval_us": 33_334}),
            serde_json::json!({"frame": 3, "completion_interval_us": 16_668}),
        ];
        let gaps = card_long_confirmation_gaps(&frames);
        assert_eq!(gaps[0]["frame"], 2);
        assert_eq!(gaps[0]["completion_interval_us"], 33_334);
        assert_eq!(gaps[0]["context"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn assessment_manifest_binds_display_and_card_geometry() {
        let unique = format!(
            "mister-magik-card-manifest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let directory = std::env::temp_dir().join(unique);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("manifest.json"), "{\"git_sha\":\"abc\"}\n").unwrap();
        let summary = serde_json::json!({
            "cadence_pass": {
                "display": {"render_w": 960, "render_h": 600},
                "card": {"width": 287, "height": 420},
            }
        });
        bind_card_assessment_manifest(&directory, &summary).unwrap();
        let manifest: Value =
            serde_json::from_slice(&fs::read(directory.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["git_sha"], "abc");
        assert_eq!(manifest["display_plan"]["render_h"], 600);
        assert_eq!(manifest["card_geometry"]["height"], 420);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn volatile_cleanup_includes_card_assessment_artifacts() {
        assert!(lab_preflight_command().contains(REMOTE_CARD_ASSESSMENT_DIR));
        assert!(
            format!(
                "rm -rf {} {}",
                sh(REMOTE_DIR),
                sh(REMOTE_CARD_ASSESSMENT_DIR)
            )
            .contains(REMOTE_CARD_ASSESSMENT_DIR)
        );
    }

    #[test]
    fn dev_launcher_requires_the_exact_development_runtime() {
        let preflight = dev_preflight_command();
        assert!(preflight.contains("pidof MiSTer_MagiKDev"));
        assert!(preflight.contains("/media/fat/MiSTer_MagiKDev"));
        assert!(!preflight.contains("pidof MiSTer_MagiK "));
        assert!(!preflight.contains("/media/fat/mister-magik/mister-magik-fb"));
    }

    #[test]
    fn every_persistent_arming_file_is_rejected() {
        let checks = safety_clear_checks();
        for path in [
            "/media/fat/mister-magik/launcher.env",
            "/media/fat/mister-magik-dev/launcher.env",
            "/tmp/mister-magik/fs-fault-launcher.env",
            "/tmp/mister-magik/fs-fault-session",
            "/tmp/mister-magik/fs-fault.json",
            "/media/fat/mister-magik/rebuild-on-next-boot",
            "/media/fat/mister-magik-dev/rebuild-on-next-boot",
        ] {
            assert!(checks.contains(path));
        }
    }

    #[test]
    fn recipes_are_published_atomically() {
        let command = remote_publish_lab_command("binary", Some("recipe"));
        assert!(command.contains("recipe.json.next"));
        assert!(command.contains("mv -f"));
        assert!(command.contains("sha256sum"));
    }

    #[test]
    fn embedded_cleanup_acknowledgement_rejects_stale_and_wrong_states() {
        let stale_embedded = serde_json::json!({"generation": 4, "state": "embedded"});
        let newer_rejected = serde_json::json!({"generation": 6, "state": "rejected"});
        let newer_embedded = serde_json::json!({"generation": 6, "state": "embedded"});

        assert!(!status_is_after(&stale_embedded, 4, Some("embedded")));
        assert!(!status_is_after(&newer_rejected, 4, Some("embedded")));
        assert!(status_is_after(&newer_embedded, 4, Some("embedded")));
    }

    #[test]
    fn crt_contracts_are_forwarded_without_lab_geometry() {
        let contracts = LabDisplayContracts {
            settings: "schema=1&output=crt-240p60".into(),
            display: "schema=1&mode=auto".into(),
        };
        let run = remote_run_lab_command(
            &contracts,
            "card-flip",
            false,
            None,
            None,
            None,
            false,
            false,
        );
        assert!(run.contains("schema=1&output=crt-240p60"));
        assert!(run.contains("schema=1&mode=auto"));
        assert!(!run.contains("960"));
        assert!(!run.contains("540"));
    }

    #[test]
    fn recipe_removal_reply_is_closed() {
        assert!(parse_recipe_removal("removed=1\n").unwrap());
        assert!(!parse_recipe_removal("removed=0\n").unwrap());
        assert!(parse_recipe_removal("maybe").is_err());
    }

    #[test]
    fn cleanup_waits_until_an_absent_recipe_is_confirmed_embedded() {
        assert!(cleanup_requires_embedded_ack(true, false));
        assert!(cleanup_requires_embedded_ack(true, true));
        assert!(cleanup_requires_embedded_ack(false, false));
        assert!(!cleanup_requires_embedded_ack(false, true));
    }
}
