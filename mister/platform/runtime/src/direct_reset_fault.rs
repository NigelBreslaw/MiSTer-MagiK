// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runtime-owned attended direct-reset fault control.

use crate::main_command::{self, MainCommand};
use mister_magik_catalog::fs_fault::{
    DirectResetFaultControl, DirectResetFaultOutcome, DirectResetFaultRequest, FaultConfig,
};
use serde_json::json;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MARKER_PATH: &str = "/tmp/mister-magik/fs-fault.json";
const SESSION_PATH: &str = "/tmp/mister-magik/fs-fault-session";

pub const DIRECT_RESET_CLEANUP_ARTIFACTS: [&str; 7] = [
    "/media/fat/mister-magik/launcher.env",
    "/media/fat/mister-magik-dev/launcher.env",
    "/tmp/mister-magik/fs-fault-launcher.env",
    SESSION_PATH,
    MARKER_PATH,
    "/media/fat/mister-magik/rebuild-on-next-boot",
    "/media/fat/mister-magik-dev/rebuild-on-next-boot",
];

static PROCESS_FAULT_CONFIG: OnceLock<Option<FaultConfig>> = OnceLock::new();

pub fn install_process_fault_config(config: Option<FaultConfig>) -> Result<(), &'static str> {
    PROCESS_FAULT_CONFIG
        .set(config)
        .map_err(|_| "process fault configuration was already installed")
}

pub fn process_fault_control() -> SystemDirectResetFaultControl {
    SystemDirectResetFaultControl::new(PROCESS_FAULT_CONFIG.get().cloned().flatten())
}

#[derive(Clone)]
pub struct SystemDirectResetFaultControl {
    config: Option<FaultConfig>,
}

impl SystemDirectResetFaultControl {
    pub fn new(config: Option<FaultConfig>) -> Self {
        Self { config }
    }

    fn request_with(
        &self,
        request: &DirectResetFaultRequest,
        runtime: &mut impl DirectResetRuntime,
    ) -> DirectResetFaultOutcome {
        let Some(config) = self.config.as_ref() else {
            return DirectResetFaultOutcome::Noop;
        };
        if config.point() != request.point() {
            return DirectResetFaultOutcome::PointMismatch;
        }
        if config.is_direct_reset_no_sync() && !session_is_armed(config, runtime) {
            crate::ui_errln!(
                "fs_fault: direct-reset-no-sync ignored at {}; volatile session not armed",
                request.point()
            );
            return DirectResetFaultOutcome::NotArmed;
        }

        let _ = runtime.write_marker(&marker_json(request, config));
        let outcome = if config.is_direct_reset_no_sync() {
            let _ = runtime.request_direct_reset();
            DirectResetFaultOutcome::ResetRequested
        } else {
            crate::ui_errln!(
                "fs_fault: unsupported action {} at point {}",
                config.action(),
                request.point()
            );
            DirectResetFaultOutcome::UnsupportedAction
        };
        runtime.sleep_ms(config.delay_ms());
        outcome
    }
}

impl DirectResetFaultControl for SystemDirectResetFaultControl {
    fn request_direct_reset(
        &mut self,
        request: &DirectResetFaultRequest,
    ) -> DirectResetFaultOutcome {
        self.request_with(request, &mut SystemDirectResetRuntime)
    }
}

pub fn cleanup_arming_artifacts() -> std::io::Result<()> {
    cleanup_arming_artifacts_with(&mut SystemDirectResetRuntime)
}

fn cleanup_arming_artifacts_with(runtime: &mut impl DirectResetRuntime) -> std::io::Result<()> {
    let mut first_error = None;
    for path in DIRECT_RESET_CLEANUP_ARTIFACTS {
        if let Err(error) = runtime.remove_file(path)
            && error.kind() != std::io::ErrorKind::NotFound
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn session_is_armed(config: &FaultConfig, runtime: &mut impl DirectResetRuntime) -> bool {
    if runtime.persistent_launcher_env_exists() {
        return false;
    }
    let Some(expected) = config.session_token() else {
        return false;
    };
    runtime
        .read_session()
        .map(|actual| actual.trim() == expected)
        .unwrap_or(false)
}

fn marker_json(request: &DirectResetFaultRequest, config: &FaultConfig) -> String {
    let value = json!({
        "schema": "mister-magik-fs-fault-v1",
        "point": request.point(),
        "target": request.target(),
        "action": config.action(),
        "delay_ms": config.delay_ms(),
        "session": config.session_token(),
        "pid": std::process::id(),
        "ts_unix_ms": unix_ms_now(),
    });
    format!("{value}\n")
}

fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

trait DirectResetRuntime {
    fn persistent_launcher_env_exists(&mut self) -> bool;
    fn read_session(&mut self) -> std::io::Result<String>;
    fn write_marker(&mut self, json: &str) -> std::io::Result<()>;
    fn request_direct_reset(&mut self) -> std::io::Result<()>;
    fn sleep_ms(&mut self, delay_ms: u64);
    fn remove_file(&mut self, path: &str) -> std::io::Result<()>;
}

struct SystemDirectResetRuntime;

impl DirectResetRuntime for SystemDirectResetRuntime {
    fn persistent_launcher_env_exists(&mut self) -> bool {
        DIRECT_RESET_CLEANUP_ARTIFACTS[..2]
            .iter()
            .any(|path| Path::new(path).exists())
    }

    fn read_session(&mut self) -> std::io::Result<String> {
        fs::read_to_string(SESSION_PATH)
    }

    fn write_marker(&mut self, json: &str) -> std::io::Result<()> {
        if let Some(parent) = Path::new(MARKER_PATH).parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(MARKER_PATH, json)
    }

    fn request_direct_reset(&mut self) -> std::io::Result<()> {
        main_command::execute(&MainCommand::DirectResetNoSync)
            .map(|_| ())
            .map_err(std::io::Error::other)
    }

    fn sleep_ms(&mut self, delay_ms: u64) {
        std::thread::sleep(Duration::from_millis(delay_ms));
    }

    fn remove_file(&mut self, path: &str) -> std::io::Result<()> {
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeRuntime {
        session: Option<String>,
        events: Vec<String>,
        fail_remove: Option<String>,
        persistent_launcher_env: bool,
    }

    impl DirectResetRuntime for FakeRuntime {
        fn persistent_launcher_env_exists(&mut self) -> bool {
            self.events.push("check-persistent-env".to_string());
            self.persistent_launcher_env
        }

        fn read_session(&mut self) -> std::io::Result<String> {
            self.events.push("read-session".to_string());
            self.session
                .clone()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "missing"))
        }

        fn write_marker(&mut self, json: &str) -> std::io::Result<()> {
            self.events.push(format!("write-marker:{json}"));
            Ok(())
        }

        fn request_direct_reset(&mut self) -> std::io::Result<()> {
            self.events.push("request-reset".to_string());
            Ok(())
        }

        fn sleep_ms(&mut self, delay_ms: u64) {
            self.events.push(format!("sleep:{delay_ms}"));
        }

        fn remove_file(&mut self, path: &str) -> std::io::Result<()> {
            self.events.push(format!("remove:{path}"));
            if self.fail_remove.as_deref() == Some(path) {
                Err(std::io::Error::other("scripted cleanup failure"))
            } else {
                Ok(())
            }
        }
    }

    fn config(point: &str, session: Option<&str>) -> FaultConfig {
        FaultConfig::from_compatible_values(Some(point), None, Some("42"), session)
            .expect("fault config")
    }

    #[test]
    fn armed_reset_preserves_session_marker_command_and_delay_order() {
        let control = SystemDirectResetFaultControl::new(Some(config("point", Some("token"))));
        let request = DirectResetFaultRequest::new("point", Path::new("/target"));
        let mut runtime = FakeRuntime {
            session: Some("token".to_string()),
            ..FakeRuntime::default()
        };

        assert_eq!(
            control.request_with(&request, &mut runtime),
            DirectResetFaultOutcome::ResetRequested
        );
        assert_eq!(runtime.events[0], "check-persistent-env");
        assert_eq!(runtime.events[1], "read-session");
        assert!(runtime.events[2].starts_with("write-marker:"));
        assert!(runtime.events[2].contains("\"target\":\"/target\""));
        assert_eq!(runtime.events[3..], ["request-reset", "sleep:42"]);
    }

    #[test]
    fn unarmed_and_mismatched_requests_are_effect_free() {
        let control = SystemDirectResetFaultControl::new(Some(config("point", Some("token"))));
        let mut runtime = FakeRuntime::default();
        assert_eq!(
            control.request_with(
                &DirectResetFaultRequest::new("other", Path::new("/target")),
                &mut runtime,
            ),
            DirectResetFaultOutcome::PointMismatch
        );
        assert!(runtime.events.is_empty());
        assert_eq!(
            control.request_with(
                &DirectResetFaultRequest::new("point", Path::new("/target")),
                &mut runtime,
            ),
            DirectResetFaultOutcome::NotArmed
        );
        assert_eq!(runtime.events, ["check-persistent-env", "read-session"]);
    }

    #[test]
    fn persistent_launcher_environment_cannot_arm_a_reset() {
        let control = SystemDirectResetFaultControl::new(Some(config("point", Some("token"))));
        let mut runtime = FakeRuntime {
            session: Some("token".to_string()),
            persistent_launcher_env: true,
            ..FakeRuntime::default()
        };

        assert_eq!(
            control.request_with(
                &DirectResetFaultRequest::new("point", Path::new("/target")),
                &mut runtime,
            ),
            DirectResetFaultOutcome::NotArmed
        );
        assert_eq!(runtime.events, ["check-persistent-env"]);
    }

    #[test]
    fn cleanup_attempts_all_seven_artifacts_in_stable_order() {
        let mut runtime = FakeRuntime::default();
        cleanup_arming_artifacts_with(&mut runtime).expect("cleanup artifacts");
        assert_eq!(
            runtime.events,
            DIRECT_RESET_CLEANUP_ARTIFACTS
                .iter()
                .map(|path| format!("remove:{path}"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn cleanup_continues_after_an_artifact_error() {
        let mut runtime = FakeRuntime {
            fail_remove: Some(DIRECT_RESET_CLEANUP_ARTIFACTS[2].to_string()),
            ..FakeRuntime::default()
        };

        assert!(cleanup_arming_artifacts_with(&mut runtime).is_err());
        assert_eq!(runtime.events.len(), DIRECT_RESET_CLEANUP_ARTIFACTS.len());
        assert_eq!(
            runtime.events.last().map(String::as_str),
            Some("remove:/media/fat/mister-magik-dev/rebuild-on-next-boot")
        );
    }
}
