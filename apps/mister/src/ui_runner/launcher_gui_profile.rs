// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const ENABLE_ENV: &str = "MISTER_GUI_FRAME_PROFILE";
const COMPLETE_ENV: &str = "MISTER_GUI_FRAME_PROFILE_COMPLETE";
const PHASE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GuiProfilePhase {
    SettledSettings,
    HomePanRight,
    HomePanLeft,
    ArcadeScroll,
    SettledArcade,
}

impl GuiProfilePhase {
    const ORDERED: [Self; 5] = [
        Self::SettledSettings,
        Self::HomePanRight,
        Self::HomePanLeft,
        Self::ArcadeScroll,
        Self::SettledArcade,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::SettledSettings => "settled-settings",
            Self::HomePanRight => "home-pan-right",
            Self::HomePanLeft => "home-pan-left",
            Self::ArcadeScroll => "arcade-scroll",
            Self::SettledArcade => "settled-arcade",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum GuiProfileState {
    Dormant,
    Warmup,
    AwaitingPresentation(GuiProfilePhase),
    Measuring(GuiProfilePhase),
    Complete,
    Failed(String),
}

pub(super) struct GuiProfilingController {
    state: GuiProfileState,
    completion_path: Option<PathBuf>,
    deadline: Option<Instant>,
    next_phase: usize,
    measurement_started_at_us: Option<u64>,
    measurement_ended_at_us: Option<u64>,
}

impl GuiProfilingController {
    pub(super) fn from_env() -> Self {
        let enabled = super::launcher_env_flag(ENABLE_ENV);
        let completion_path = std::env::var_os(COMPLETE_ENV)
            .map(PathBuf::from)
            .filter(|path| valid_volatile_profile_path(path));
        if !enabled || completion_path.is_none() {
            return Self::dormant();
        }
        mister_magik_perf_events::clear_process_profiles();
        Self {
            state: GuiProfileState::Warmup,
            completion_path,
            deadline: Some(Instant::now() + PHASE_TIMEOUT),
            next_phase: 0,
            measurement_started_at_us: None,
            measurement_ended_at_us: None,
        }
    }

    fn dormant() -> Self {
        Self {
            state: GuiProfileState::Dormant,
            completion_path: None,
            deadline: None,
            next_phase: 0,
            measurement_started_at_us: None,
            measurement_ended_at_us: None,
        }
    }

    #[cfg(test)]
    fn enabled_for_test(now: Instant) -> Self {
        Self {
            state: GuiProfileState::Warmup,
            completion_path: None,
            deadline: Some(now + PHASE_TIMEOUT),
            next_phase: 0,
            measurement_started_at_us: None,
            measurement_ended_at_us: None,
        }
    }

    pub(super) fn enabled(&self) -> bool {
        !matches!(self.state, GuiProfileState::Dormant)
    }

    pub(super) fn active(&self) -> bool {
        matches!(
            self.state,
            GuiProfileState::AwaitingPresentation(_) | GuiProfileState::Measuring(_)
        )
    }

    pub(super) fn phase(&self) -> Option<GuiProfilePhase> {
        match self.state {
            GuiProfileState::AwaitingPresentation(phase) | GuiProfileState::Measuring(phase) => {
                Some(phase)
            }
            _ => None,
        }
    }

    pub(super) fn request_phase(
        &mut self,
        phase: GuiProfilePhase,
        now: Instant,
    ) -> Result<(), String> {
        if !self.enabled() {
            return Ok(());
        }
        let expected = GuiProfilePhase::ORDERED.get(self.next_phase).copied();
        if expected != Some(phase)
            || !matches!(
                self.state,
                GuiProfileState::Warmup | GuiProfileState::Measuring(_)
            )
        {
            return self.fail(format!(
                "unexpected phase {} expected {}",
                phase.label(),
                expected.map(GuiProfilePhase::label).unwrap_or("completion")
            ));
        }
        self.state = GuiProfileState::AwaitingPresentation(phase);
        self.deadline = Some(now + PHASE_TIMEOUT);
        Ok(())
    }

    pub(super) fn confirm_phase_presented(
        &mut self,
        phase: GuiProfilePhase,
        now: Instant,
        monotonic_us: u64,
    ) -> Result<(), String> {
        if !self.enabled() {
            return Ok(());
        }
        if self.state != GuiProfileState::AwaitingPresentation(phase) {
            return self.fail(format!("presentation arrived outside {}", phase.label()));
        }
        self.measurement_started_at_us.get_or_insert(monotonic_us);
        self.measurement_ended_at_us = Some(monotonic_us);
        self.next_phase = self.next_phase.saturating_add(1);
        self.deadline = Some(now + PHASE_TIMEOUT);
        if phase == GuiProfilePhase::SettledArcade {
            self.finish();
        } else {
            self.state = GuiProfileState::Measuring(phase);
        }
        Ok(())
    }

    pub(super) fn interrupt_input(&mut self) -> Result<(), String> {
        if !self.enabled() {
            return Ok(());
        }
        self.fail("profiling route interrupted by unexpected input".into())
    }

    pub(super) fn tick(&mut self, now: Instant) {
        if self.deadline.is_some_and(|deadline| now >= deadline)
            && matches!(
                self.state,
                GuiProfileState::Warmup
                    | GuiProfileState::AwaitingPresentation(_)
                    | GuiProfileState::Measuring(_)
            )
        {
            let _ = self.fail("profiling route timed out waiting for presentation".into());
        }
    }

    pub(super) fn span(&self, name: &'static str) -> Option<mister_magik_perf_events::SampledSpan> {
        self.active()
            .then(|| mister_magik_perf_events::sampled_span(name))
            .flatten()
    }

    fn finish(&mut self) {
        self.state = GuiProfileState::Complete;
        self.deadline = None;
        self.write_profile_async(None);
    }

    fn fail(&mut self, reason: String) -> Result<(), String> {
        self.state = GuiProfileState::Failed(reason.clone());
        self.deadline = None;
        self.write_profile_async(Some(reason.clone()));
        Err(reason)
    }

    fn write_profile_async(&mut self, failure: Option<String>) {
        let Some(path) = self.completion_path.take() else {
            return;
        };
        let thread_profile = mister_magik_perf_events::take_thread_profile();
        let worker_profiles = mister_magik_perf_events::take_process_profiles();
        let started_at_us = self.measurement_started_at_us;
        let ended_at_us = self.measurement_ended_at_us;
        std::thread::spawn(move || {
            let passed = failure.is_none()
                && thread_profile.enabled
                && thread_profile.failure.is_none()
                && thread_profile.dropped_spans == 0
                && !thread_profile.records.is_empty()
                && worker_profiles.dropped_profiles == 0;
            let payload = json!({
                "schema": "mister-magik-gui-profiling-window-v1",
                "state": if passed { "complete" } else { "failed" },
                "failure": failure,
                "clock_domain": "CLOCK_MONOTONIC",
                "measurement_started_at_us": started_at_us,
                "measurement_ended_at_us": ended_at_us,
                "thread_profile": thread_profile,
                "worker_profiles": worker_profiles,
            });
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, format!("{payload}\n"));
        });
    }
}

fn valid_volatile_profile_path(path: &Path) -> bool {
    path.is_absolute()
        && path.starts_with("/tmp/mister-magik")
        && !path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_through(controller: &mut GuiProfilingController, phases: &[GuiProfilePhase]) {
        let mut now = Instant::now();
        for (index, phase) in phases.iter().copied().enumerate() {
            controller.request_phase(phase, now).unwrap();
            now += Duration::from_millis(1);
            controller
                .confirm_phase_presented(phase, now, 1_000 + index as u64)
                .unwrap();
        }
    }

    #[test]
    fn fixed_phase_sequence_completes_after_final_presentation() {
        let now = Instant::now();
        let mut controller = GuiProfilingController::enabled_for_test(now);
        complete_through(&mut controller, &GuiProfilePhase::ORDERED);
        assert_eq!(controller.state, GuiProfileState::Complete);
        assert!(!controller.active());
    }

    #[test]
    fn missing_presentation_times_out() {
        let now = Instant::now();
        let mut controller = GuiProfilingController::enabled_for_test(now);
        controller
            .request_phase(GuiProfilePhase::SettledSettings, now)
            .unwrap();
        controller.tick(now + PHASE_TIMEOUT);
        assert!(matches!(controller.state, GuiProfileState::Failed(_)));
    }

    #[test]
    fn interrupted_input_fails_the_window() {
        let now = Instant::now();
        let mut controller = GuiProfilingController::enabled_for_test(now);
        assert!(controller.interrupt_input().is_err());
        assert!(matches!(controller.state, GuiProfileState::Failed(_)));
    }

    #[test]
    fn out_of_order_phase_fails_the_window() {
        let now = Instant::now();
        let mut controller = GuiProfilingController::enabled_for_test(now);
        assert!(
            controller
                .request_phase(GuiProfilePhase::HomePanRight, now)
                .is_err()
        );
        assert!(matches!(controller.state, GuiProfileState::Failed(_)));
    }

    #[test]
    fn volatile_output_path_is_bounded() {
        assert!(valid_volatile_profile_path(Path::new(
            "/tmp/mister-magik/gui-profile.json"
        )));
        assert!(!valid_volatile_profile_path(Path::new("gui-profile.json")));
        assert!(!valid_volatile_profile_path(Path::new(
            "/tmp/mister-magik/../profile.json"
        )));
    }
}
