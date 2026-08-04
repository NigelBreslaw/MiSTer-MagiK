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
use crate::commands::device::{SceneLabScene, StartupParticleRuntime};
use serde_json::Value;
use ssh2::{ExtendedData, Session};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const REMOTE_DIR: &str = "/tmp/mister-magik/startup-particles";
const REMOTE_BINARY: &str =
    "/tmp/mister-magik/startup-particles/mister-magik-framebuffer-scene-lab";
const REMOTE_LAB_RECIPE: &str = "/tmp/mister-magik/startup-particles/recipe.json";
const REMOTE_MAGIK_RECIPE: &str = "/tmp/mister-magik/startup-particles/magik.json";
const REMOTE_STATUS: &str = "/tmp/mister-magik/startup-particles/status.json";
const DEVELOPMENT_LAUNCHER_ENV: &str = "/media/fat/mister-magik-dev/launcher.env";
const WATCH_INTERVAL: Duration = Duration::from_millis(100);
const ACK_DEADLINE: Duration = Duration::from_secs(1);
const LAUNCHER_START_DEADLINE: Duration = Duration::from_secs(45);
const MAGIK_SCHEMA: &str = "mister-magik-particle-magik-v1";
const CABINET_SCHEMA: &str = "mister-magik-particle-cabinet-v1";

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
                binary.ok_or_else(|| {
                    DeviceFailure::InvalidRequest("lab runtime requires a built lab binary".into())
                })?,
                Some(recipe),
                scene,
                None,
                None,
                false,
            )
        }
        StartupParticleRuntime::DevLauncher => run_dev_launcher(&prepared, recipe),
    }
    .map_err(device_failure)
}

pub(super) fn run_scene_lab(
    device: &mut NativeDevice,
    binary: &Path,
    scene: SceneLabScene,
    recipe: Option<&Path>,
    fixture: Option<&str>,
    case: Option<&str>,
    profile: bool,
) -> std::result::Result<(), DeviceFailure> {
    validate_local_input(binary, "framebuffer scene lab binary").map_err(device_failure)?;
    if let Some(recipe) = recipe {
        validate_local_input(recipe, "framebuffer scene recipe").map_err(device_failure)?;
    }
    let prepared = device.prepare(DeviceAccess::SSH_MUTATION)?;
    install_prepared_device_environment(&prepared.config);
    run_lab(
        &prepared,
        binary,
        recipe,
        scene.as_str(),
        fixture,
        case,
        profile,
    )
    .map_err(device_failure)
}

fn run_lab(
    prepared: &super::PreparedDevice,
    binary: &Path,
    recipe: Option<&Path>,
    scene: &str,
    fixture: Option<&str>,
    case: Option<&str>,
    profile: bool,
) -> Result<()> {
    let has_recipe = recipe.is_some();
    let session = connect_with(&prepared.config.connection, 10)?;
    let destination = active_lab_destination(&session)?;
    if let Err(error) = prepare_lab_files(&session, binary, recipe, scene, fixture) {
        let cleanup = remove_volatile_directory(&session);
        return combine_results(Err(error), cleanup);
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
    let scene = scene.to_owned();
    let fixture = fixture.map(str::to_owned);
    let case = case.map(str::to_owned);
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let worker = match thread::Builder::new()
        .name("framebuffer-scene-lab-device".into())
        .spawn(move || {
            let result = run_remote_lab(
                &run_config,
                destination,
                &scene,
                has_recipe,
                fixture.as_deref(),
                case.as_deref(),
                profile,
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
    let launcher_result = wait_launcher_ready(&session, Instant::now(), Duration::from_secs(45));
    let safety_result = launcher_result.and_then(|_| verify_safety_clear(&session));
    combine_results(run_result, safety_result)
}

fn active_lab_destination(session: &Session) -> Result<(u16, u16)> {
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
    lab_destination_for_mode(&active)
}

fn lab_destination_for_mode(active: &str) -> Result<(u16, u16)> {
    if active.starts_with("crt-") {
        return Err("startup particle lab currently requires a fixed HDMI display mode".into());
    }
    super::DISPLAY_MATRIX_MODES
        .iter()
        .find(|mode| mode.id == active)
        .and_then(|mode| mode.output)
        .ok_or_else(|| {
            format!("startup particle lab requires a known fixed display mode, found {active}")
                .into()
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
            remote_scene_arguments(scene, recipe.is_some(), fixture)
        ),
    )
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
    destination: (u16, u16),
    scene: &str,
    has_recipe: bool,
    fixture: Option<&str>,
    case: Option<&str>,
    profile: bool,
) -> Result<()> {
    let session = connect_with(config, 10)?;
    stream_exec(
        &session,
        &remote_run_lab_command(destination, scene, has_recipe, fixture, case, profile),
    )
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
        "set -eu; test \"$(cat /sys/class/graphics/fb0/bits_per_pixel)\" = 16; ! pidof mister-magik-framebuffer-scene-lab >/dev/null 2>&1; {}; rm -rf {}; mkdir -p {}",
        safety_clear_checks(),
        sh(REMOTE_DIR),
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
            "set -eu; {}; test ! -e {}",
            safety_clear_checks(),
            sh(REMOTE_DIR)
        ),
    )
}

fn remove_volatile_directory(session: &Session) -> Result<()> {
    exec_checked(
        session,
        "clean startup particle volatile directory",
        &format!("rm -rf {}", sh(REMOTE_DIR)),
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

fn remote_scene_arguments(scene: &str, has_recipe: bool, fixture: Option<&str>) -> String {
    if has_recipe {
        format!("--scene {} --recipe {}", sh(scene), sh(REMOTE_LAB_RECIPE))
    } else if let Some(fixture) = fixture {
        format!("--scene {} --fixture {}", sh(scene), sh(fixture))
    } else {
        format!("--scene {}", sh(scene))
    }
}

fn remote_run_lab_command(
    destination: (u16, u16),
    scene: &str,
    has_recipe: bool,
    fixture: Option<&str>,
    case: Option<&str>,
    profile: bool,
) -> String {
    let suspend = acknowledged_main_command("mister_magik_suspend");
    let resume = acknowledged_main_command("mister_magik_resume");
    format!(
        "suspended=0; cleanup() {{ rc=$?; trap - EXIT HUP INT TERM; resume_rc=0; if test \"$suspended\" = 1; then {resume} || resume_rc=$?; fi; rm -rf {dir}; if test \"$rc\" -ne 0; then exit \"$rc\"; fi; exit \"$resume_rc\"; }}; trap cleanup EXIT HUP INT TERM; set -eu; {suspend}; suspended=1; {binary} {scene_arguments} {case_argument} {profile_argument} --destination-width {destination_width} --destination-height {destination_height}",
        dir = sh(REMOTE_DIR),
        binary = sh(REMOTE_BINARY),
        scene_arguments = remote_scene_arguments(scene, has_recipe, fixture),
        case_argument = case.map_or_else(String::new, |case| format!("--case {}", sh(case))),
        profile_argument = if profile { "--profile" } else { "" },
        destination_width = destination.0,
        destination_height = destination.1,
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

    #[test]
    fn lab_is_volatile_and_restores_main() {
        let run = remote_run_lab_command((1920, 1080), "magik", true, None, None, false);
        assert!(run.contains(REMOTE_DIR));
        assert!(run.contains("mister_magik_suspend"));
        assert!(run.contains("mister_magik_resume"));
        assert!(!run.contains("/media/fat/mister-magik/mister-magik-fb"));
        assert!(run.contains("--recipe"));
        assert!(run.contains("--destination-width 1920 --destination-height 1080"));
    }

    #[test]
    fn navigation_fixture_lab_is_volatile_and_recipe_free() {
        let run = remote_run_lab_command(
            (1920, 1080),
            "navigation-transition",
            false,
            Some("home-arcade"),
            None,
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
        let run = remote_run_lab_command((1920, 1080), "card-flip", false, None, None, false);
        assert!(run.contains(&format!("--scene {}", sh("card-flip"))));
        assert!(!run.contains("--recipe"));
        assert!(!run.contains("--fixture"));
        assert!(run.contains("mister_magik_suspend"));
        assert!(run.contains("mister_magik_resume"));
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
    fn focused_lab_accepts_fixed_hdmi_and_rejects_crt_routes() {
        assert_eq!(
            lab_destination_for_mode("hdmi-1920x1080p60").unwrap(),
            (1920, 1080)
        );
        assert!(lab_destination_for_mode("crt-240p60").is_err());
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
