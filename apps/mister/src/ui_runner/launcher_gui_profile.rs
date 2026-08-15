// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const ENABLE_ENV: &str = "MISTER_GUI_FRAME_PROFILE";
const COMPLETE_ENV: &str = "MISTER_GUI_FRAME_PROFILE_COMPLETE";
const PMU_ENV: &str = "MISTER_GUI_FRAME_PROFILE_PMU";
const PHASE_TIMEOUT: Duration = Duration::from_secs(20);
const ARCADE_SCROLL_PHASE_TIMEOUT: Duration = Duration::from_secs(30);
const FRAME_LIMIT: usize = 4_096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GuiProfileConfig {
    enabled: bool,
    completion_path: Option<PathBuf>,
    pmu_requested: bool,
}

impl GuiProfileConfig {
    pub(crate) fn capture_with<'a>(mut get: impl FnMut(&str) -> Option<&'a str>) -> Self {
        Self {
            enabled: get(ENABLE_ENV).is_some_and(profile_flag_is_true),
            completion_path: get(COMPLETE_ENV)
                .map(PathBuf::from)
                .filter(|path| valid_volatile_profile_path(path)),
            pmu_requested: get(PMU_ENV).is_some_and(profile_flag_is_true),
        }
    }
}

fn profile_flag_is_true(value: &str) -> bool {
    matches!(value, "1" | "on" | "true" | "yes")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GuiBridgeProfilePhase {
    None,
    Light,
    Full,
}

impl GuiBridgeProfilePhase {
    pub(super) const fn span_name(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Light => Some("gui.bridge-sync.light"),
            Self::Full => Some("gui.bridge-sync.full"),
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Light => "light",
            Self::Full => "full",
        }
    }
}

pub(super) const fn gui_bridge_profile_phase(
    full_sync: bool,
    light_sync: bool,
) -> GuiBridgeProfilePhase {
    if full_sync {
        GuiBridgeProfilePhase::Full
    } else if light_sync {
        GuiBridgeProfilePhase::Light
    } else {
        GuiBridgeProfilePhase::None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GuiRasterProfilePhase {
    None,
    Ordinary,
    ForcedFull,
}

impl GuiRasterProfilePhase {
    pub(super) const fn span_name(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Ordinary => Some("gui.slint-raster.ordinary"),
            Self::ForcedFull => Some("gui.slint-raster.forced-full"),
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Ordinary => "ordinary",
            Self::ForcedFull => "forced-full",
        }
    }
}

pub(super) const fn gui_raster_profile_phase(
    rendered: bool,
    forced_full: bool,
) -> GuiRasterProfilePhase {
    if !rendered {
        GuiRasterProfilePhase::None
    } else if forced_full {
        GuiRasterProfilePhase::ForcedFull
    } else {
        GuiRasterProfilePhase::Ordinary
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct GuiCustomProfileSelection {
    pub(super) arcade_row_update: Option<&'static str>,
    pub(super) preview_composition: Option<&'static str>,
    pub(super) navigation_transition_raster: Option<&'static str>,
    pub(super) orientation_transition_raster: Option<&'static str>,
}

impl GuiCustomProfileSelection {
    pub(super) const fn any(self) -> bool {
        self.arcade_row_update.is_some()
            || self.preview_composition.is_some()
            || self.navigation_transition_raster.is_some()
            || self.orientation_transition_raster.is_some()
    }
}

pub(super) const fn gui_custom_profile_selection(
    arcade: bool,
    preview: bool,
    navigation_transition: bool,
    orientation_transition: bool,
) -> GuiCustomProfileSelection {
    GuiCustomProfileSelection {
        arcade_row_update: if arcade {
            Some("gui.custom.arcade-row-update")
        } else {
            None
        },
        preview_composition: if preview {
            Some("gui.custom.preview-cut-fade")
        } else {
            None
        },
        navigation_transition_raster: if navigation_transition {
            Some("gui.custom.navigation-transition-raster")
        } else {
            None
        },
        orientation_transition_raster: if orientation_transition {
            Some("gui.custom.orientation-transition-raster")
        } else {
            None
        },
    }
}

pub(super) const fn gui_latch_copy_span_name(
    invalid_bytes: usize,
    current_damage_bytes: usize,
) -> &'static str {
    if invalid_bytes > current_damage_bytes {
        "gui.latch.catch-up-restoration"
    } else {
        "gui.latch.base-damage-copy"
    }
}

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

    const fn timeout(self) -> Duration {
        match self {
            Self::ArcadeScroll => ARCADE_SCROLL_PHASE_TIMEOUT,
            _ => PHASE_TIMEOUT,
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
    frames: Vec<serde_json::Value>,
    dropped_frames: u64,
    pmu_requested: bool,
    phase_markers: Vec<serde_json::Value>,
}

impl GuiProfilingController {
    pub(super) fn from_config(config: GuiProfileConfig) -> Self {
        let GuiProfileConfig {
            enabled,
            completion_path,
            pmu_requested,
        } = config;
        if !enabled || completion_path.is_none() {
            return Self::dormant();
        }
        if let Some(path) = completion_path.as_deref() {
            let _ = std::fs::remove_file(path);
        }
        if pmu_requested {
            mister_magik_perf_events::clear_process_profiles();
        }
        Self {
            state: GuiProfileState::Warmup,
            completion_path,
            deadline: Some(Instant::now() + PHASE_TIMEOUT),
            next_phase: 0,
            measurement_started_at_us: None,
            measurement_ended_at_us: None,
            frames: Vec::with_capacity(FRAME_LIMIT),
            dropped_frames: 0,
            pmu_requested,
            phase_markers: Vec::with_capacity(GuiProfilePhase::ORDERED.len() * 2),
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
            frames: Vec::new(),
            dropped_frames: 0,
            pmu_requested: false,
            phase_markers: Vec::new(),
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
            frames: Vec::new(),
            dropped_frames: 0,
            pmu_requested: false,
            phase_markers: Vec::new(),
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

    pub(super) fn needs_presentation(&self) -> bool {
        matches!(self.state, GuiProfileState::AwaitingPresentation(_))
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
        self.phase_markers.push(json!({
            "phase": phase.label(),
            "event": "started",
            "monotonic_us": crate::input_hub::monotonic_us(),
        }));
        self.deadline = Some(now + phase.timeout());
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
        self.phase_markers.push(json!({
            "phase": phase.label(),
            "event": "presented",
            "monotonic_us": monotonic_us,
        }));
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

    pub(super) fn observe_route_action(
        &mut self,
        screen: &'static str,
        event: crate::input_event::InputEvent,
        now: Instant,
    ) {
        if !self.enabled() || event.phase != crate::input_event::InputPhase::Pressed {
            return;
        }
        if event.source.kind != crate::input_event::InputSourceKind::Automation {
            let _ = self.interrupt_input();
            return;
        }
        let phase = match (screen, event.action) {
            ("home", crate::input_event::LogicalAction::Right) => {
                Some(GuiProfilePhase::HomePanRight)
            }
            ("home", crate::input_event::LogicalAction::Left) => Some(GuiProfilePhase::HomePanLeft),
            ("arcade", crate::input_event::LogicalAction::Down) => {
                Some(GuiProfilePhase::ArcadeScroll)
            }
            _ => None,
        };
        if let Some(phase) = phase {
            let _ = self.request_phase(phase, now);
        }
    }

    pub(super) fn observe_route_presentation(
        &mut self,
        screen: &'static str,
        arcade_motion_active: bool,
        terminal_preview: bool,
        now: Instant,
        monotonic_us: u64,
    ) {
        if !self.enabled() {
            return;
        }
        let phase = match self.state {
            GuiProfileState::Warmup if screen == "settings" => {
                let _ = self.request_phase(GuiProfilePhase::SettledSettings, now);
                Some(GuiProfilePhase::SettledSettings)
            }
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::HomePanRight)
                if screen == "home" =>
            {
                Some(GuiProfilePhase::HomePanRight)
            }
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::HomePanLeft)
                if screen == "home" =>
            {
                Some(GuiProfilePhase::HomePanLeft)
            }
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::ArcadeScroll)
                if screen == "arcade" && !arcade_motion_active && terminal_preview =>
            {
                Some(GuiProfilePhase::ArcadeScroll)
            }
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::SettledArcade)
                if screen == "arcade" && !arcade_motion_active && terminal_preview =>
            {
                Some(GuiProfilePhase::SettledArcade)
            }
            _ => None,
        };
        let Some(phase) = phase else {
            return;
        };
        if self
            .confirm_phase_presented(phase, now, monotonic_us)
            .is_ok()
            && phase == GuiProfilePhase::ArcadeScroll
        {
            let _ = self.request_phase(GuiProfilePhase::SettledArcade, now);
        }
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
        (self.active() && self.pmu_requested)
            .then(|| mister_magik_perf_events::sampled_span(name))
            .flatten()
    }

    pub(super) fn phase_span(
        &self,
        name: Option<&'static str>,
    ) -> Option<mister_magik_perf_events::SampledSpan> {
        name.and_then(|name| self.span(name))
    }

    pub(super) fn record_frame(
        &mut self,
        frame: u64,
        monotonic_us: u64,
        logical_change_class: &'static str,
        bridge_phase: GuiBridgeProfilePhase,
        raster_phase: GuiRasterProfilePhase,
        damage_rects: Vec<[usize; 4]>,
    ) {
        if !self.active() {
            return;
        }
        self.measurement_ended_at_us = Some(monotonic_us);
        if self.frames.len() == FRAME_LIMIT {
            self.dropped_frames = self.dropped_frames.saturating_add(1);
            return;
        }
        self.frames.push(json!({
            "frame": frame,
            "monotonic_us": monotonic_us,
            "phase": self.phase().map(GuiProfilePhase::label),
            "logical_change_class": logical_change_class,
            "bridge_sync": bridge_phase.label(),
            "slint_raster": raster_phase.label(),
            "slint_damage_rects": damage_rects,
        }));
    }

    pub(super) fn record_latch(
        &mut self,
        frame: u64,
        invalid_bytes: usize,
        catchup_bytes: usize,
        copied_rectangles: u32,
        full_copy: bool,
        target_slot: u8,
        copy_path: &'static str,
    ) {
        if !self.active() {
            return;
        }
        let Some(record) =
            self.frames.iter_mut().rev().find(|record| {
                record.get("frame").and_then(serde_json::Value::as_u64) == Some(frame)
            })
        else {
            self.dropped_frames = self.dropped_frames.saturating_add(1);
            return;
        };
        record["latch"] = json!({
            "invalid_bytes": invalid_bytes,
            "catchup_bytes": catchup_bytes,
            "copied_rectangles": copied_rectangles,
            "full_copy": full_copy,
            "target_slot": target_slot,
            "copy_path": copy_path,
        });
    }

    pub(super) fn record_presentation(
        &mut self,
        frame: u64,
        telemetry: mister_magik_latch_contract::PresentationTelemetry,
        latch_drop_count: u32,
        active_sequence: u16,
    ) {
        if !self.active() {
            return;
        }
        let Some(record) =
            self.frames.iter_mut().rev().find(|record| {
                record.get("frame").and_then(serde_json::Value::as_u64) == Some(frame)
            })
        else {
            return;
        };
        record["presentation"] = json!({
            "owned_vblank_count": telemetry.owned_vblank_count,
            "presented_vblank_count": telemetry.presented_vblank_count,
            "repeated_vblank_count": telemetry.repeated_vblank_count,
            "ownership_loss_count": telemetry.ownership_loss_count,
            "latch_drop_count": latch_drop_count,
            "active_sequence": active_sequence,
            "magik_ownership": telemetry.magik_ownership(),
        });
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
        let frames = std::mem::take(&mut self.frames);
        let dropped_frames = self.dropped_frames;
        let pmu_requested = self.pmu_requested;
        let phase_markers = std::mem::take(&mut self.phase_markers);
        std::thread::spawn(move || {
            let pmu_valid = !pmu_requested
                || (thread_profile.enabled
                    && thread_profile.failure.is_none()
                    && thread_profile.dropped_spans == 0
                    && !thread_profile.records.is_empty());
            let passed = failure.is_none()
                && pmu_valid
                && worker_profiles.dropped_profiles == 0
                && dropped_frames == 0;
            let payload = json!({
                "schema": "mister-magik-gui-profiling-window-v1",
                "state": if passed { "complete" } else { "failed" },
                "failure": failure,
                "pmu_requested": pmu_requested,
                "pmu_valid": pmu_valid,
                "clock_domain": "CLOCK_MONOTONIC",
                "measurement_started_at_us": started_at_us,
                "measurement_ended_at_us": ended_at_us,
                "thread_profile": thread_profile,
                "worker_profiles": worker_profiles,
                "frames": frames,
                "dropped_frame_records": dropped_frames,
                "phase_markers": phase_markers,
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
    fn arcade_scroll_allows_the_fixed_twenty_second_hold() {
        let now = Instant::now();
        let mut controller = GuiProfilingController::enabled_for_test(now);
        complete_through(
            &mut controller,
            &[
                GuiProfilePhase::SettledSettings,
                GuiProfilePhase::HomePanRight,
                GuiProfilePhase::HomePanLeft,
            ],
        );
        let phase_start = now + Duration::from_millis(3);
        controller
            .request_phase(GuiProfilePhase::ArcadeScroll, phase_start)
            .unwrap();
        controller.tick(phase_start + Duration::from_secs(20));
        assert_eq!(
            controller.state,
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::ArcadeScroll)
        );
        controller.tick(phase_start + ARCADE_SCROLL_PHASE_TIMEOUT);
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
    fn bridge_and_raster_phase_names_are_disjoint() {
        assert_eq!(
            gui_bridge_profile_phase(false, false),
            GuiBridgeProfilePhase::None
        );
        assert_eq!(
            gui_bridge_profile_phase(false, true),
            GuiBridgeProfilePhase::Light
        );
        assert_eq!(
            gui_bridge_profile_phase(true, true),
            GuiBridgeProfilePhase::Full
        );
        assert_eq!(GuiBridgeProfilePhase::None.span_name(), None);
        assert_eq!(
            GuiBridgeProfilePhase::Light.span_name(),
            Some("gui.bridge-sync.light")
        );
        assert_eq!(
            GuiBridgeProfilePhase::Full.span_name(),
            Some("gui.bridge-sync.full")
        );
        assert_eq!(GuiRasterProfilePhase::None.span_name(), None);
        assert_eq!(
            GuiRasterProfilePhase::Ordinary.span_name(),
            Some("gui.slint-raster.ordinary")
        );
        assert_eq!(
            GuiRasterProfilePhase::ForcedFull.span_name(),
            Some("gui.slint-raster.forced-full")
        );
        assert_eq!(
            gui_raster_profile_phase(false, false),
            GuiRasterProfilePhase::None
        );
        assert_eq!(
            gui_raster_profile_phase(true, false),
            GuiRasterProfilePhase::Ordinary
        );
        assert_eq!(
            gui_raster_profile_phase(true, true),
            GuiRasterProfilePhase::ForcedFull
        );
    }

    #[test]
    fn custom_frame_classification_selects_only_applicable_spans() {
        assert_eq!(
            gui_custom_profile_selection(false, false, false, false),
            GuiCustomProfileSelection::default()
        );
        let arcade_preview = gui_custom_profile_selection(true, true, false, false);
        assert_eq!(
            arcade_preview.arcade_row_update,
            Some("gui.custom.arcade-row-update")
        );
        assert_eq!(
            arcade_preview.preview_composition,
            Some("gui.custom.preview-cut-fade")
        );
        assert_eq!(arcade_preview.navigation_transition_raster, None);
        assert_eq!(arcade_preview.orientation_transition_raster, None);
        assert!(arcade_preview.any());
        let transitions = gui_custom_profile_selection(false, false, true, true);
        assert_eq!(
            transitions.navigation_transition_raster,
            Some("gui.custom.navigation-transition-raster")
        );
        assert_eq!(
            transitions.orientation_transition_raster,
            Some("gui.custom.orientation-transition-raster")
        );
        assert_eq!(transitions.arcade_row_update, None);
        assert_eq!(transitions.preview_composition, None);
        assert!(transitions.any());
        assert!(!GuiCustomProfileSelection::default().any());
    }

    #[test]
    fn latch_copy_classification_distinguishes_base_and_stale_slot_work() {
        assert_eq!(
            gui_latch_copy_span_name(1280 * 720 * 2, 1280 * 720 * 2),
            "gui.latch.base-damage-copy"
        );
        assert_eq!(
            gui_latch_copy_span_name(4_096, 4_096),
            "gui.latch.base-damage-copy"
        );
        assert_eq!(
            gui_latch_copy_span_name(32_768, 4_096),
            "gui.latch.catch-up-restoration"
        );
        assert_eq!(
            gui_latch_copy_span_name(8_192, 8_192),
            "gui.latch.base-damage-copy"
        );
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

    #[test]
    fn route_presentations_require_terminal_arcade_preview() {
        let now = Instant::now();
        let mut controller = GuiProfilingController::enabled_for_test(now);
        controller.observe_route_presentation("settings", false, true, now, 1_000);
        assert_eq!(
            controller.state,
            GuiProfileState::Measuring(GuiProfilePhase::SettledSettings)
        );
        controller
            .request_phase(GuiProfilePhase::HomePanRight, now)
            .unwrap();
        controller.observe_route_presentation("home", false, true, now, 2_000);
        controller
            .request_phase(GuiProfilePhase::HomePanLeft, now)
            .unwrap();
        controller.observe_route_presentation("home", false, true, now, 3_000);
        controller
            .request_phase(GuiProfilePhase::ArcadeScroll, now)
            .unwrap();
        controller.observe_route_presentation("arcade", false, false, now, 4_000);
        assert_eq!(
            controller.state,
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::ArcadeScroll)
        );
        controller.observe_route_presentation("arcade", false, true, now, 5_000);
        assert_eq!(
            controller.state,
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::SettledArcade)
        );
        controller.observe_route_presentation("arcade", false, true, now, 6_000);
        assert_eq!(controller.state, GuiProfileState::Complete);
    }
}
