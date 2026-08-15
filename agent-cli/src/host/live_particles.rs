// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Volatile, attended particle-lab sessions.
//!
//! The lab and its editable family live only below `/tmp`. Main remains the
//! supervisor and the installed launcher/runtime bundle is never replaced.

use super::remote::{connect_with, put, shell_quote as sh};
use super::{
    DeviceAccess, DeviceFailure, NativeDevice, Result, acknowledged_main_command, device_failure,
    exec_checked, file_sha256, install_prepared_device_environment, platform_safety_script,
    wait_launcher_ready,
};
use ssh2::{ExtendedData, Session};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const REMOTE_DIR: &str = "/tmp/mister-magik/live-particles";
const REMOTE_BINARY: &str = "/tmp/mister-magik/live-particles/mister-magik-particle-lab";
const REMOTE_FAMILY: &str = "/tmp/mister-magik/live-particles/family.json";
const REMOTE_FAMILY_NEXT: &str = "/tmp/mister-magik/live-particles/family.json.next";
const WATCH_INTERVAL: Duration = Duration::from_millis(100);

pub(super) fn run(
    device: &mut NativeDevice,
    binary: &Path,
    family: &Path,
    demo: &str,
) -> std::result::Result<(), DeviceFailure> {
    validate_local_input(binary, "particle lab binary").map_err(device_failure)?;
    validate_local_input(family, "particle family").map_err(device_failure)?;
    if demo.trim().is_empty()
        || demo
            .chars()
            .any(|character| matches!(character, '\n' | '\r'))
    {
        return Err(DeviceFailure::InvalidRequest(
            "particle demo must be a non-empty single-line identifier".into(),
        ));
    }

    let prepared = device.prepare(DeviceAccess::SSH_MUTATION)?;
    run_prepared(&prepared, binary, family, demo).map_err(device_failure)
}

fn run_prepared(
    prepared: &super::PreparedDevice,
    binary: &Path,
    family: &Path,
    demo: &str,
) -> Result<()> {
    install_prepared_device_environment(&prepared.config);
    let upload = connect_with(&prepared.config.connection, 10)?;
    let display_contracts = super::startup_particles::active_lab_display_contracts(&upload)?;
    prepare_remote_files(&upload, binary, family, demo)?;

    let run_config = prepared.config.connection.clone();
    let run_demo = demo.to_owned();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("particle-lab-device".into())
        .spawn(move || {
            let result = run_remote_lab(&run_config, &run_demo, &display_contracts)
                .map_err(|error| error.to_string());
            let _ = finished_tx.send(result);
        })?;

    let mut uploaded_hash = file_sha256(family.to_path_buf())?;
    let run_result = loop {
        match finished_rx.recv_timeout(WATCH_INTERVAL) {
            Ok(result) => break result.map_err(Into::into),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err("particle lab device worker disconnected".into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let candidate_hash = match file_sha256(family.to_path_buf()) {
            Ok(hash) => hash,
            Err(error) => {
                eprintln!("particle family read failed; retaining device data: {error}");
                continue;
            }
        };
        if candidate_hash == uploaded_hash {
            continue;
        }
        match publish_family(&upload, family) {
            Ok(()) => {
                uploaded_hash = candidate_hash;
                println!("particle family published sha256={uploaded_hash}");
            }
            Err(error) => {
                eprintln!("particle family upload failed; will retry: {error}");
            }
        }
    };
    worker
        .join()
        .map_err(|_| "particle lab device worker panicked")?;

    let launcher_result = wait_launcher_ready(&upload, Instant::now(), Duration::from_secs(45));
    combine_run_and_launcher(run_result, launcher_result.map(|_| ()))
}

fn validate_local_input(path: &Path, label: &str) -> Result<()> {
    if !path.is_file() {
        return Err(format!("{label} is missing: {}", path.display()).into());
    }
    Ok(())
}

fn prepare_remote_files(session: &Session, binary: &Path, family: &Path, demo: &str) -> Result<()> {
    exec_checked(
        session,
        "particle lab preflight",
        &remote_preflight_command(),
    )?;
    put(session, binary, &format!("{REMOTE_BINARY}.upload"))?;
    put(session, family, REMOTE_FAMILY_NEXT)?;
    let binary_hash = file_sha256(binary.to_path_buf())?;
    let family_hash = file_sha256(family.to_path_buf())?;
    exec_checked(
        session,
        "publish particle lab",
        &remote_publish_command(&binary_hash, &family_hash),
    )?;
    exec_checked(
        session,
        "validate particle lab",
        &format!(
            "{} --check --demo {} --family {}",
            sh(REMOTE_BINARY),
            sh(demo),
            sh(REMOTE_FAMILY)
        ),
    )
}

fn publish_family(session: &Session, family: &Path) -> Result<()> {
    put(session, family, REMOTE_FAMILY_NEXT)?;
    let hash = file_sha256(family.to_path_buf())?;
    exec_checked(
        session,
        "publish particle family",
        &format!(
            "set -eu; test \"$(sha256sum {} | awk '{{print $1}}')\" = {}; mv -f {} {}",
            sh(REMOTE_FAMILY_NEXT),
            sh(&hash),
            sh(REMOTE_FAMILY_NEXT),
            sh(REMOTE_FAMILY)
        ),
    )
}

fn run_remote_lab(
    config: &super::remote::ConnectionConfig,
    demo: &str,
    display_contracts: &super::startup_particles::LabDisplayContracts,
) -> Result<()> {
    let session = connect_with(config, 10)?;
    let command = remote_run_command(demo, display_contracts);
    stream_exec(&session, &command)
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
    if status == 0 {
        Ok(())
    } else {
        Err(format!("particle lab exited with status {status}").into())
    }
}

fn remote_preflight_command() -> String {
    format!(
        "set -eu; test \"$(cat /sys/class/graphics/fb0/bits_per_pixel)\" = 16; ! pidof mister-magik-particle-lab >/dev/null 2>&1; {}; rm -rf {}; mkdir -p {}",
        platform_safety_script(),
        sh(REMOTE_DIR),
        sh(REMOTE_DIR)
    )
}

fn remote_publish_command(binary_hash: &str, family_hash: &str) -> String {
    format!(
        "set -eu; test \"$(sha256sum {} | awk '{{print $1}}')\" = {}; test \"$(sha256sum {} | awk '{{print $1}}')\" = {}; chmod 755 {}; mv -f {} {}; mv -f {} {}",
        sh(&format!("{REMOTE_BINARY}.upload")),
        sh(binary_hash),
        sh(REMOTE_FAMILY_NEXT),
        sh(family_hash),
        sh(&format!("{REMOTE_BINARY}.upload")),
        sh(&format!("{REMOTE_BINARY}.upload")),
        sh(REMOTE_BINARY),
        sh(REMOTE_FAMILY_NEXT),
        sh(REMOTE_FAMILY)
    )
}

fn remote_run_command(
    demo: &str,
    display_contracts: &super::startup_particles::LabDisplayContracts,
) -> String {
    let suspend = acknowledged_main_command("mister_magik_suspend");
    let resume = acknowledged_main_command("mister_magik_resume");
    format!(
        "cleanup() {{ rc=$?; trap - EXIT HUP INT TERM; resume_rc=0; {resume} || resume_rc=$?; rm -rf {dir}; if test \"$rc\" -ne 0; then exit \"$rc\"; fi; exit \"$resume_rc\"; }}; trap cleanup EXIT HUP INT TERM; set -eu; {suspend}; MISTER_MAGIK_RUNTIME_SETTINGS_V1={runtime_settings} MISTER_MAGIK_RUNTIME_DISPLAY_V1={runtime_display} {binary} --demo {demo} --family {family}",
        dir = sh(REMOTE_DIR),
        binary = sh(REMOTE_BINARY),
        demo = sh(demo),
        family = sh(REMOTE_FAMILY),
        runtime_settings = sh(&display_contracts.settings),
        runtime_display = sh(&display_contracts.display),
    )
}

fn combine_run_and_launcher(run: Result<()>, launcher: Result<()>) -> Result<()> {
    match (run, launcher) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(run), Ok(())) => Err(run),
        (Ok(()), Err(launcher)) => Err(launcher),
        (Err(run), Err(launcher)) => {
            Err(format!("{run}; launcher recovery also failed: {launcher}").into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volatile_paths_never_touch_installed_runtime() {
        let publish = remote_publish_command("binary", "family");
        let contracts = super::super::startup_particles::LabDisplayContracts {
            settings: "schema=1&output=hdmi".into(),
            display: "schema=1&mode=hdmi-1920x1200p60".into(),
        };
        let run = remote_run_command("solar-chrysanthemum", &contracts);
        assert!(publish.contains(REMOTE_DIR));
        assert!(run.contains(REMOTE_DIR));
        assert!(!publish.contains("platform-v3.manifest"));
        assert!(!run.contains("launcher.env"));
        assert!(run.contains("mister_magik_suspend"));
        assert!(run.contains("mister_magik_resume"));
        assert!(run.contains("MISTER_MAGIK_RUNTIME_SETTINGS_V1="));
        assert!(run.contains("MISTER_MAGIK_RUNTIME_DISPLAY_V1="));
        assert!(!run.contains("--destination"));
    }

    #[test]
    fn preflight_rejects_every_persistent_fault_arming_file() {
        let command = remote_preflight_command();
        for path in [
            "/media/fat/mister-magik/launcher.env",
            "/media/fat/mister-magik-dev/launcher.env",
            "/tmp/mister-magik/fs-fault-launcher.env",
            "/tmp/mister-magik/fs-fault-session",
            "/tmp/mister-magik/fs-fault.json",
            "/media/fat/mister-magik/rebuild-on-next-boot",
            "/media/fat/mister-magik-dev/rebuild-on-next-boot",
        ] {
            assert!(command.contains(path));
        }
    }

    #[test]
    fn family_publish_is_atomic() {
        let command = remote_publish_command("binary", "family");
        assert!(command.contains("family.json.next"));
        assert!(command.contains("mv -f"));
        assert!(command.contains("sha256sum"));
    }
}
