// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::launcher_bench::LauncherBenchScenario;
use serde::Serialize;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

pub(super) const CONTROL_PATH: &str = "/tmp/mister-magik/latch-v5-qualification-control.tsv";
pub(super) const STATE_PATH: &str = "/tmp/mister-magik/latch-v5-qualification-state.json";
const CONTROL_POLL_INTERVAL: Duration = Duration::from_secs(1);
const STATE_WRITE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum LatchV5StressClass {
    Particles,
    Transitions,
    ArcadeScroll,
    PreviewArchive,
    SearchFilterModel,
    InputTraffic,
}

impl LatchV5StressClass {
    pub(super) const ALL: [Self; 6] = [
        Self::Particles,
        Self::Transitions,
        Self::ArcadeScroll,
        Self::PreviewArchive,
        Self::SearchFilterModel,
        Self::InputTraffic,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Particles => "particles",
            Self::Transitions => "transitions",
            Self::ArcadeScroll => "arcade-scroll",
            Self::PreviewArchive => "preview-archive",
            Self::SearchFilterModel => "search-filter-model",
            Self::InputTraffic => "input-traffic",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|stress_class| stress_class.label() == value)
    }

    pub(super) const fn bench_scenario(self) -> Option<LauncherBenchScenario> {
        match self {
            Self::Particles => None,
            Self::Transitions => Some(LauncherBenchScenario::QuickTap),
            Self::ArcadeScroll => Some(LauncherBenchScenario::HumanTurboHold),
            Self::PreviewArchive => Some(LauncherBenchScenario::PreviewStepHold),
            Self::SearchFilterModel => Some(LauncherBenchScenario::ModelSync),
            Self::InputTraffic => Some(LauncherBenchScenario::RapidTaps),
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Particles => 0,
            Self::Transitions => 1,
            Self::ArcadeScroll => 2,
            Self::PreviewArchive => 3,
            Self::SearchFilterModel => 4,
            Self::InputTraffic => 5,
        }
    }
}

#[derive(Debug)]
pub(super) struct LatchV5Qualification {
    enabled: bool,
    started: Instant,
    stress_class: LatchV5StressClass,
    catalog_request: u64,
    catalog_started: u64,
    catalog_completed: u64,
    catalog_worker_was_running: bool,
    accepted_confirmed_frames: u64,
    catalog_overlap_frames: u64,
    stress_class_frames: [u64; 6],
    next_control_poll: Instant,
    next_state_write: Instant,
    control_error: Option<String>,
}

impl LatchV5Qualification {
    pub(super) fn from_env(now: Instant) -> Self {
        let enabled = matches!(
            std::env::var("MISTER_LATCH_V5_QUALIFICATION")
                .ok()
                .as_deref(),
            Some("1") | Some("on") | Some("true") | Some("yes")
        );
        Self {
            enabled,
            started: now,
            stress_class: LatchV5StressClass::Particles,
            catalog_request: 0,
            catalog_started: 0,
            catalog_completed: 0,
            catalog_worker_was_running: false,
            accepted_confirmed_frames: 0,
            catalog_overlap_frames: 0,
            stress_class_frames: [0; 6],
            next_control_poll: now,
            next_state_write: now,
            control_error: None,
        }
    }

    pub(super) const fn enabled(&self) -> bool {
        self.enabled
    }

    pub(super) const fn stress_class(&self) -> LatchV5StressClass {
        self.stress_class
    }

    pub(super) fn poll_control(&mut self, now: Instant) {
        if !self.enabled || now < self.next_control_poll {
            return;
        }
        self.next_control_poll = now + CONTROL_POLL_INTERVAL;
        match fs::read_to_string(CONTROL_PATH).and_then(|text| self.apply_control(&text)) {
            Ok(()) => self.control_error = None,
            Err(error) => self.control_error = Some(error.to_string()),
        }
    }

    fn apply_control(&mut self, text: &str) -> std::io::Result<()> {
        let mut valid = false;
        let mut stress_class = None;
        let mut catalog_request = None;
        for field in text.split_whitespace() {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            match key {
                "schema" => valid = value == "mister-magik-latch-v5-qualification-control-v1",
                "stress_class" => stress_class = LatchV5StressClass::parse(value),
                "catalog_request" => catalog_request = value.parse::<u64>().ok(),
                _ => {}
            }
        }
        if !valid || stress_class.is_none() || catalog_request.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "qualification control is incomplete",
            ));
        }
        self.stress_class = stress_class.expect("checked above");
        self.catalog_request = catalog_request.expect("checked above");
        Ok(())
    }

    pub(super) fn take_catalog_request(&mut self, worker_running: bool) -> bool {
        if !self.enabled
            || worker_running
            || self.catalog_started >= self.catalog_request
            || self.catalog_started != self.catalog_completed
        {
            return false;
        }
        self.catalog_started = self.catalog_started.saturating_add(1);
        true
    }

    pub(super) fn observe_catalog_worker(&mut self, worker_running: bool, refresh_done: bool) {
        if !self.enabled {
            return;
        }
        if self.catalog_worker_was_running && !worker_running && refresh_done {
            self.catalog_completed = self.catalog_started;
        }
        self.catalog_worker_was_running = worker_running;
    }

    pub(super) fn record_present(
        &mut self,
        accepted_and_active_confirmed: bool,
        catalog_worker_running: bool,
    ) {
        if !self.enabled || !accepted_and_active_confirmed {
            return;
        }
        self.accepted_confirmed_frames = self.accepted_confirmed_frames.saturating_add(1);
        self.stress_class_frames[self.stress_class.index()] =
            self.stress_class_frames[self.stress_class.index()].saturating_add(1);
        if catalog_worker_running {
            self.catalog_overlap_frames = self.catalog_overlap_frames.saturating_add(1);
        }
    }

    pub(super) fn write_state_if_due(&mut self, now: Instant) {
        if !self.enabled || now < self.next_state_write {
            return;
        }
        self.next_state_write = now + STATE_WRITE_INTERVAL;
        let identity = crate::diagnostic_identity::current();
        let state = QualificationState {
            schema: "mister-magik-latch-v5-qualification-state-v1",
            identity,
            identity_namespace: identity.namespace(),
            elapsed_ms: now.saturating_duration_since(self.started).as_millis() as u64,
            stress_class: self.stress_class,
            catalog_requested: self.catalog_request,
            catalog_started: self.catalog_started,
            catalog_completed: self.catalog_completed,
            accepted_confirmed_frames: self.accepted_confirmed_frames,
            catalog_overlap_frames: self.catalog_overlap_frames,
            stress_class_frames: StressClassFrames {
                particles: self.stress_class_frames[0],
                transitions: self.stress_class_frames[1],
                arcade_scroll: self.stress_class_frames[2],
                preview_archive: self.stress_class_frames[3],
                search_filter_model: self.stress_class_frames[4],
                input_traffic: self.stress_class_frames[5],
            },
            control_error: self.control_error.as_deref(),
        };
        if let Err(error) = write_json_atomic(Path::new(STATE_PATH), &state) {
            crate::ui_errln!("latch_v5_qualification_state_write_failed error={error}");
        }
    }
}

#[derive(Serialize)]
struct QualificationState<'a> {
    schema: &'static str,
    identity: &'a crate::diagnostic_identity::DiagnosticIdentity,
    identity_namespace: String,
    elapsed_ms: u64,
    stress_class: LatchV5StressClass,
    catalog_requested: u64,
    catalog_started: u64,
    catalog_completed: u64,
    accepted_confirmed_frames: u64,
    catalog_overlap_frames: u64,
    stress_class_frames: StressClassFrames,
    control_error: Option<&'a str>,
}

#[derive(Serialize)]
struct StressClassFrames {
    particles: u64,
    transitions: u64,
    arcade_scroll: u64,
    preview_archive: u64,
    search_filter_model: u64,
    input_traffic: u64,
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "state path has no parent")
    })?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.tmp");
    let mut bytes = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_requires_exact_schema_class_and_generation() {
        let now = Instant::now();
        let mut qualification = LatchV5Qualification {
            enabled: true,
            ..LatchV5Qualification::from_env(now)
        };
        qualification
            .apply_control(
                "schema=mister-magik-latch-v5-qualification-control-v1 \
                 stress_class=arcade-scroll catalog_request=7",
            )
            .unwrap();
        assert_eq!(
            qualification.stress_class(),
            LatchV5StressClass::ArcadeScroll
        );
        assert_eq!(qualification.catalog_request, 7);
        assert!(
            qualification
                .apply_control("stress_class=particles")
                .is_err()
        );
    }

    #[test]
    fn catalog_generation_is_serial_and_completion_is_edge_triggered() {
        let now = Instant::now();
        let mut qualification = LatchV5Qualification {
            enabled: true,
            catalog_request: 2,
            ..LatchV5Qualification::from_env(now)
        };
        assert!(qualification.take_catalog_request(false));
        assert!(!qualification.take_catalog_request(false));
        qualification.observe_catalog_worker(true, false);
        qualification.observe_catalog_worker(false, true);
        assert_eq!(qualification.catalog_completed, 1);
        assert!(qualification.take_catalog_request(false));
    }

    #[test]
    fn only_active_confirmed_frames_contribute_to_required_counters() {
        let now = Instant::now();
        let mut qualification = LatchV5Qualification {
            enabled: true,
            stress_class: LatchV5StressClass::Transitions,
            ..LatchV5Qualification::from_env(now)
        };
        qualification.record_present(false, true);
        qualification.record_present(true, true);
        assert_eq!(qualification.accepted_confirmed_frames, 1);
        assert_eq!(qualification.catalog_overlap_frames, 1);
        assert_eq!(qualification.stress_class_frames[1], 1);
    }
}
