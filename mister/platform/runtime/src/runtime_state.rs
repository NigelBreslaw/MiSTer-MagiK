// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Typed inspection of the supervised MiSTer Main runtime.

use mister_magik_core::launcher_effects::{LauncherEffectFailure, MainRuntimeState, RuntimeState};
use serde_json::Value;
use std::process::Command;
use std::{fs, io};

const MAIN_STATUS_PATH: &str = "/tmp/mister-magik/main-status.json";
const MAIN_PROCESS_NAMES: &[&str] = &["MiSTer_MagiKDev", "MiSTer_MagiK", "MiSTer"];

#[derive(Default)]
pub struct SystemRuntimeState;

impl RuntimeState for SystemRuntimeState {
    fn main_state(&mut self) -> Result<MainRuntimeState, LauncherEffectFailure> {
        Ok(main_state_with(main_cmdline, main_heartbeat()))
    }
}

fn main_state_with(
    mut observe: impl FnMut(&str) -> io::Result<Option<Vec<u8>>>,
    heartbeat_boot_ms: Option<u64>,
) -> MainRuntimeState {
    for name in MAIN_PROCESS_NAMES {
        match observe(name) {
            Ok(Some(cmdline)) => {
                return MainRuntimeState {
                    running: true,
                    magik_owned: matches!(*name, "MiSTer_MagiKDev" | "MiSTer_MagiK"),
                    arcade_core: cmdline_is_arcade_core(&cmdline),
                    heartbeat_boot_ms,
                };
            }
            Ok(None) => continue,
            Err(_) => break,
        }
    }
    MainRuntimeState {
        heartbeat_boot_ms,
        ..MainRuntimeState::default()
    }
}

fn main_cmdline(name: &str) -> io::Result<Option<Vec<u8>>> {
    let output = Command::new("pidof").arg(name).output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let pid = std::str::from_utf8(&output.stdout)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        .split_whitespace()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "pidof returned no PID"))?;
    fs::read(format!("/proc/{pid}/cmdline")).map(Some)
}

fn cmdline_is_arcade_core(cmdline: &[u8]) -> bool {
    let text = String::from_utf8_lossy(cmdline);
    text.contains(".rbf") && !text.contains("menu.rbf")
}

fn main_heartbeat() -> Option<u64> {
    let text = fs::read_to_string(MAIN_STATUS_PATH).ok()?;
    serde_json::from_str::<Value>(&text)
        .ok()?
        .get("ts_boot_ms")
        .and_then(Value::as_u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detected_main_identity_controls_magik_ownership() {
        for (process_name, magik_owned) in [
            ("MiSTer", false),
            ("MiSTer_MagiK", true),
            ("MiSTer_MagiKDev", true),
        ] {
            let state = main_state_with(
                |name| {
                    Ok((name == process_name)
                        .then(|| format!("{name}\0/media/fat/_Arcade/Test.rbf\0").into_bytes()))
                },
                Some(1234),
            );
            assert_eq!(
                state,
                MainRuntimeState {
                    running: true,
                    magik_owned,
                    arcade_core: true,
                    heartbeat_boot_ms: Some(1234),
                },
                "{process_name}",
            );
        }
    }

    #[test]
    fn detection_prefers_dev_then_magik_then_stock() {
        for available in [
            vec!["MiSTer", "MiSTer_MagiK", "MiSTer_MagiKDev"],
            vec!["MiSTer", "MiSTer_MagiK"],
        ] {
            let mut queried = Vec::new();
            let state = main_state_with(
                |name| {
                    queried.push(name.to_owned());
                    Ok(available
                        .contains(&name)
                        .then(|| b"Main\0menu.rbf\0".to_vec()))
                },
                None,
            );
            assert!(state.running);
            assert!(state.magik_owned);
            assert!(!state.arcade_core);
            assert_eq!(
                queried.last().map(String::as_str),
                available.last().copied()
            );
        }
    }

    #[test]
    fn missing_process_does_not_infer_ownership_from_heartbeat() {
        let state = main_state_with(|_| Ok(None), Some(1234));
        assert_eq!(
            state,
            MainRuntimeState {
                heartbeat_boot_ms: Some(1234),
                ..MainRuntimeState::default()
            }
        );
    }

    #[test]
    fn failed_observation_does_not_guess_another_process() {
        let mut queried = Vec::new();
        let state = main_state_with(
            |name| {
                queried.push(name.to_owned());
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "unreadable process",
                ))
            },
            None,
        );
        assert_eq!(state, MainRuntimeState::default());
        assert_eq!(queried, ["MiSTer_MagiKDev"]);
    }

    #[test]
    fn arcade_core_classification_preserves_menu_exclusion() {
        assert!(cmdline_is_arcade_core(
            b"MiSTer_MagiKDev\0/media/fat/_Arcade/Test.rbf\0"
        ));
        assert!(!cmdline_is_arcade_core(b"MiSTer_MagiKDev\0menu.rbf\0"));
        assert!(!cmdline_is_arcade_core(b"MiSTer_MagiKDev\0"));
    }
}
