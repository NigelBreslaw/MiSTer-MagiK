// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::process_config::LauncherStartupTestMode;
use std::path::{Path, PathBuf};

const READY_FAIL_OPEN: Duration = Duration::from_secs(20);
const HOLD_FAIL_OPEN: Duration = Duration::from_secs(10);

pub(super) struct CatalogPublicationTestDriver {
    ready_gate: Option<PathBuf>,
    first_frame_release_gate: Option<PathBuf>,
    replay_catalog: Option<ArcadeCatalog>,
    startup_mode: Option<LauncherStartupTestMode>,
    ready_sent: bool,
    ready_deadline: Option<Instant>,
    ready_at: Option<Instant>,
    holding_first_frame: bool,
    hold_deadline: Option<Instant>,
}

impl CatalogPublicationTestDriver {
    pub(super) fn from_config(
        config: &mister_magik_fb::process_config::LauncherTestConfig,
        start: Instant,
        enabled: bool,
    ) -> Self {
        let ready_gate = config.catalog_publication_gate().map(Path::to_path_buf);
        let first_frame_release_gate = config.first_frame_release_gate().map(Path::to_path_buf);
        let session = config.catalog_publication_session();
        let armed = ready_gate.is_some()
            && first_frame_release_gate.is_some()
            && session
                .as_deref()
                .is_some_and(|path| Path::new(path).exists());
        if let Some(path) = session.as_deref().filter(|_| armed) {
            let _ = std::fs::remove_file(path);
        }
        if armed {
            print_startup_event(
                start,
                "catalog_publication_test_armed",
                "scenario=fresh-ready",
            );
        }
        let startup_mode = enabled.then(|| config.startup_mode()).flatten();
        if startup_mode.is_some() {
            print_startup_event(
                start,
                "startup_ui_test_mode",
                format!("mode={}", startup_mode_label(startup_mode)),
            );
        }
        Self {
            ready_gate: armed.then_some(ready_gate).flatten(),
            first_frame_release_gate: armed.then_some(first_frame_release_gate).flatten(),
            replay_catalog: None,
            startup_mode,
            ready_sent: false,
            ready_deadline: armed.then_some(start + READY_FAIL_OPEN),
            ready_at: None,
            holding_first_frame: false,
            hold_deadline: None,
        }
    }

    pub(super) fn prepare_startup_catalog(
        &mut self,
        root: &str,
        catalog: &mut ArcadeCatalog,
        catalog_ready: &mut bool,
        start: Instant,
    ) -> bool {
        let cold_mode = matches!(
            self.startup_mode,
            Some(
                LauncherStartupTestMode::WarmHydrating
                    | LauncherStartupTestMode::ColdDelayed
                    | LauncherStartupTestMode::ColdIntroFailure,
            )
        );
        if (!cold_mode && self.ready_gate.is_none()) || !*catalog_ready {
            return false;
        }
        self.replay_catalog = Some(catalog.clone());
        *catalog = empty_arcade_catalog(root);
        *catalog_ready = false;
        print_startup_event(
            start,
            "catalog_publication_test_waiting",
            format!("scenario={}", self.scenario_label()),
        );
        if cold_mode {
            self.ready_at = Some(start + Duration::from_millis(500));
        }
        true
    }

    pub(super) fn startup_catalog_hydration_pending(&self) -> bool {
        matches!(
            self.startup_mode,
            Some(LauncherStartupTestMode::WarmHydrating)
        )
    }

    pub(super) fn catalog_worker_allowed(&self) -> bool {
        self.ready_gate.is_none() && self.startup_mode.is_none()
    }

    pub(super) fn tick(&mut self, now: Instant, start: Instant) -> Option<CatalogWorkerMessage> {
        if self.ready_sent {
            return None;
        }
        let gate_open = self
            .ready_gate
            .as_deref()
            .is_some_and(|path| Path::new(path).exists());
        let startup_mode_ready = self.ready_at.is_some_and(|ready_at| now >= ready_at);
        let fail_open = self.ready_deadline.is_some_and(|deadline| now >= deadline);
        if !gate_open && !startup_mode_ready && !fail_open {
            return None;
        }
        if fail_open {
            print_startup_event(
                start,
                "catalog_publication_test_fail_open",
                "phase=ready-wait",
            );
        }
        let catalog = self.replay_catalog.take()?;
        self.ready_sent = true;
        print_startup_event(
            start,
            "catalog_publication_test_ready",
            format!(
                "games={} systems={} scenario={}",
                catalog.len(),
                catalog.systems.len(),
                self.scenario_label()
            ),
        );
        Some(CatalogWorkerMessage::Ready {
            catalog,
            load_us: 0,
            source: CatalogSource::FreshBuild,
            durable_save_pending: false,
            generation_fingerprint: None,
            publication_ack: None,
        })
    }

    pub(super) fn hold_first_launcher_frame(&mut self, start: Instant) {
        if self.first_frame_release_gate.is_some() && !self.holding_first_frame {
            self.holding_first_frame = true;
            self.hold_deadline = Some(Instant::now() + HOLD_FAIL_OPEN);
            print_startup_event(
                start,
                "catalog_publication_test_first_frame_held",
                format!("scenario={}", self.scenario_label()),
            );
        }
    }

    pub(super) fn wait_for_first_frame_release(&mut self, now: Instant, start: Instant) -> bool {
        if !self.holding_first_frame {
            return false;
        }
        let released = self
            .first_frame_release_gate
            .as_deref()
            .is_some_and(|path| Path::new(path).exists());
        let fail_open = self.hold_deadline.is_some_and(|deadline| now >= deadline);
        if released || fail_open {
            if fail_open {
                print_startup_event(
                    start,
                    "catalog_publication_test_fail_open",
                    "phase=first-frame-hold",
                );
            }
            self.holding_first_frame = false;
            self.hold_deadline = None;
            return false;
        }
        true
    }

    fn scenario_label(&self) -> &'static str {
        if self.ready_gate.is_some() {
            "fresh-ready"
        } else {
            startup_mode_label(self.startup_mode)
        }
    }
}

fn startup_mode_label(mode: Option<LauncherStartupTestMode>) -> &'static str {
    match mode {
        Some(LauncherStartupTestMode::WarmReady) => "warm-ready",
        Some(LauncherStartupTestMode::WarmHydrating) => "warm-hydrating",
        Some(LauncherStartupTestMode::ColdDelayed) => "cold-delayed",
        Some(LauncherStartupTestMode::ColdIntroFailure) => "cold-intro-failure",
        None => "unconfigured",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfigured_publication_test_is_inert() {
        let driver = CatalogPublicationTestDriver::from_config(
            &mister_magik_fb::process_config::LauncherTestConfig::default(),
            Instant::now(),
            false,
        );
        assert!(driver.ready_gate.is_none());
        assert!(driver.first_frame_release_gate.is_none());
    }
}
