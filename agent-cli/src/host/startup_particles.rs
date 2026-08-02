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
use ssh2::{ExtendedData, Session};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const REMOTE_DIR: &str = "/tmp/mister-magik/startup-particles";
const REMOTE_BINARY: &str = "/tmp/mister-magik/startup-particles/mister-magik-startup-particle-lab";
const REMOTE_LAB_RECIPE: &str = "/tmp/mister-magik/startup-particles/recipe.json";
const REMOTE_MAGIK_RECIPE: &str = "/tmp/mister-magik/startup-particles/magik.json";
const REMOTE_STATUS: &str = "/tmp/mister-magik/startup-particles/status.json";
const DEVELOPMENT_LAUNCHER_ENV: &str = "/media/fat/mister-magik-dev/launcher.env";
const WATCH_INTERVAL: Duration = Duration::from_millis(100);
const ACK_DEADLINE: Duration = Duration::from_secs(1);

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
        StartupParticleRuntime::Lab => run_lab(
            &prepared,
            binary.ok_or_else(|| {
                DeviceFailure::InvalidRequest("lab runtime requires a built lab binary".into())
            })?,
            recipe,
        ),
        StartupParticleRuntime::DevLauncher => run_dev_launcher(&prepared, recipe),
    }
    .map_err(device_failure)
}

fn run_lab(prepared: &super::PreparedDevice, binary: &Path, recipe: &Path) -> Result<()> {
    let session = connect_with(&prepared.config.connection, 10)?;
    if let Err(error) = prepare_lab_files(&session, binary, recipe) {
        let cleanup = remove_volatile_directory(&session);
        return combine_results(Err(error), cleanup);
    }
    let _signal_guard = AttendedOperationSignalGuard::install();
    let run_config = prepared.config.connection.clone();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("startup-particle-lab-device".into())
        .spawn(move || {
            let result = run_remote_lab(&run_config).map_err(|error| error.to_string());
            let _ = finished_tx.send(result);
        })?;

    let mut publisher = RecipePublisher::new(recipe, REMOTE_LAB_RECIPE)?;
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
        if let Err(error) = publisher.poll(&session) {
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

fn run_dev_launcher(prepared: &super::PreparedDevice, recipe: &Path) -> Result<()> {
    let session = connect_with(&prepared.config.connection, 10)?;
    exec_checked(
        &session,
        "startup particle Dev launcher preflight",
        &dev_preflight_command(),
    )?;
    if let Err(error) = publish_recipe(&session, recipe, REMOTE_MAGIK_RECIPE) {
        let cleanup = remove_volatile_directory(&session);
        return combine_results(Err(error), cleanup);
    }
    let _signal_guard = AttendedOperationSignalGuard::install();
    let restart_result = restart_launcher_with_one_shot_env(
        &session,
        LauncherRestartOptions {
            env_vars: vec![
                ("MISTER_CATALOG_REFRESH".into(), "off".into()),
                (
                    "MISTER_SCREENSAVER_START_IDLE_WHEN_READY".into(),
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
        let initial = wait_status_after(&session, 0, ACK_DEADLINE)?;
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

fn validate_local_input(path: &Path, label: &str) -> Result<()> {
    if !path.is_file() {
        return Err(format!("{label} is missing: {}", path.display()).into());
    }
    Ok(())
}

fn prepare_lab_files(session: &Session, binary: &Path, recipe: &Path) -> Result<()> {
    exec_checked(
        session,
        "startup particle lab preflight",
        &lab_preflight_command(),
    )?;
    put(session, binary, &format!("{REMOTE_BINARY}.upload"))?;
    put(session, recipe, &format!("{REMOTE_LAB_RECIPE}.next"))?;
    let binary_hash = file_sha256(binary.to_path_buf())?;
    let recipe_hash = file_sha256(recipe.to_path_buf())?;
    exec_checked(
        session,
        "publish startup particle lab",
        &remote_publish_lab_command(&binary_hash, &recipe_hash),
    )?;
    exec_checked(
        session,
        "validate startup particle lab",
        &format!(
            "{} --check --recipe {}",
            sh(REMOTE_BINARY),
            sh(REMOTE_LAB_RECIPE)
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
            && status_generation(&status).is_ok_and(|candidate| candidate > generation)
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
    let generation = publisher.map_or(0, |publisher| publisher.generation);
    let already_embedded = remote_read(session, REMOTE_STATUS)
        .and_then(|text| serde_json::from_str::<Value>(text.trim()).ok())
        .is_some_and(|status| {
            status.get("state").and_then(Value::as_str) == Some("embedded")
                && status_generation(&status).is_ok_and(|candidate| candidate >= generation)
        });
    let removal_result = exec_checked(
        session,
        "remove Dev launcher startup particle recipe",
        &format!("rm -f {}", sh(REMOTE_MAGIK_RECIPE)),
    );
    let acknowledgement_result = if removal_result.is_ok() && !already_embedded {
        wait_status_after(session, generation, ACK_DEADLINE)
            .and_then(|status| require_status(&status, "embedded"))
    } else {
        Ok(())
    };
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

fn run_remote_lab(config: &super::remote::ConnectionConfig) -> Result<()> {
    let session = connect_with(config, 10)?;
    stream_exec(&session, &remote_run_lab_command())
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
        "set -eu; pid=$(pidof mister-magik-startup-particle-lab || true); test -z \"$pid\" || kill -TERM $pid",
    )
}

fn stop_remote_lab_connection(config: &super::remote::ConnectionConfig) -> Result<()> {
    let session = connect_with(config, 10)?;
    stop_remote_lab(&session)
}

fn lab_preflight_command() -> String {
    format!(
        "set -eu; test \"$(cat /sys/class/graphics/fb0/bits_per_pixel)\" = 16; ! pidof mister-magik-startup-particle-lab >/dev/null 2>&1; {}; rm -rf {}; mkdir -p {}",
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

fn remote_publish_lab_command(binary_hash: &str, recipe_hash: &str) -> String {
    format!(
        "set -eu; test \"$(sha256sum {} | awk '{{print $1}}')\" = {}; test \"$(sha256sum {} | awk '{{print $1}}')\" = {}; chmod 755 {}; mv -f {} {}; mv -f {} {}",
        sh(&format!("{REMOTE_BINARY}.upload")),
        sh(binary_hash),
        sh(&format!("{REMOTE_LAB_RECIPE}.next")),
        sh(recipe_hash),
        sh(&format!("{REMOTE_BINARY}.upload")),
        sh(&format!("{REMOTE_BINARY}.upload")),
        sh(REMOTE_BINARY),
        sh(&format!("{REMOTE_LAB_RECIPE}.next")),
        sh(REMOTE_LAB_RECIPE)
    )
}

fn remote_run_lab_command() -> String {
    let suspend = acknowledged_main_command("mister_magik_suspend");
    let resume = acknowledged_main_command("mister_magik_resume");
    format!(
        "suspended=0; cleanup() {{ rc=$?; trap - EXIT HUP INT TERM; resume_rc=0; if test \"$suspended\" = 1; then {resume} || resume_rc=$?; fi; rm -rf {dir}; if test \"$rc\" -ne 0; then exit \"$rc\"; fi; exit \"$resume_rc\"; }}; trap cleanup EXIT HUP INT TERM; set -eu; {suspend}; suspended=1; {binary} --recipe {recipe}",
        dir = sh(REMOTE_DIR),
        binary = sh(REMOTE_BINARY),
        recipe = sh(REMOTE_LAB_RECIPE),
    )
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
        let run = remote_run_lab_command();
        assert!(run.contains(REMOTE_DIR));
        assert!(run.contains("mister_magik_suspend"));
        assert!(run.contains("mister_magik_resume"));
        assert!(!run.contains("/media/fat/mister-magik/mister-magik-fb"));
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
        let command = remote_publish_lab_command("binary", "recipe");
        assert!(command.contains("recipe.json.next"));
        assert!(command.contains("mv -f"));
        assert!(command.contains("sha256sum"));
    }
}
