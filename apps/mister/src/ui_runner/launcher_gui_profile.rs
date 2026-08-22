// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::launcher_compositor::LauncherPresentResult;
use super::launcher_frame_accounting::LauncherCustomDrawTrace;

const ENABLE_ENV: &str = "MISTER_GUI_FRAME_PROFILE";
const COMPLETE_ENV: &str = "MISTER_GUI_FRAME_PROFILE_COMPLETE";
const PMU_ENV: &str = "MISTER_GUI_FRAME_PROFILE_PMU";
const ROUTE_ENV: &str = "MISTER_GUI_FRAME_PROFILE_ROUTE";
const PHASE_TIMEOUT: Duration = Duration::from_secs(20);
const ARCADE_SCROLL_PHASE_TIMEOUT: Duration = Duration::from_secs(50);
const FRAME_LIMIT: usize = 4_096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GuiProfileConfig {
    enabled: bool,
    completion_path: Option<PathBuf>,
    pmu_requested: bool,
    route: GuiProfileRoute,
}

impl GuiProfileConfig {
    pub(crate) fn capture_with<'a>(mut get: impl FnMut(&str) -> Option<&'a str>) -> Self {
        Self {
            enabled: get(ENABLE_ENV).is_some_and(profile_flag_is_true),
            completion_path: get(COMPLETE_ENV)
                .map(PathBuf::from)
                .filter(|path| valid_volatile_profile_path(path)),
            pmu_requested: get(PMU_ENV).is_some_and(profile_flag_is_true),
            route: GuiProfileRoute::from_value(get(ROUTE_ENV)),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum GuiProfileRoute {
    #[default]
    ArcadeVelocity,
    SettledComposition,
    BridgeChurn,
}

impl GuiProfileRoute {
    fn from_value(value: Option<&str>) -> Self {
        match value {
            Some("settled-composition") => Self::SettledComposition,
            Some("bridge-churn") => Self::BridgeChurn,
            _ => Self::ArcadeVelocity,
        }
    }

    const fn phases(self) -> &'static [GuiProfilePhase] {
        match self {
            Self::ArcadeVelocity => &GuiProfilePhase::ARCADE_VELOCITY_ORDERED,
            Self::SettledComposition => &GuiProfilePhase::SETTLED_COMPOSITION_ORDERED,
            Self::BridgeChurn => &GuiProfilePhase::BRIDGE_CHURN_ORDERED,
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct GuiPhysicalLayerPresentationTrace {
    preview_rows: u32,
    hidden_copied_bytes: u64,
    preview_hidden_compose_us: u64,
    arcade_hidden_compose_us: u64,
    preview_present_us: u64,
    arcade_present_us: u64,
}

impl From<&LauncherPresentResult> for GuiPhysicalLayerPresentationTrace {
    fn from(presentation: &LauncherPresentResult) -> Self {
        Self {
            preview_rows: presentation.direct_preview_rows,
            hidden_copied_bytes: presentation.main_present_hidden_copied_bytes as u64,
            preview_hidden_compose_us: u128_to_u64_saturating(
                presentation.hidden_preview_compose_us,
            ),
            arcade_hidden_compose_us: u128_to_u64_saturating(presentation.hidden_arcade_compose_us),
            preview_present_us: u128_to_u64_saturating(presentation.direct_preview_present_us),
            arcade_present_us: u128_to_u64_saturating(presentation.arcade_list_present_us),
        }
    }
}

pub(super) struct GuiFrameWorkRecord<'a> {
    pub(super) frame: u64,
    pub(super) wall_us: u128,
    pub(super) vsync_us: u128,
    pub(super) custom_draw: &'a LauncherCustomDrawTrace,
    pub(super) physical_layer_presentation: GuiPhysicalLayerPresentationTrace,
}

impl<'a> GuiFrameWorkRecord<'a> {
    pub(super) fn from_traces(
        frame: u64,
        wall_us: u128,
        vsync_us: u128,
        custom_draw: &'a LauncherCustomDrawTrace,
        presentation: &LauncherPresentResult,
    ) -> Self {
        Self {
            frame,
            wall_us,
            vsync_us,
            custom_draw,
            physical_layer_presentation: presentation.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GuiProfilePhase {
    ArcadeScroll,
    SettledArcade,
    CustomDamage,
    ModalOverArcade,
    SettingsDestination,
    SettingsFollowing,
    MediaProgress,
    MenuSelection,
    LightBridge,
}

impl GuiProfilePhase {
    const ARCADE_VELOCITY_ORDERED: [Self; 2] = [Self::ArcadeScroll, Self::SettledArcade];
    const SETTLED_COMPOSITION_ORDERED: [Self; 4] = [
        Self::CustomDamage,
        Self::ModalOverArcade,
        Self::SettingsDestination,
        Self::SettingsFollowing,
    ];
    const BRIDGE_CHURN_ORDERED: [Self; 3] =
        [Self::MediaProgress, Self::MenuSelection, Self::LightBridge];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::ArcadeScroll => "arcade-scroll",
            Self::SettledArcade => "settled-arcade",
            Self::CustomDamage => "custom-damage",
            Self::ModalOverArcade => "modal-over-arcade",
            Self::SettingsDestination => "settings-destination",
            Self::SettingsFollowing => "settings-following",
            Self::MediaProgress => "media-progress",
            Self::MenuSelection => "menu-selection",
            Self::LightBridge => "light-bridge",
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
    route: GuiProfileRoute,
    settled_modal_presentations: u8,
    settings_destination_frame: Option<u64>,
    phase_markers: Vec<serde_json::Value>,
    bridge_churn_summary: Option<serde_json::Value>,
    last_loop_start: Option<Instant>,
    last_frame_t4: Option<Instant>,
    last_timing_finalized_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct GuiFrameTimingTrace {
    pub(super) loop_start: Instant,
    pub(super) frame_t4: Instant,
    pub(super) timing_finalized_at: Instant,
    pub(super) vsync_us: u128,
    pub(super) pre_render_wait_us: u128,
    pub(super) post_present_wait_us: u128,
    pub(super) frame_finish_us: u128,
    pub(super) prepare_us: u128,
    pub(super) pre_render_stage_us: u128,
    pub(super) slint_render_us: u128,
    pub(super) custom_draw_us: u128,
    pub(super) post_custom_to_present_us: u128,
    pub(super) bridge_sync_us: u128,
    pub(super) bridge_model_projection_us: u128,
    pub(super) bridge_model_replacements: u64,
    pub(super) bridge_row_mutations: u64,
    pub(super) bridge_row_allocations: u64,
    pub(super) bridge_shared_string_constructions: u64,
    pub(super) bridge_model_allocation_us: u64,
    pub(super) media_worker_us: u128,
    pub(super) media_gate_us: u128,
    pub(super) preview_schedule_us: u128,
    pub(super) preview_apply_us: u128,
    pub(super) hidden_compose_us: u128,
    pub(super) hidden_preview_compose_us: u128,
    pub(super) hidden_arcade_compose_us: u128,
    pub(super) main_present_hidden_copy_us: u128,
    pub(super) main_present_request_us: u128,
    pub(super) frame_start_phase_us: u64,
    pub(super) present_phase_us: u128,
    pub(super) redraw_pending: bool,
    pub(super) wake_reasons_bits: u64,
    pub(super) completion_poll_count: u16,
    pub(super) completion_poll_wall_us: u64,
    pub(super) completion_poll_cpu_us: u64,
}

impl GuiFrameTimingTrace {
    pub(super) fn from_presented_frame(
        frame: &super::launcher_frame_accounting::LauncherPresentedFrame,
        frame_finish_us: u128,
    ) -> Self {
        Self {
            loop_start: frame.loop_start,
            frame_t4: frame.frame_t4,
            timing_finalized_at: Instant::now(),
            vsync_us: frame.vsync_us_override.unwrap_or_else(|| {
                frame
                    .frame_t3
                    .saturating_duration_since(frame.custom_draw_done)
                    .as_micros()
            }),
            pre_render_wait_us: frame.pre_render_wait_us,
            post_present_wait_us: frame.post_present_wait_us,
            frame_finish_us,
            prepare_us: frame.prepare_us,
            pre_render_stage_us: frame
                .frame_t1
                .saturating_duration_since(frame.frame_t0)
                .as_micros(),
            slint_render_us: frame
                .frame_t2
                .saturating_duration_since(frame.frame_t1)
                .as_micros(),
            custom_draw_us: frame
                .custom_draw_done
                .saturating_duration_since(frame.custom_draw_start)
                .as_micros(),
            post_custom_to_present_us: frame
                .frame_t3
                .saturating_duration_since(frame.custom_draw_done)
                .as_micros(),
            bridge_sync_us: frame.prepare_trace.bridge_sync_us,
            bridge_model_projection_us: frame.prepare_trace.bridge_model_projection_us,
            bridge_model_replacements: frame.prepare_trace.bridge_model_replacements,
            bridge_row_mutations: frame.prepare_trace.bridge_row_mutations,
            bridge_row_allocations: frame.prepare_trace.bridge_row_allocations,
            bridge_shared_string_constructions: frame
                .prepare_trace
                .bridge_shared_string_constructions,
            bridge_model_allocation_us: frame.prepare_trace.bridge_model_allocation_us,
            media_worker_us: frame.prepare_trace.media_worker_us,
            media_gate_us: frame.prepare_trace.media_gate_us,
            preview_schedule_us: frame.prepare_trace.preview_schedule_us,
            preview_apply_us: frame.prepare_trace.preview_apply_us,
            hidden_compose_us: frame.hidden_compose_us,
            hidden_preview_compose_us: frame.hidden_preview_compose_us,
            hidden_arcade_compose_us: frame.hidden_arcade_compose_us,
            main_present_hidden_copy_us: frame.main_present_hidden_copy_us,
            main_present_request_us: frame.main_present_request_us,
            frame_start_phase_us: frame.frame_start_phase_us,
            present_phase_us: frame.present_phase_us,
            redraw_pending: frame.redraw_pending,
            wake_reasons_bits: frame.wake_reasons_bits,
            completion_poll_count: frame.main_present_completion_poll_count,
            completion_poll_wall_us: frame.main_present_completion_poll_wall_us,
            completion_poll_cpu_us: frame.main_present_completion_poll_cpu_us,
        }
    }
}

impl GuiProfilingController {
    pub(super) fn from_config(config: GuiProfileConfig) -> Self {
        let GuiProfileConfig {
            enabled,
            completion_path,
            pmu_requested,
            route,
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
            route,
            settled_modal_presentations: 0,
            settings_destination_frame: None,
            phase_markers: Vec::with_capacity(route.phases().len() * 2),
            bridge_churn_summary: None,
            last_loop_start: None,
            last_frame_t4: None,
            last_timing_finalized_at: None,
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
            route: GuiProfileRoute::ArcadeVelocity,
            settled_modal_presentations: 0,
            settings_destination_frame: None,
            phase_markers: Vec::new(),
            bridge_churn_summary: None,
            last_loop_start: None,
            last_frame_t4: None,
            last_timing_finalized_at: None,
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
            route: GuiProfileRoute::ArcadeVelocity,
            settled_modal_presentations: 0,
            settings_destination_frame: None,
            phase_markers: Vec::new(),
            bridge_churn_summary: None,
            last_loop_start: None,
            last_frame_t4: None,
            last_timing_finalized_at: None,
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

    pub(super) fn pmu_requested(&self) -> bool {
        self.pmu_requested
    }

    pub(super) fn bridge_churn_route(&self) -> bool {
        self.route == GuiProfileRoute::BridgeChurn
    }

    pub(super) fn set_bridge_churn_summary(&mut self, summary: serde_json::Value) {
        if self.bridge_churn_route() {
            self.bridge_churn_summary = Some(summary);
        }
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

    pub(super) fn arcade_scroll_phase_started(&self) -> bool {
        matches!(
            self.state,
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::ArcadeScroll)
                | GuiProfileState::Measuring(GuiProfilePhase::ArcadeScroll)
        )
    }

    pub(super) fn settled_arcade_phase_pending(&self) -> bool {
        self.state == GuiProfileState::AwaitingPresentation(GuiProfilePhase::SettledArcade)
    }

    pub(super) fn request_phase(
        &mut self,
        phase: GuiProfilePhase,
        now: Instant,
    ) -> Result<(), String> {
        if !self.enabled() {
            return Ok(());
        }
        let expected = self.route.phases().get(self.next_phase).copied();
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
        if phase == GuiProfilePhase::ModalOverArcade {
            self.settled_modal_presentations = 0;
        }
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
        if self.next_phase == self.route.phases().len() {
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
        let phase = match (self.route, screen, event.action) {
            (
                GuiProfileRoute::ArcadeVelocity,
                "arcade",
                crate::input_event::LogicalAction::Down,
            ) => Some(GuiProfilePhase::ArcadeScroll),
            (GuiProfileRoute::BridgeChurn, "home", crate::input_event::LogicalAction::Y)
                if self.state == GuiProfileState::Warmup =>
            {
                Some(GuiProfilePhase::MediaProgress)
            }
            (
                GuiProfileRoute::SettledComposition,
                "arcade",
                crate::input_event::LogicalAction::Left,
            ) if self.state == GuiProfileState::Warmup => Some(GuiProfilePhase::CustomDamage),
            (
                GuiProfileRoute::SettledComposition,
                "arcade",
                crate::input_event::LogicalAction::X,
            ) if self.state == GuiProfileState::Measuring(GuiProfilePhase::CustomDamage) => {
                Some(GuiProfilePhase::ModalOverArcade)
            }
            (
                GuiProfileRoute::SettledComposition,
                "home",
                crate::input_event::LogicalAction::Activate,
            ) if self.state == GuiProfileState::Measuring(GuiProfilePhase::ModalOverArcade) => {
                Some(GuiProfilePhase::SettingsDestination)
            }
            _ => None,
        };
        if let Some(phase) = phase {
            if matches!(
                self.state,
                GuiProfileState::AwaitingPresentation(active)
                    | GuiProfileState::Measuring(active)
                    if active == phase
            ) {
                return;
            }
            let _ = self.request_phase(phase, now);
        }
    }

    pub(super) fn observe_route_presentation(
        &mut self,
        frame: u64,
        screen: &'static str,
        arcade_motion_active: bool,
        terminal_preview: bool,
        confirm_visible: bool,
        composition: &crate::launcher_runtime::composition::UiCompositionStatus,
        now: Instant,
        monotonic_us: u64,
    ) {
        if !self.enabled() {
            return;
        }
        let phase = match self.state {
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
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::CustomDamage)
                if screen == "arcade"
                    && composition.state == "mixed-arcade"
                    && composition.retirement_state == "idle" =>
            {
                Some(GuiProfilePhase::CustomDamage)
            }
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::ModalOverArcade)
                if screen == "arcade"
                    && confirm_visible
                    && composition.state == "modal-over-arcade"
                    && composition.retirement_state == "idle"
                    && !composition.retirement_receipt.is_empty() =>
            {
                self.settled_modal_presentations =
                    self.settled_modal_presentations.saturating_add(1);
                (self.settled_modal_presentations >= 8).then_some(GuiProfilePhase::ModalOverArcade)
            }
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::SettingsDestination)
                if screen == "settings"
                    && composition.state == "navigation-destination"
                    && composition.retirement_state == "idle" =>
            {
                self.settings_destination_frame = Some(frame);
                Some(GuiProfilePhase::SettingsDestination)
            }
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::SettingsFollowing)
                if screen == "settings"
                    && self
                        .settings_destination_frame
                        .is_some_and(|destination| frame > destination) =>
            {
                Some(GuiProfilePhase::SettingsFollowing)
            }
            _ => None,
        };
        let Some(phase) = phase else {
            return;
        };
        if self
            .confirm_phase_presented(phase, now, monotonic_us)
            .is_ok()
        {
            match phase {
                GuiProfilePhase::ArcadeScroll => {
                    let _ = self.request_phase(GuiProfilePhase::SettledArcade, now);
                }
                GuiProfilePhase::SettingsDestination => {
                    let _ = self.request_phase(GuiProfilePhase::SettingsFollowing, now);
                }
                _ => {}
            }
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

    pub(super) fn record_frame_work(&mut self, input: GuiFrameWorkRecord<'_>) {
        if !self.active() {
            return;
        }
        let GuiFrameWorkRecord {
            frame,
            wall_us,
            vsync_us,
            custom_draw,
            physical_layer_presentation,
        } = input;
        let persistent_arcade_composition = custom_draw.persistent_arcade_composition;
        let Some(record) =
            self.frames.iter_mut().rev().find(|record| {
                record.get("frame").and_then(serde_json::Value::as_u64) == Some(frame)
            })
        else {
            self.dropped_frames = self.dropped_frames.saturating_add(1);
            return;
        };
        record["wall_us"] = json!(wall_us.min(u128::from(u64::MAX)) as u64);
        record["vsync_us"] = json!(vsync_us.min(u128::from(u64::MAX)) as u64);
        record["custom_damage_invalidation"] = json!({
            "arcade_bbox": custom_draw.arcade_bbox_invalidation,
            "arcade_rects": custom_draw.arcade_rect_invalidation,
            "arcade_false_positive": custom_draw.arcade_false_positive_invalidation,
            "preview_bbox": custom_draw.preview_bbox_invalidation,
            "preview_rects": custom_draw.preview_rect_invalidation,
            "preview_false_positive": custom_draw.preview_false_positive_invalidation,
        });
        record["crt_backdrop_prepare_us"] = json!(custom_draw.crt_backdrop_prepare_us);
        record["crt_backdrop_prepare_pixels"] = json!(custom_draw.crt_backdrop_prepare_pixels);
        record["crt_backdrop_blend_us"] = json!(custom_draw.crt_backdrop_blend_us);
        record["crt_backdrop_blend_pixels"] = json!(custom_draw.crt_backdrop_blend_pixels);
        record["crt_backdrop_copy_us"] = json!(custom_draw.crt_backdrop_copy_us);
        record["crt_backdrop_copy_pixels"] = json!(custom_draw.crt_backdrop_copy_pixels);
        record["crt_backdrop_list_overlay_us"] = json!(custom_draw.crt_backdrop_list_overlay_us);
        record["crt_backdrop_list_overlay_pixels"] =
            json!(custom_draw.crt_backdrop_list_overlay_pixels);
        record["crt_backdrop_list_restore_pixels"] =
            json!(custom_draw.crt_backdrop_list_restore_pixels);
        record["crt_backdrop_list_foreground_pixels"] =
            json!(custom_draw.crt_backdrop_list_foreground_pixels);
        record["crt_backdrop_alpha_bucket"] = json!(custom_draw.crt_backdrop_alpha_bucket);
        record["crt_backdrop_active"] = json!(custom_draw.crt_backdrop_active);
        record["crt_backdrop_selected"] = json!(custom_draw.crt_backdrop_selected);
        record["crt_backdrop_transition_id"] = json!(custom_draw.crt_backdrop_transition_id);
        record["crt_backdrop_cache_state"] = json!(custom_draw.crt_backdrop_cache_state);
        record["portrait_arcade_list_pixels"] = json!(custom_draw.portrait_arcade_list_pixels);
        record["portrait_arcade_list_bytes"] = json!(custom_draw.portrait_arcade_list_bytes);
        record["portrait_arcade_requested_update"] =
            json!(persistent_arcade_composition.requested_update.label());
        record["portrait_arcade_requested_reason"] =
            json!(persistent_arcade_composition.requested_reason.label());
        record["portrait_arcade_effective_update"] =
            json!(persistent_arcade_composition.effective_update.label());
        record["portrait_arcade_rebuild_reason"] =
            json!(persistent_arcade_composition.rebuild_reason.label());
        record["portrait_arcade_compose_us"] = json!(persistent_arcade_composition.elapsed_us);
        record["portrait_arcade_composed_pixels"] =
            json!(persistent_arcade_composition.written_pixels);
        record["portrait_arcade_allocated_bytes"] =
            json!(persistent_arcade_composition.allocated_bytes);
        record["portrait_preview_rotation_pixels"] =
            json!(custom_draw.portrait_preview_rotation_pixels);
        record["portrait_preview_blend_pixels"] = json!(custom_draw.portrait_preview_blend_pixels);
        record["portrait_preview_worker_queue_replacements"] =
            json!(custom_draw.portrait_preview_worker_queue_replacements);
        record["portrait_preview_worker_result_replacements"] =
            json!(custom_draw.portrait_preview_worker_result_replacements);
        record["portrait_preview_worker_stale_results"] =
            json!(custom_draw.portrait_preview_worker_stale_results);
        record["portrait_preview_worker_age_us"] =
            json!(custom_draw.portrait_preview_worker_age_us);
        record["portrait_preview_worker_generation_lag"] =
            json!(custom_draw.portrait_preview_worker_generation_lag);
        record["portrait_preview_worker_affinity_status"] =
            json!(custom_draw.portrait_preview_worker_affinity_status);
        record["portrait_preview_worker_errors"] =
            json!(custom_draw.portrait_preview_worker_errors);
        record["portrait_preview_worker_adoption_failures"] =
            json!(custom_draw.portrait_preview_worker_adoption_failures);
        record["portrait_preview_worker_alive"] = json!(custom_draw.portrait_preview_worker_alive);
        record["physical_layers"] = json!({
            "arcade": {
                "pixels": custom_draw.portrait_arcade_list_pixels,
                "bytes": custom_draw.portrait_arcade_list_bytes,
                "requested_update": persistent_arcade_composition.requested_update.label(),
                "requested_reason": persistent_arcade_composition.requested_reason.label(),
                "effective_update": persistent_arcade_composition.effective_update.label(),
                "rebuild_reason": persistent_arcade_composition.rebuild_reason.label(),
                "compose_us": persistent_arcade_composition.elapsed_us,
                "composed_pixels": persistent_arcade_composition.written_pixels,
                "allocated_bytes": persistent_arcade_composition.allocated_bytes,
                "hidden_compose_us": physical_layer_presentation.arcade_hidden_compose_us,
                "present_us": physical_layer_presentation.arcade_present_us,
            },
            "preview": {
                "rotation_pixels": custom_draw.portrait_preview_rotation_pixels,
                "blend_pixels": custom_draw.portrait_preview_blend_pixels,
                "hidden_compose_us": physical_layer_presentation.preview_hidden_compose_us,
                "present_us": physical_layer_presentation.preview_present_us,
                "copied_rows": physical_layer_presentation.preview_rows,
            },
            "presentation": {
                "hidden_copied_bytes": physical_layer_presentation.hidden_copied_bytes,
            },
        });
    }

    pub(super) fn record_composition(
        &mut self,
        frame: u64,
        status: &crate::launcher_runtime::composition::UiCompositionStatus,
        force_full_present: bool,
        force_full_raster: bool,
        full_frame_present: bool,
        navigation_transition_active: bool,
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
        let full_present_reason = if force_full_raster {
            "composition-forced-raster"
        } else if force_full_present {
            "composition-forced-present"
        } else if navigation_transition_active && full_frame_present {
            "navigation-transition"
        } else if full_frame_present {
            "other-full"
        } else {
            "damage"
        };
        record["composition"] = json!({
            "state": status.state,
            "retirement_state": status.retirement_state,
            "retirement_generation": status.retirement_generation,
            "retirement_obligations": status.retirement_obligations,
            "retirement_receipt": status.retirement_receipt,
            "retirement_receipt_sequence": status.retirement_receipt_sequence,
            "retirement_receipt_slot": status.retirement_receipt_slot,
            "retirement_receipt_route_epoch": status.retirement_receipt_route_epoch,
            "force_full_present": force_full_present,
            "force_full_raster": force_full_raster,
            "full_frame_present": full_frame_present,
            "full_present_reason": full_present_reason,
        });
    }

    pub(super) fn finalize_frame_timing(&mut self, frame: u64, timing: GuiFrameTimingTrace) {
        if !self.active() {
            return;
        }
        let loop_delta_us = self
            .last_loop_start
            .map(|previous| {
                timing
                    .loop_start
                    .saturating_duration_since(previous)
                    .as_micros()
            })
            .unwrap_or(0);
        let post_frame_tail_us = self
            .last_frame_t4
            .map(|previous| {
                timing
                    .loop_start
                    .saturating_duration_since(previous)
                    .as_micros()
            })
            .unwrap_or(0);
        let post_finish_tail_us = self
            .last_timing_finalized_at
            .map(|previous| {
                timing
                    .loop_start
                    .saturating_duration_since(previous)
                    .as_micros()
            })
            .unwrap_or(0);
        self.last_loop_start = Some(timing.loop_start);
        self.last_frame_t4 = Some(timing.frame_t4);
        self.last_timing_finalized_at = Some(timing.timing_finalized_at);

        let Some(record) =
            self.frames.iter_mut().rev().find(|record| {
                record.get("frame").and_then(serde_json::Value::as_u64) == Some(frame)
            })
        else {
            self.dropped_frames = self.dropped_frames.saturating_add(1);
            return;
        };
        record["wall_us"] = json!(saturating_duration_us(timing.loop_start, timing.frame_t4));
        record["vsync_us"] = json!(u128_to_u64_saturating(timing.vsync_us));
        record["loop_delta_us"] = json!(u128_to_u64_saturating(loop_delta_us));
        record["pre_render_wait_us"] = json!(u128_to_u64_saturating(timing.pre_render_wait_us));
        record["post_present_wait_us"] = json!(u128_to_u64_saturating(timing.post_present_wait_us));
        record["post_frame_tail_us"] = json!(u128_to_u64_saturating(post_frame_tail_us));
        record["frame_finish_us"] = json!(u128_to_u64_saturating(timing.frame_finish_us));
        record["prepare_us"] = json!(u128_to_u64_saturating(timing.prepare_us));
        record["pre_render_stage_us"] = json!(u128_to_u64_saturating(timing.pre_render_stage_us));
        record["slint_render_us"] = json!(u128_to_u64_saturating(timing.slint_render_us));
        record["custom_draw_us"] = json!(u128_to_u64_saturating(timing.custom_draw_us));
        record["post_custom_to_present_us"] =
            json!(u128_to_u64_saturating(timing.post_custom_to_present_us));
        record["bridge_sync_us"] = json!(u128_to_u64_saturating(timing.bridge_sync_us));
        record["bridge_model_projection_us"] =
            json!(u128_to_u64_saturating(timing.bridge_model_projection_us));
        record["bridge_model_replacements"] = json!(timing.bridge_model_replacements);
        record["bridge_row_mutations"] = json!(timing.bridge_row_mutations);
        record["bridge_row_allocations"] = json!(timing.bridge_row_allocations);
        record["bridge_shared_string_constructions"] =
            json!(timing.bridge_shared_string_constructions);
        record["bridge_model_allocation_us"] = json!(timing.bridge_model_allocation_us);
        record["media_worker_us"] = json!(u128_to_u64_saturating(timing.media_worker_us));
        record["media_gate_us"] = json!(u128_to_u64_saturating(timing.media_gate_us));
        record["preview_schedule_us"] = json!(u128_to_u64_saturating(timing.preview_schedule_us));
        record["preview_apply_us"] = json!(u128_to_u64_saturating(timing.preview_apply_us));
        record["hidden_compose_us"] = json!(u128_to_u64_saturating(timing.hidden_compose_us));
        record["hidden_preview_compose_us"] =
            json!(u128_to_u64_saturating(timing.hidden_preview_compose_us));
        record["hidden_arcade_compose_us"] =
            json!(u128_to_u64_saturating(timing.hidden_arcade_compose_us));
        record["main_present_hidden_copy_us"] =
            json!(u128_to_u64_saturating(timing.main_present_hidden_copy_us));
        record["main_present_request_us"] =
            json!(u128_to_u64_saturating(timing.main_present_request_us));
        record["post_finish_tail_us"] = json!(u128_to_u64_saturating(post_finish_tail_us));
        record["frame_start_phase_us"] = json!(timing.frame_start_phase_us);
        record["present_phase_us"] = json!(u128_to_u64_saturating(timing.present_phase_us));
        record["redraw_pending"] = json!(timing.redraw_pending);
        record["wake_reasons_bits"] = json!(timing.wake_reasons_bits);
        record["completion_poll_count"] = json!(timing.completion_poll_count);
        record["completion_poll_wall_us"] = json!(timing.completion_poll_wall_us);
        record["completion_poll_cpu_us"] = json!(timing.completion_poll_cpu_us);
    }

    pub(super) fn record_latch(
        &mut self,
        frame: u64,
        copied_bytes: usize,
        invalid_bytes: usize,
        catchup_bytes: usize,
        copied_rectangles: u32,
        full_copy: bool,
        target_slot: u8,
        copy_path: &'static str,
        arcade_copy: crate::arcade_list_renderer::PersistentArcadeCopyTrace,
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
            "copied_bytes": copied_bytes,
            "invalid_bytes": invalid_bytes,
            "catchup_bytes": catchup_bytes,
            "copied_rectangles": copied_rectangles,
            "full_copy": full_copy,
            "target_slot": target_slot,
            "copy_path": copy_path,
            "arcade_copy_decision": arcade_copy.decision.label(),
            "arcade_diff_safe": arcade_copy.diff_safe,
            "arcade_mirror_valid": arcade_copy.mirror_valid,
            "arcade_compare_us": arcade_copy.compare_us,
            "arcade_write_us": arcade_copy.write_us,
            "arcade_mirror_refresh_us": arcade_copy.mirror_refresh_us,
            "arcade_compared_pixels": arcade_copy.compared_pixels,
            "arcade_written_pixels": arcade_copy.written_pixels,
            "arcade_mirror_refresh_pixels": arcade_copy.mirror_refresh_pixels,
            "arcade_changed_rows": arcade_copy.changed_rows,
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
        let bridge_churn_summary = self.bridge_churn_summary.take();
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
                "bridge_churn": bridge_churn_summary,
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

fn u128_to_u64_saturating(value: u128) -> u64 {
    value.min(u128::from(u64::MAX)) as u64
}

fn saturating_duration_us(start: Instant, end: Instant) -> u64 {
    u128_to_u64_saturating(end.saturating_duration_since(start).as_micros())
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
        complete_through(&mut controller, &GuiProfilePhase::ARCADE_VELOCITY_ORDERED);
        assert_eq!(controller.state, GuiProfileState::Complete);
        assert!(!controller.active());
    }

    #[test]
    fn settled_composition_waits_for_retirement_and_immediate_following_frame() {
        let now = Instant::now();
        let mut controller = GuiProfilingController::enabled_for_test(now);
        controller.route = GuiProfileRoute::SettledComposition;
        controller
            .request_phase(GuiProfilePhase::CustomDamage, now)
            .unwrap();
        controller.observe_route_presentation(
            0,
            "arcade",
            false,
            true,
            false,
            &crate::launcher_runtime::composition::UiCompositionStatus {
                state: "mixed-arcade",
                retirement_state: "idle",
                ..crate::launcher_runtime::composition::UiCompositionStatus::default()
            },
            now,
            999,
        );
        assert_eq!(
            controller.state,
            GuiProfileState::Measuring(GuiProfilePhase::CustomDamage)
        );
        controller
            .request_phase(GuiProfilePhase::ModalOverArcade, now)
            .unwrap();
        let modal = crate::launcher_runtime::composition::UiCompositionStatus {
            state: "modal-over-arcade",
            retirement_state: "idle",
            retirement_receipt: "sequence=9 slot=1 route_epoch=3 carrier=full-slint".into(),
            ..crate::launcher_runtime::composition::UiCompositionStatus::default()
        };
        for frame in 1..=7 {
            controller.observe_route_presentation(
                frame,
                "arcade",
                false,
                true,
                true,
                &modal,
                now,
                1_000 + frame,
            );
            assert_eq!(
                controller.state,
                GuiProfileState::AwaitingPresentation(GuiProfilePhase::ModalOverArcade)
            );
        }
        controller.observe_route_presentation(8, "arcade", false, true, true, &modal, now, 1_008);
        assert_eq!(
            controller.state,
            GuiProfileState::Measuring(GuiProfilePhase::ModalOverArcade)
        );

        controller
            .request_phase(GuiProfilePhase::SettingsDestination, now)
            .unwrap();
        let destination = crate::launcher_runtime::composition::UiCompositionStatus {
            state: "navigation-destination",
            retirement_state: "idle",
            ..crate::launcher_runtime::composition::UiCompositionStatus::default()
        };
        controller.observe_route_presentation(
            20,
            "settings",
            false,
            false,
            false,
            &destination,
            now,
            2_000,
        );
        assert_eq!(
            controller.state,
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::SettingsFollowing)
        );
        controller.observe_route_presentation(
            21,
            "settings",
            false,
            false,
            false,
            &crate::launcher_runtime::composition::UiCompositionStatus::default(),
            now,
            2_001,
        );
        assert_eq!(controller.state, GuiProfileState::Complete);
    }

    #[test]
    fn missing_presentation_times_out() {
        let now = Instant::now();
        let mut controller = GuiProfilingController::enabled_for_test(now);
        controller
            .request_phase(GuiProfilePhase::ArcadeScroll, now)
            .unwrap();
        controller.tick(now + ARCADE_SCROLL_PHASE_TIMEOUT);
        assert!(matches!(controller.state, GuiProfileState::Failed(_)));
    }

    #[test]
    fn automated_turbo_repress_keeps_arcade_scroll_phase_active() {
        let now = Instant::now();
        let mut controller = GuiProfilingController::enabled_for_test(now);
        let event = crate::input_event::InputEvent {
            source: crate::input_event::InputSourceId {
                kind: crate::input_event::InputSourceKind::Automation,
                instance: 1,
            },
            source_epoch: crate::input_event::SourceEpoch(1),
            sequence: 1,
            press_id: crate::input_event::PressId(1),
            captured_at_us: 1,
            action: crate::input_event::LogicalAction::Down,
            phase: crate::input_event::InputPhase::Pressed,
        };
        controller.observe_route_action("arcade", event, now);
        controller.observe_route_action("arcade", event, now + Duration::from_millis(50));
        assert_eq!(
            controller.state,
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::ArcadeScroll)
        );
    }

    #[test]
    fn frame_work_records_foreground_and_crt_backdrop_costs() {
        let now = Instant::now();
        let mut controller = GuiProfilingController::enabled_for_test(now);
        controller
            .request_phase(GuiProfilePhase::ArcadeScroll, now)
            .unwrap();
        controller.record_frame(
            7,
            2_000,
            "event-driven",
            GuiBridgeProfilePhase::None,
            GuiRasterProfilePhase::Ordinary,
            Vec::new(),
        );
        let custom_draw = LauncherCustomDrawTrace {
            crt_backdrop_prepare_us: 220,
            crt_backdrop_prepare_pixels: 307_200,
            crt_backdrop_blend_us: 180,
            crt_backdrop_blend_pixels: 307_200,
            crt_backdrop_copy_us: 11,
            crt_backdrop_copy_pixels: 307_200,
            crt_backdrop_list_overlay_us: 12,
            crt_backdrop_list_overlay_pixels: 100_000,
            crt_backdrop_list_restore_pixels: 80_000,
            crt_backdrop_list_foreground_pixels: 20_000,
            crt_backdrop_alpha_bucket: 4,
            crt_backdrop_active: true,
            crt_backdrop_selected: 7,
            crt_backdrop_transition_id: 11,
            crt_backdrop_cache_state: "exact",
            portrait_arcade_list_pixels: 12_345,
            portrait_arcade_list_bytes: 24_690,
            persistent_arcade_composition:
                crate::arcade_list_renderer::PersistentArcadeCompositionTrace {
                    allocated_bytes: 456_000,
                    ..crate::arcade_list_renderer::PersistentArcadeCompositionTrace::default()
                },
            portrait_preview_rotation_pixels: 2_048,
            portrait_preview_blend_pixels: 1_024,
            portrait_preview_worker_queue_replacements: 3,
            portrait_preview_worker_result_replacements: 2,
            portrait_preview_worker_stale_results: 1,
            portrait_preview_worker_age_us: 4_500,
            portrait_preview_worker_generation_lag: 0,
            portrait_preview_worker_affinity_status: "applied",
            ..LauncherCustomDrawTrace::default()
        };
        controller.record_frame_work(GuiFrameWorkRecord {
            frame: 7,
            wall_us: 8_500,
            vsync_us: 3_000,
            custom_draw: &custom_draw,
            physical_layer_presentation: GuiPhysicalLayerPresentationTrace {
                preview_rows: 12,
                hidden_copied_bytes: 64,
                preview_hidden_compose_us: 13,
                arcade_hidden_compose_us: 14,
                preview_present_us: 15,
                arcade_present_us: 16,
            },
        });
        controller.record_latch(
            7,
            64,
            128,
            32,
            2,
            false,
            1,
            "vertical-partial",
            crate::arcade_list_renderer::PersistentArcadeCopyTrace {
                decision: crate::arcade_list_renderer::PersistentArcadeCopyDecision::SparseDiff,
                diff_safe: true,
                mirror_valid: true,
                compare_us: 10,
                write_us: 20,
                mirror_refresh_us: 5,
                compared_pixels: 100,
                written_pixels: 32,
                mirror_refresh_pixels: 100,
                changed_rows: 2,
            },
        );
        assert_eq!(controller.frames[0]["wall_us"], 8_500);
        assert_eq!(controller.frames[0]["vsync_us"], 3_000);
        assert_eq!(controller.frames[0]["crt_backdrop_prepare_us"], 220);
        assert_eq!(controller.frames[0]["crt_backdrop_prepare_pixels"], 307_200);
        assert_eq!(controller.frames[0]["crt_backdrop_blend_us"], 180);
        assert_eq!(controller.frames[0]["crt_backdrop_blend_pixels"], 307_200);
        assert_eq!(controller.frames[0]["crt_backdrop_copy_us"], 11);
        assert_eq!(controller.frames[0]["crt_backdrop_list_overlay_us"], 12);
        assert_eq!(
            controller.frames[0]["crt_backdrop_list_restore_pixels"],
            80_000
        );
        assert_eq!(
            controller.frames[0]["crt_backdrop_list_foreground_pixels"],
            20_000
        );
        assert_eq!(controller.frames[0]["crt_backdrop_alpha_bucket"], 4);
        assert_eq!(controller.frames[0]["crt_backdrop_active"], true);
        assert_eq!(controller.frames[0]["crt_backdrop_selected"], 7);
        assert_eq!(controller.frames[0]["crt_backdrop_transition_id"], 11);
        assert_eq!(controller.frames[0]["crt_backdrop_cache_state"], "exact");
        assert_eq!(controller.frames[0]["portrait_arcade_list_pixels"], 12_345);
        assert_eq!(controller.frames[0]["portrait_arcade_list_bytes"], 24_690);
        assert_eq!(
            controller.frames[0]["portrait_arcade_allocated_bytes"],
            456_000
        );
        assert_eq!(
            controller.frames[0]["latch"]["arcade_copy_decision"],
            "sparse-diff"
        );
        assert_eq!(controller.frames[0]["latch"]["arcade_compare_us"], 10);
        assert_eq!(controller.frames[0]["latch"]["arcade_written_pixels"], 32);
        assert_eq!(
            controller.frames[0]["portrait_preview_rotation_pixels"],
            2_048
        );
        assert_eq!(controller.frames[0]["portrait_preview_blend_pixels"], 1_024);
        assert_eq!(
            controller.frames[0]["portrait_preview_worker_queue_replacements"],
            3
        );
        assert_eq!(
            controller.frames[0]["portrait_preview_worker_age_us"],
            4_500
        );
        assert_eq!(
            controller.frames[0]["portrait_preview_worker_affinity_status"],
            "applied"
        );
        assert_eq!(
            controller.frames[0]["physical_layers"]["arcade"]["pixels"],
            12_345
        );
        assert_eq!(
            controller.frames[0]["physical_layers"]["arcade"]["bytes"],
            24_690
        );
        assert_eq!(
            controller.frames[0]["physical_layers"]["arcade"]["allocated_bytes"],
            456_000
        );
        assert_eq!(
            controller.frames[0]["physical_layers"]["arcade"]["hidden_compose_us"],
            14
        );
        assert_eq!(
            controller.frames[0]["physical_layers"]["preview"]["rotation_pixels"],
            controller.frames[0]["portrait_preview_rotation_pixels"]
        );
        assert_eq!(
            controller.frames[0]["physical_layers"]["presentation"]["hidden_copied_bytes"],
            64
        );
    }

    #[test]
    fn frame_work_for_an_unrecorded_frame_counts_as_record_loss() {
        let now = Instant::now();
        let mut controller = GuiProfilingController::enabled_for_test(now);
        controller
            .request_phase(GuiProfilePhase::ArcadeScroll, now)
            .unwrap();
        let custom_draw = LauncherCustomDrawTrace::default();
        controller.record_frame_work(GuiFrameWorkRecord {
            frame: 99,
            wall_us: 1,
            vsync_us: 1,
            custom_draw: &custom_draw,
            physical_layer_presentation: GuiPhysicalLayerPresentationTrace::default(),
        });
        assert_eq!(controller.dropped_frames, 1);
    }

    #[test]
    fn finalized_timing_replaces_pre_confirmation_work_timing() {
        let now = Instant::now();
        let mut controller = GuiProfilingController::enabled_for_test(now);
        controller
            .request_phase(GuiProfilePhase::ArcadeScroll, now)
            .unwrap();
        controller.record_frame(
            7,
            2_000,
            "event-driven",
            GuiBridgeProfilePhase::None,
            GuiRasterProfilePhase::Ordinary,
            Vec::new(),
        );
        controller.finalize_frame_timing(
            7,
            GuiFrameTimingTrace {
                loop_start: now,
                frame_t4: now + Duration::from_micros(8_000),
                timing_finalized_at: now + Duration::from_micros(9_000),
                vsync_us: 5_000,
                pre_render_wait_us: 100,
                post_present_wait_us: 4_800,
                frame_finish_us: 1_000,
                prepare_us: 10,
                pre_render_stage_us: 20,
                slint_render_us: 30,
                custom_draw_us: 40,
                post_custom_to_present_us: 50,
                bridge_sync_us: 60,
                bridge_model_projection_us: 70,
                bridge_model_replacements: 1,
                bridge_row_mutations: 2,
                bridge_row_allocations: 3,
                bridge_shared_string_constructions: 4,
                bridge_model_allocation_us: 5,
                media_worker_us: 80,
                media_gate_us: 90,
                preview_schedule_us: 100,
                preview_apply_us: 110,
                hidden_compose_us: 120,
                hidden_preview_compose_us: 130,
                hidden_arcade_compose_us: 140,
                main_present_hidden_copy_us: 150,
                main_present_request_us: 160,
                frame_start_phase_us: 2_000,
                present_phase_us: 7_500,
                redraw_pending: true,
                wake_reasons_bits: 5,
                completion_poll_count: 3,
                completion_poll_wall_us: 4_700,
                completion_poll_cpu_us: 40,
            },
        );
        controller.record_frame(
            8,
            18_667,
            "event-driven",
            GuiBridgeProfilePhase::None,
            GuiRasterProfilePhase::Ordinary,
            Vec::new(),
        );
        controller.finalize_frame_timing(
            8,
            GuiFrameTimingTrace {
                loop_start: now + Duration::from_micros(16_667),
                frame_t4: now + Duration::from_micros(25_000),
                timing_finalized_at: now + Duration::from_micros(26_000),
                vsync_us: 5_100,
                pre_render_wait_us: 110,
                post_present_wait_us: 4_900,
                frame_finish_us: 1_100,
                prepare_us: 11,
                pre_render_stage_us: 21,
                slint_render_us: 31,
                custom_draw_us: 41,
                post_custom_to_present_us: 51,
                bridge_sync_us: 61,
                bridge_model_projection_us: 71,
                bridge_model_replacements: 0,
                bridge_row_mutations: 2,
                bridge_row_allocations: 0,
                bridge_shared_string_constructions: 4,
                bridge_model_allocation_us: 0,
                media_worker_us: 81,
                media_gate_us: 91,
                preview_schedule_us: 101,
                preview_apply_us: 111,
                hidden_compose_us: 121,
                hidden_preview_compose_us: 131,
                hidden_arcade_compose_us: 141,
                main_present_hidden_copy_us: 151,
                main_present_request_us: 161,
                frame_start_phase_us: 2_100,
                present_phase_us: 7_600,
                redraw_pending: false,
                wake_reasons_bits: 2,
                completion_poll_count: 4,
                completion_poll_wall_us: 4_800,
                completion_poll_cpu_us: 50,
            },
        );

        let first = &controller.frames[0];
        assert_eq!(first["wall_us"], 8_000);
        assert_eq!(first["post_present_wait_us"], 4_800);
        assert_eq!(first["frame_finish_us"], 1_000);
        assert_eq!(first["bridge_sync_us"], 60);
        assert_eq!(first["hidden_arcade_compose_us"], 140);
        assert_eq!(first["redraw_pending"], true);
        assert_eq!(first["completion_poll_count"], 3);
        let second = &controller.frames[1];
        assert_eq!(second["loop_delta_us"], 16_667);
        assert_eq!(second["post_frame_tail_us"], 8_667);
        assert_eq!(second["post_finish_tail_us"], 7_667);
        assert_eq!(second["wake_reasons_bits"], 2);
    }

    #[test]
    fn arcade_scroll_allows_the_fixed_twenty_second_hold() {
        let now = Instant::now();
        let mut controller = GuiProfilingController::enabled_for_test(now);
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
                .request_phase(GuiProfilePhase::SettledArcade, now)
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
        controller
            .request_phase(GuiProfilePhase::ArcadeScroll, now)
            .unwrap();
        let composition = crate::launcher_runtime::composition::UiCompositionStatus::default();
        controller.observe_route_presentation(
            1,
            "arcade",
            false,
            false,
            false,
            &composition,
            now,
            4_000,
        );
        assert_eq!(
            controller.state,
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::ArcadeScroll)
        );
        controller.observe_route_presentation(
            2,
            "arcade",
            false,
            true,
            false,
            &composition,
            now,
            5_000,
        );
        assert_eq!(
            controller.state,
            GuiProfileState::AwaitingPresentation(GuiProfilePhase::SettledArcade)
        );
        controller.observe_route_presentation(
            3,
            "arcade",
            false,
            true,
            false,
            &composition,
            now,
            6_000,
        );
        assert_eq!(controller.state, GuiProfileState::Complete);
    }
}
