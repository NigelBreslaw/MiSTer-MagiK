// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Serialized transport for commands mediated by MiSTer Main.
//!
//! Navigation policy belongs to callers. This module alone owns the production
//! app's FIFO paths, wire spelling, operation lock, and reply association.

use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const COMMAND_FIFO: &str = "/dev/MiSTer_cmd";
const COMMAND_REPLY_FIFO: &str = "/dev/MiSTer_cmd_reply";
const COMMAND_OPERATION_LOCK: &str = "/tmp/mister-magik/command-operation.lock";
const MAIN_STATUS_PATH: &str = "/tmp/mister-magik/main-status.json";
const MISTER_PROCESS_NAMES: &[&str] = &["MiSTer_MagiKDev", "MiSTer_MagiK", "MiSTer"];
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MainCommand {
    SupervisedLauncherRestart,
    DisplayState,
    DisplayApply { mode: String },
    DisplayConfirm,
    DisplayCancel,
    ExitToMenu,
    Reboot,
    LaunchPath { target: String },
    StructuredLaunch { fields: String },
    LoadCore { target: String },
}

impl MainCommand {
    fn wire(&self) -> String {
        match self {
            Self::SupervisedLauncherRestart => {
                "mister_magik_supervised_restart_launcher\n".to_string()
            }
            Self::DisplayState => "mister_magik_display_get_v1\n".to_string(),
            Self::DisplayApply { mode } => {
                format!("mister_magik_display_apply_v1 mode={mode}\n")
            }
            Self::DisplayConfirm => "mister_magik_display_confirm_v1\n".to_string(),
            Self::DisplayCancel => "mister_magik_display_cancel_v1\n".to_string(),
            Self::ExitToMenu => "mister_magik_exit_to_menu\n".to_string(),
            Self::Reboot => "mister_magik_reboot\n".to_string(),
            Self::LaunchPath { target } => format!("mister_magik_launch {target}\n"),
            Self::StructuredLaunch { fields } => {
                format!("mister_magik_launch_plan_v1 {fields}\n")
            }
            Self::LoadCore { target } => format!("load_core {target}\n"),
        }
    }

    fn expects_reply(&self) -> bool {
        !matches!(
            self,
            Self::SupervisedLauncherRestart | Self::LoadCore { .. }
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MainCommandError(String);

impl MainCommandError {
    fn new(detail: impl Into<String>) -> Self {
        Self(detail.into())
    }
}

impl fmt::Display for MainCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for MainCommandError {}

trait MainCommandEndpoint {
    fn execute(
        &mut self,
        wire: &str,
        expects_reply: bool,
        lock_nonblocking: bool,
    ) -> Result<Option<String>, MainCommandError>;
}

struct MainCommandTransport<E> {
    endpoint: E,
}

impl<E> MainCommandTransport<E> {
    fn new(endpoint: E) -> Self {
        Self { endpoint }
    }

    #[cfg(test)]
    fn into_endpoint(self) -> E {
        self.endpoint
    }
}

impl<E: MainCommandEndpoint> MainCommandTransport<E> {
    fn execute(&mut self, command: &MainCommand) -> Result<Option<String>, MainCommandError> {
        self.execute_with_lock(command, false)
    }

    fn try_execute(&mut self, command: &MainCommand) -> Result<Option<String>, MainCommandError> {
        self.execute_with_lock(command, true)
    }

    fn execute_with_lock(
        &mut self,
        command: &MainCommand,
        lock_nonblocking: bool,
    ) -> Result<Option<String>, MainCommandError> {
        self.endpoint
            .execute(&command.wire(), command.expects_reply(), lock_nonblocking)
    }
}

#[derive(Default)]
struct SystemMainCommandEndpoint;

impl MainCommandEndpoint for SystemMainCommandEndpoint {
    fn execute(
        &mut self,
        wire: &str,
        expects_reply: bool,
        lock_nonblocking: bool,
    ) -> Result<Option<String>, MainCommandError> {
        if expects_reply {
            request_response(wire, lock_nonblocking).map(Some)
        } else {
            write_nonblocking(wire).map(|()| None)
        }
    }
}

pub fn execute(command: &MainCommand) -> Result<Option<String>, MainCommandError> {
    MainCommandTransport::new(SystemMainCommandEndpoint).execute(command)
}

pub fn try_execute(command: &MainCommand) -> Result<Option<String>, MainCommandError> {
    MainCommandTransport::new(SystemMainCommandEndpoint).try_execute(command)
}

pub fn wait_for_command_fifo(timeout: Duration) -> Result<(), MainCommandError> {
    wait_until(timeout, || Path::new(COMMAND_FIFO).exists())
        .then_some(())
        .ok_or_else(|| MainCommandError::new(format!("timed out waiting for {COMMAND_FIFO}")))
}

pub fn wait_for_running_main_and_fifo(
    main_name: &str,
    timeout: Duration,
) -> Result<(), MainCommandError> {
    wait_until(timeout, || {
        Path::new(COMMAND_FIFO).exists() && main_running()
    })
    .then_some(())
    .ok_or_else(|| {
        MainCommandError::new(format!(
            "timed out waiting for {main_name} + {COMMAND_FIFO}"
        ))
    })
}

fn main_running() -> bool {
    process_running(MISTER_PROCESS_NAMES)
}

fn process_running(names: &[&str]) -> bool {
    names.iter().any(|name| {
        Command::new("pidof")
            .arg(name)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    })
}

fn wait_until(timeout: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ready() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    ready()
}

fn write_nonblocking(wire: &str) -> Result<(), MainCommandError> {
    let start = Instant::now();
    let mut last_error = None;
    while start.elapsed() < WRITE_TIMEOUT {
        match fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(COMMAND_FIFO)
        {
            Ok(mut fifo) => {
                let bytes = wire.as_bytes();
                let mut written = 0usize;
                while written < bytes.len() && start.elapsed() < WRITE_TIMEOUT {
                    match fifo.write(&bytes[written..]) {
                        Ok(0) => {
                            last_error = Some("zero-length FIFO write".to_string());
                            break;
                        }
                        Ok(count) => written += count,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            last_error = Some(error.to_string());
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => {
                            return Err(MainCommandError::new(format!(
                                "failed to write {COMMAND_FIFO}: {error}"
                            )));
                        }
                    }
                }
                if written == bytes.len() {
                    return Ok(());
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.raw_os_error() == Some(libc::ENXIO) =>
            {
                last_error = Some(error.to_string());
            }
            Err(error) => {
                return Err(MainCommandError::new(format!(
                    "failed to open {COMMAND_FIFO}: {error}"
                )));
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(MainCommandError::new(format!(
        "timed out writing {COMMAND_FIFO}: {}",
        last_error.unwrap_or_else(|| "no reader".to_string())
    )))
}

fn request_response(wire: &str, lock_nonblocking: bool) -> Result<String, MainCommandError> {
    let command_lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(COMMAND_OPERATION_LOCK)
        .map_err(|error| MainCommandError::new(format!("failed to open command lock: {error}")))?;
    let lock_operation = libc::LOCK_EX | if lock_nonblocking { libc::LOCK_NB } else { 0 };
    if unsafe { libc::flock(command_lock.as_raw_fd(), lock_operation) } != 0 {
        return Err(MainCommandError::new(format!(
            "failed to lock command channel: {}",
            std::io::Error::last_os_error()
        )));
    }
    let mut reply = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(COMMAND_REPLY_FIFO)
        .map_err(|error| {
            MainCommandError::new(format!("failed to open {COMMAND_REPLY_FIFO}: {error}"))
        })?;
    let mut discard = [0u8; 256];
    while reply.read(&mut discard).is_ok_and(|count| count > 0) {}

    write_nonblocking(wire)?;

    let mut bytes = Vec::with_capacity(128);
    let mut heartbeat = main_heartbeat().unwrap_or(0);
    let mut heartbeat_seen = Instant::now();
    loop {
        let mut chunk = [0u8; 128];
        match reply.read(&mut chunk) {
            Ok(0) => return Err(MainCommandError::new("MiSTer command channel closed")),
            Ok(count) => {
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
                    let response = String::from_utf8_lossy(&bytes[..end]);
                    return parse_reply_line(&response);
                }
                if bytes.len() > 512 {
                    return Err(MainCommandError::new("MiSTer command reply too long"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => {
                return Err(MainCommandError::new(format!(
                    "failed to read {COMMAND_REPLY_FIFO}: {error}"
                )));
            }
        }
        if !main_running() {
            return Err(MainCommandError::new("MiSTer command channel closed"));
        }
        let current_heartbeat = main_heartbeat().unwrap_or(heartbeat);
        if current_heartbeat != heartbeat {
            heartbeat = current_heartbeat;
            heartbeat_seen = Instant::now();
        } else if heartbeat_seen.elapsed() >= HEARTBEAT_TIMEOUT {
            return Err(MainCommandError::new("MiSTer Main heartbeat stopped"));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn parse_reply_line(response: &str) -> Result<String, MainCommandError> {
    if response == "ok" || response.starts_with("ok ") {
        Ok(response.to_string())
    } else {
        Err(MainCommandError::new(response))
    }
}

fn main_heartbeat() -> Option<u64> {
    let text = fs::read_to_string(MAIN_STATUS_PATH).ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("ts_boot_ms")
        .and_then(serde_json::Value::as_u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeEndpoint {
        calls: Vec<(String, bool, bool)>,
        response: Option<String>,
    }

    impl MainCommandEndpoint for FakeEndpoint {
        fn execute(
            &mut self,
            wire: &str,
            expects_reply: bool,
            lock_nonblocking: bool,
        ) -> Result<Option<String>, MainCommandError> {
            self.calls
                .push((wire.to_string(), expects_reply, lock_nonblocking));
            Ok(self.response.clone())
        }
    }

    #[test]
    fn typed_commands_preserve_current_wire_spelling() {
        let endpoint = FakeEndpoint {
            response: Some("ok DisplayV1".to_string()),
            ..FakeEndpoint::default()
        };
        let mut transport = MainCommandTransport::new(endpoint);

        assert_eq!(
            transport.execute(&MainCommand::DisplayState).unwrap(),
            Some("ok DisplayV1".to_string())
        );
        transport
            .execute(&MainCommand::LaunchPath {
                target: "/media/fat/_Arcade/Test.mra".to_string(),
            })
            .unwrap();
        transport
            .execute(&MainCommand::LoadCore {
                target: "/media/fat/_Arcade/Test.mra".to_string(),
            })
            .unwrap();
        transport
            .execute(&MainCommand::SupervisedLauncherRestart)
            .unwrap();
        transport
            .execute(&MainCommand::DisplayApply {
                mode: "crt-240p60".to_string(),
            })
            .unwrap();
        transport.execute(&MainCommand::DisplayConfirm).unwrap();
        transport.execute(&MainCommand::DisplayCancel).unwrap();
        transport.execute(&MainCommand::ExitToMenu).unwrap();
        transport.execute(&MainCommand::Reboot).unwrap();
        transport
            .execute(&MainCommand::StructuredLaunch {
                fields: "schema=1&launch_ref=test".to_string(),
            })
            .unwrap();

        assert_eq!(
            transport.into_endpoint().calls,
            [
                ("mister_magik_display_get_v1\n".to_string(), true, false),
                (
                    "mister_magik_launch /media/fat/_Arcade/Test.mra\n".to_string(),
                    true,
                    false,
                ),
                (
                    "load_core /media/fat/_Arcade/Test.mra\n".to_string(),
                    false,
                    false,
                ),
                (
                    "mister_magik_supervised_restart_launcher\n".to_string(),
                    false,
                    false,
                ),
                (
                    "mister_magik_display_apply_v1 mode=crt-240p60\n".to_string(),
                    true,
                    false,
                ),
                ("mister_magik_display_confirm_v1\n".to_string(), true, false),
                ("mister_magik_display_cancel_v1\n".to_string(), true, false),
                ("mister_magik_exit_to_menu\n".to_string(), true, false),
                ("mister_magik_reboot\n".to_string(), true, false),
                (
                    "mister_magik_launch_plan_v1 schema=1&launch_ref=test\n".to_string(),
                    true,
                    false,
                ),
            ]
        );
    }

    #[test]
    fn try_execute_requests_a_nonblocking_operation_lock() {
        let mut transport = MainCommandTransport::new(FakeEndpoint::default());
        transport.try_execute(&MainCommand::DisplayState).unwrap();
        assert_eq!(
            transport.into_endpoint().calls,
            [("mister_magik_display_get_v1\n".to_string(), true, true)]
        );
    }

    #[test]
    fn reply_parser_accepts_ok_and_preserves_failures() {
        assert_eq!(
            parse_reply_line("ok LauncherSuspended").unwrap(),
            "ok LauncherSuspended"
        );
        assert_eq!(
            parse_reply_line("rejected LauncherCrashed")
                .unwrap_err()
                .to_string(),
            "rejected LauncherCrashed"
        );
        assert_eq!(
            parse_reply_line("malformed response")
                .unwrap_err()
                .to_string(),
            "malformed response"
        );
    }
}
