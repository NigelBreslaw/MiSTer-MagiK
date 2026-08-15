// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Immutable process-boundary configuration capture.

use mister_magik_catalog::fs_fault::FaultConfig;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

const STARTUP_TOKEN: &str = "MISTER_MAGIK_STARTUP_TOKEN";
const READY_FIFO: &str = "MISTER_MAGIK_READY_FIFO";
const MAIN_PID: &str = "MISTER_MAGIK_MAIN_PID";
const MAIN_GENERATION: &str = "MISTER_MAGIK_MAIN_GENERATION";
const OWNER_EPOCH: &str = "MISTER_MAGIK_OWNER_EPOCH";

#[derive(Clone, Default)]
pub struct EnvironmentSnapshot {
    values: BTreeMap<OsString, OsString>,
}

impl EnvironmentSnapshot {
    pub fn capture_process() -> Self {
        Self {
            values: std::env::vars_os().collect(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .get(OsStr::new(name))
            .and_then(|value| value.to_str())
    }

    pub fn get_path(&self, name: &str) -> Option<&Path> {
        self.values.get(OsStr::new(name)).map(Path::new)
    }

    #[cfg(test)]
    fn from_values(values: impl IntoIterator<Item = (&'static str, &'static str)>) -> Self {
        Self {
            values: values
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandMode {
    Ui,
    LatchReadinessReport,
    Other(String),
}

#[derive(Clone)]
pub struct LauncherProcessConfig {
    readiness: LauncherReadinessConfig,
}

impl LauncherProcessConfig {
    pub fn readiness(&self) -> &LauncherReadinessConfig {
        &self.readiness
    }
}

#[derive(Clone, Default)]
pub struct LauncherReadinessConfig {
    startup_token: String,
    ready_fifo: PathBuf,
    main_pid: u32,
    main_generation: u64,
    owner_epoch: u64,
}

impl LauncherReadinessConfig {
    fn from_snapshot(environment: &EnvironmentSnapshot) -> Self {
        Self {
            startup_token: environment
                .get(STARTUP_TOKEN)
                .unwrap_or_default()
                .to_owned(),
            ready_fifo: environment
                .get_path(READY_FIFO)
                .unwrap_or_default()
                .to_owned(),
            main_pid: parse_u32(environment.get(MAIN_PID)),
            main_generation: parse_u64(environment.get(MAIN_GENERATION)),
            owner_epoch: parse_u64(environment.get(OWNER_EPOCH)),
        }
    }

    pub fn into_parts(self) -> (String, PathBuf, u32, u64, u64) {
        (
            self.startup_token,
            self.ready_fifo,
            self.main_pid,
            self.main_generation,
            self.owner_epoch,
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DiagnosticConfig {
    pub latch_readiness_json: bool,
}

#[derive(Clone)]
pub struct ProcessConfig {
    command: CommandMode,
    launcher: LauncherProcessConfig,
    diagnostics: DiagnosticConfig,
    fault: Option<FaultConfig>,
}

impl ProcessConfig {
    pub fn capture(args: &[String], command: &str) -> Self {
        Self::from_snapshot(args, command, &EnvironmentSnapshot::capture_process())
    }

    fn from_snapshot(args: &[String], command: &str, environment: &EnvironmentSnapshot) -> Self {
        let command = match command {
            "ui" => CommandMode::Ui,
            "latch-readiness-report" => CommandMode::LatchReadinessReport,
            other => CommandMode::Other(other.to_owned()),
        };
        let diagnostics = DiagnosticConfig {
            latch_readiness_json: matches!(command, CommandMode::LatchReadinessReport)
                && args.iter().any(|arg| arg == "--json"),
        };
        let fault = FaultConfig::capture_with(|name| environment.get(name));
        Self {
            command,
            launcher: LauncherProcessConfig {
                readiness: LauncherReadinessConfig::from_snapshot(environment),
            },
            diagnostics,
            fault,
        }
    }

    pub fn command(&self) -> &CommandMode {
        &self.command
    }

    pub fn launcher(&self) -> &LauncherProcessConfig {
        &self.launcher
    }

    pub fn diagnostics(&self) -> DiagnosticConfig {
        self.diagnostics
    }

    pub fn fault(&self) -> Option<&FaultConfig> {
        self.fault.as_ref()
    }
}

fn parse_u32(value: Option<&str>) -> u32 {
    value.and_then(|value| value.parse().ok()).unwrap_or(0)
}

fn parse_u64(value: Option<&str>) -> u64 {
    value.and_then(|value| value.parse().ok()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_capture_preserves_compatible_values_and_invalid_defaults() {
        let environment = EnvironmentSnapshot::from_values([
            (STARTUP_TOKEN, "0123456789abcdef0123456789abcdef"),
            (READY_FIFO, "/tmp/ready"),
            (MAIN_PID, "7"),
            (MAIN_GENERATION, "11"),
            (OWNER_EPOCH, "invalid"),
        ]);
        let config = ProcessConfig::from_snapshot(
            &["mister-magik-fb".into(), "ui".into()],
            "ui",
            &environment,
        );
        assert_eq!(config.command(), &CommandMode::Ui);
        assert_eq!(
            config.launcher().readiness().clone().into_parts(),
            (
                "0123456789abcdef0123456789abcdef".into(),
                PathBuf::from("/tmp/ready"),
                7,
                11,
                0,
            )
        );
    }

    #[test]
    fn diagnostic_modifier_is_scoped_to_the_readiness_command() {
        let args = vec!["mister-magik-fb".into(), "ui".into(), "--json".into()];
        let ui = ProcessConfig::from_snapshot(&args, "ui", &EnvironmentSnapshot::default());
        assert!(!ui.diagnostics().latch_readiness_json);
        let report = ProcessConfig::from_snapshot(
            &args,
            "latch-readiness-report",
            &EnvironmentSnapshot::default(),
        );
        assert!(report.diagnostics().latch_readiness_json);
    }
}
