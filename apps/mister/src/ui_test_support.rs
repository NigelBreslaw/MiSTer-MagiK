// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic data and presentation probes for attended device UI tests.
//!
//! This module is compiled only into the opt-in `ui-device-tests` target.  It
//! deliberately does not replace the production catalog or presenter; tests
//! select these helpers through the typed device-test bridge.

use crate::arcade_catalog::ArcadeCatalog;
use crate::launcher::{ArcadeSearchStatus, LauncherNav, Screen};
use crate::test_support::{arcade_catalog, arcade_game, arcade_system};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Environment switch understood by the device-test runner.
pub const FIXTURE_ENV: &str = "MISTER_UI_TEST_FIXTURE";
/// Stable fixture name used by the first-party UI suite.
pub const DETERMINISTIC_FIXTURE: &str = "deterministic-arcade-v1";

/// Effects that are intentionally disabled while a device UI journey runs.
///
/// A UI test may exercise the launch, rebuild, or restart menu entries, but it
/// must never start a real core, rewrite the device catalog, or reboot the
/// board.  Keeping this policy as data lets the typed bridge report it and
/// makes accidental production-effect calls visible in assertions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SandboxEffects {
    pub allow_core_launch: bool,
    pub allow_catalog_write: bool,
    pub allow_reboot: bool,
}

impl SandboxEffects {
    pub const fn device_ui_defaults() -> Self {
        Self {
            allow_core_launch: false,
            allow_catalog_write: false,
            allow_reboot: false,
        }
    }
}

/// Volatile, per-run storage for device fixtures and captures.
///
/// The root is created below the host/device temporary directory and removed
/// on drop.  `path_for` rejects absolute paths and parent traversal so cleanup
/// can never escape the run's sandbox.
#[derive(Debug)]
pub struct UiTestSandbox {
    root: PathBuf,
    effects: SandboxEffects,
}

impl UiTestSandbox {
    pub fn new() -> io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "mister-magik-ui-test-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root)?;
        Ok(Self {
            root,
            effects: SandboxEffects::device_ui_defaults(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn effects(&self) -> SandboxEffects {
        self.effects
    }

    pub fn path_for(&self, relative: impl AsRef<Path>) -> io::Result<PathBuf> {
        let relative = relative.as_ref();
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| component == std::path::Component::ParentDir)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "UI test sandbox paths must stay below the run root",
            ));
        }
        Ok(self.root.join(relative))
    }
}

impl Drop for UiTestSandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Build a small catalog with stable metadata for navigation and filter tests.
pub fn deterministic_catalog() -> ArcadeCatalog {
    let games = vec![
        arcade_game("Alpha Fighter")
            .preview("alpha-fighter")
            .year(1985)
            .manufacturer("Acme")
            .category("Shooter")
            .players(2)
            .control("Joystick")
            .build(),
        arcade_game("Beta Blaster")
            .preview("beta-blaster")
            .year(1987)
            .manufacturer("Acme")
            .category("Shooter")
            .players(1)
            .control("Joystick")
            .build(),
        arcade_game("Gamma Racer")
            .year(1991)
            .manufacturer("Vector")
            .category("Racing")
            .players(2)
            .control("Steering")
            .build(),
        arcade_game("Delta Puzzle")
            .year(1994)
            .manufacturer("Vector")
            .category("Puzzle")
            .players(1)
            .control("Buttons")
            .build(),
        arcade_game("Epsilon Quest")
            .year(1998)
            .manufacturer("Nova")
            .category("Adventure")
            .players(4)
            .control("Joystick")
            .build(),
        arcade_game("Zeta Sports")
            .year(2002)
            .manufacturer("Nova")
            .category("Sports")
            .players(2)
            .control("Buttons")
            .build(),
    ];
    arcade_catalog(games, vec![arcade_system("arcade", 6)])
}

/// A compact state record that can be asserted without reading framebuffer
/// memory or depending on Slint's private protocol internals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresentedState {
    pub screen: Screen,
    pub selected_index: usize,
    pub selected_game: Option<String>,
    pub search_query: String,
    pub search_status: ArcadeSearchStatus,
}

/// In-memory presentation oracle used by deterministic device journeys.
#[derive(Clone, Debug, Default)]
pub struct MemoryPresenter {
    states: Vec<PresentedState>,
}

impl MemoryPresenter {
    /// Record the state that the production presenter would expose for a frame.
    pub fn present(&mut self, nav: &LauncherNav, catalog: &ArcadeCatalog, system_id: &str) {
        let selected_game = nav
            .active_arcade_game_at(catalog, system_id, nav.arcade.selected)
            .map(|game| game.mra_path.to_string());
        self.states.push(PresentedState {
            screen: nav.screen,
            selected_index: nav.arcade.selected,
            selected_game,
            search_query: nav.arcade_search.query.clone(),
            search_status: nav.arcade_search.status,
        });
    }

    pub fn states(&self) -> &[PresentedState] {
        &self.states
    }

    pub fn latest(&self) -> Option<&PresentedState> {
        self.states.last()
    }

    pub fn clear(&mut self) {
        self.states.clear();
    }
}
