// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Typed inspection of the supervised MiSTer Main runtime.

use mister_magik_core::launcher_effects::{LauncherEffectFailure, MainRuntimeState, RuntimeState};
use serde_json::Value;
use std::fs;
use std::process::Command;

const MAIN_STATUS_PATH: &str = "/tmp/mister-magik/main-status.json";
const MISTER_PROCESS_NAMES: &[&str] = &["MiSTer_MagiKDev", "MiSTer_MagiK", "MiSTer"];

#[derive(Default)]
pub struct SystemRuntimeState;

impl RuntimeState for SystemRuntimeState {
    fn main_state(&mut self) -> Result<MainRuntimeState, LauncherEffectFailure> {
        let cmdline = running_main_cmdline();
        let running = cmdline.is_some();
        Ok(MainRuntimeState {
            running,
            // The compatibility probe historically treated every registered Main
            // process name as capable of the MagiK command surface.
            magik_owned: running,
            arcade_core: cmdline.as_deref().is_some_and(cmdline_is_arcade_core),
            heartbeat_boot_ms: main_heartbeat(),
        })
    }
}

fn running_main_cmdline() -> Option<Vec<u8>> {
    for name in MISTER_PROCESS_NAMES {
        let output = Command::new("pidof").arg(name).output().ok()?;
        if !output.status.success() {
            continue;
        }
        let pid = std::str::from_utf8(&output.stdout)
            .ok()?
            .split_whitespace()
            .next()?;
        return fs::read(format!("/proc/{pid}/cmdline")).ok();
    }
    None
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
    fn arcade_core_classification_preserves_menu_exclusion() {
        assert!(cmdline_is_arcade_core(
            b"MiSTer_MagiKDev\0/media/fat/_Arcade/Test.rbf\0"
        ));
        assert!(!cmdline_is_arcade_core(b"MiSTer_MagiKDev\0menu.rbf\0"));
        assert!(!cmdline_is_arcade_core(b"MiSTer_MagiKDev\0"));
    }
}
